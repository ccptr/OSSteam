// SPDX-License-Identifier: GPL-2.0-or-later
//! Steam message framing: the EMsg word and the three header forms
//! (`docs/spec/cm.md` §3.1 to §3.3).

use ossteam_proto::steam::{CMsgProtoBufHeader, EMsg};
use ossteam_types::{EResult, SteamId};
use prost::Message;

/// Set in the EMsg word when the message has a protobuf header.
pub const PROTO_FLAG: u32 = 0x8000_0000;

/// The job id meaning "no job".
pub const JOB_NONE: u64 = u64::MAX;

const EXTENDED_HEADER_LEN: usize = 36;
const EXTENDED_HEADER_VERSION: u16 = 2;
const EXTENDED_CANARY: u8 = 239;
const BASIC_HEADER_LEN: usize = 20;

/// A message header.
#[derive(Clone, Debug, PartialEq)]
pub enum Header {
    /// Boxed: the protobuf header is large and most enum values are not.
    Proto(Box<CMsgProtoBufHeader>),
    Extended(ExtendedHeader),
    Basic {
        target_job: u64,
        source_job: u64,
    },
}

/// The fixed-layout header of legacy non-protobuf messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtendedHeader {
    pub target_job: u64,
    pub source_job: u64,
    pub steam_id: SteamId,
    pub session_id: i32,
}

impl Default for ExtendedHeader {
    fn default() -> Self {
        Self {
            target_job: JOB_NONE,
            source_job: JOB_NONE,
            steam_id: SteamId(0),
            session_id: 0,
        }
    }
}

/// One Steam message: its type, header and undecoded body.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    /// The EMsg number without the protobuf flag.
    pub emsg: u32,
    pub header: Header,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    #[error("message is {0} bytes, shorter than its header")]
    Truncated(usize),
    #[error("protobuf header length {0} exceeds the message")]
    HeaderLength(u32),
    #[error("bad protobuf header: {0}")]
    ProtoHeader(#[from] prost::DecodeError),
    #[error("extended header has size {size}, version {version}, canary {canary}")]
    ExtendedHeader { size: u8, version: u16, canary: u8 },
}

/// Messages that use the basic header: only the TCP channel-encryption
/// handshake (`cm.md` §2.2).
fn uses_basic_header(emsg: u32) -> bool {
    [
        EMsg::KEMsgChannelEncryptRequest,
        EMsg::KEMsgChannelEncryptResponse,
        EMsg::KEMsgChannelEncryptResult,
    ]
    .iter()
    .any(|e| *e as u32 == emsg)
}

impl Packet {
    /// A protobuf-header message with a default header. The connection
    /// fills in the session's Steam id and session id unless they are
    /// already set.
    pub fn proto(emsg: EMsg, body: &impl Message) -> Self {
        Self {
            emsg: emsg as u32,
            header: Header::Proto(Box::default()),
            body: body.encode_to_vec(),
        }
    }

    pub fn parse(buf: &[u8]) -> Result<Self, ParseError> {
        let word = read_u32(buf, 0).ok_or(ParseError::Truncated(buf.len()))?;
        let emsg = word & !PROTO_FLAG;
        if word & PROTO_FLAG != 0 {
            let len = read_u32(buf, 4).ok_or(ParseError::Truncated(buf.len()))?;
            let start = 8usize;
            let end = start
                .checked_add(len as usize)
                .filter(|&e| e <= buf.len())
                .ok_or(ParseError::HeaderLength(len))?;
            let header = CMsgProtoBufHeader::decode(&buf[start..end])?;
            return Ok(Self {
                emsg,
                header: Header::Proto(Box::new(header)),
                body: buf[end..].to_vec(),
            });
        }
        if uses_basic_header(emsg) {
            if buf.len() < BASIC_HEADER_LEN {
                return Err(ParseError::Truncated(buf.len()));
            }
            return Ok(Self {
                emsg,
                header: Header::Basic {
                    target_job: read_u64(buf, 4).expect("length checked"),
                    source_job: read_u64(buf, 12).expect("length checked"),
                },
                body: buf[BASIC_HEADER_LEN..].to_vec(),
            });
        }
        if buf.len() < EXTENDED_HEADER_LEN {
            return Err(ParseError::Truncated(buf.len()));
        }
        let size = buf[4];
        let version = u16::from_le_bytes([buf[5], buf[6]]);
        let canary = buf[23];
        if usize::from(size) != EXTENDED_HEADER_LEN
            || version != EXTENDED_HEADER_VERSION
            || canary != EXTENDED_CANARY
        {
            return Err(ParseError::ExtendedHeader {
                size,
                version,
                canary,
            });
        }
        Ok(Self {
            emsg,
            header: Header::Extended(ExtendedHeader {
                target_job: read_u64(buf, 7).expect("length checked"),
                source_job: read_u64(buf, 15).expect("length checked"),
                steam_id: SteamId(read_u64(buf, 24).expect("length checked")),
                session_id: read_u32(buf, 32).expect("length checked") as i32,
            }),
            body: buf[EXTENDED_HEADER_LEN..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 64);
        match &self.header {
            Header::Proto(h) => {
                out.extend_from_slice(&(self.emsg | PROTO_FLAG).to_le_bytes());
                let len = h.encoded_len();
                out.extend_from_slice(&(len as u32).to_le_bytes());
                h.encode(&mut out).expect("Vec grows");
            }
            Header::Extended(h) => {
                out.extend_from_slice(&self.emsg.to_le_bytes());
                out.push(EXTENDED_HEADER_LEN as u8);
                out.extend_from_slice(&EXTENDED_HEADER_VERSION.to_le_bytes());
                out.extend_from_slice(&h.target_job.to_le_bytes());
                out.extend_from_slice(&h.source_job.to_le_bytes());
                out.push(EXTENDED_CANARY);
                out.extend_from_slice(&h.steam_id.0.to_le_bytes());
                out.extend_from_slice(&h.session_id.to_le_bytes());
            }
            Header::Basic {
                target_job,
                source_job,
            } => {
                out.extend_from_slice(&self.emsg.to_le_bytes());
                out.extend_from_slice(&target_job.to_le_bytes());
                out.extend_from_slice(&source_job.to_le_bytes());
            }
        }
        out.extend_from_slice(&self.body);
        out
    }

    /// The EMsg, if this build knows the number.
    pub fn kind(&self) -> Option<EMsg> {
        i32::try_from(self.emsg)
            .ok()
            .and_then(|v| EMsg::try_from(v).ok())
    }

    pub fn is(&self, emsg: EMsg) -> bool {
        self.emsg == emsg as u32
    }

    pub fn target_job(&self) -> u64 {
        match &self.header {
            Header::Proto(h) => h.jobid_target(),
            Header::Extended(h) => h.target_job,
            Header::Basic { target_job, .. } => *target_job,
        }
    }

    pub fn source_job(&self) -> u64 {
        match &self.header {
            Header::Proto(h) => h.jobid_source(),
            Header::Extended(h) => h.source_job,
            Header::Basic { source_job, .. } => *source_job,
        }
    }

    pub fn set_source_job(&mut self, job: u64) {
        match &mut self.header {
            Header::Proto(h) => h.jobid_source = Some(job),
            Header::Extended(h) => h.source_job = job,
            Header::Basic { source_job, .. } => *source_job = job,
        }
    }

    /// The protobuf header, if the message has one.
    pub fn proto_header(&self) -> Option<&CMsgProtoBufHeader> {
        match &self.header {
            Header::Proto(h) => Some(h),
            _ => None,
        }
    }

    pub fn proto_header_mut(&mut self) -> Option<&mut CMsgProtoBufHeader> {
        match &mut self.header {
            Header::Proto(h) => Some(h),
            _ => None,
        }
    }

    /// The header EResult of a protobuf message. Absent means the field's
    /// declared default, which is `FAIL`.
    pub fn header_eresult(&self) -> Option<EResult> {
        self.proto_header().map(|h| EResult(h.eresult()))
    }

    pub fn decode_body<M: Message + Default>(&self) -> Result<M, prost::DecodeError> {
        M::decode(self.body.as_slice())
    }
}

fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    buf.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

fn read_u64(buf: &[u8], at: usize) -> Option<u64> {
    buf.get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proto_round_trip() {
        let mut p = Packet::proto(
            EMsg::KEMsgClientHello,
            &ossteam_proto::steam::CMsgClientHello {
                protocol_version: Some(7),
            },
        );
        p.set_source_job(42);
        let wire = p.encode();
        assert_eq!(
            u32::from_le_bytes(wire[..4].try_into().unwrap()),
            EMsg::KEMsgClientHello as u32 | PROTO_FLAG
        );
        let back = Packet::parse(&wire).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.source_job(), 42);
        assert_eq!(back.target_job(), JOB_NONE);
        assert_eq!(back.kind(), Some(EMsg::KEMsgClientHello));
    }

    #[test]
    fn extended_round_trip() {
        let p = Packet {
            emsg: EMsg::KEMsgClientLogOnResponse as u32,
            header: Header::Extended(ExtendedHeader {
                target_job: 9,
                source_job: JOB_NONE,
                steam_id: SteamId::individual(5),
                session_id: -3,
            }),
            body: vec![1, 2, 3],
        };
        let wire = p.encode();
        assert_eq!(wire.len(), EXTENDED_HEADER_LEN + 3);
        assert_eq!(Packet::parse(&wire).unwrap(), p);
    }

    #[test]
    fn basic_round_trip() {
        let p = Packet {
            emsg: EMsg::KEMsgChannelEncryptRequest as u32,
            header: Header::Basic {
                target_job: JOB_NONE,
                source_job: JOB_NONE,
            },
            body: vec![0xaa; 24],
        };
        let wire = p.encode();
        assert_eq!(wire.len(), BASIC_HEADER_LEN + 24);
        assert_eq!(Packet::parse(&wire).unwrap(), p);
    }

    #[test]
    fn rejects_bad_lengths() {
        assert_eq!(Packet::parse(&[1, 2]), Err(ParseError::Truncated(2)));
        let mut wire = Vec::new();
        wire.extend_from_slice(&(1u32 | PROTO_FLAG).to_le_bytes());
        wire.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            Packet::parse(&wire),
            Err(ParseError::HeaderLength(u32::MAX))
        );
        let mut ext = vec![0u8; EXTENDED_HEADER_LEN];
        ext[..4].copy_from_slice(&751u32.to_le_bytes());
        assert!(matches!(
            Packet::parse(&ext),
            Err(ParseError::ExtendedHeader { .. })
        ));
    }
}
