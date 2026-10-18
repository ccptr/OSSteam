// SPDX-License-Identifier: GPL-2.0-or-later
//! The game IPC protocol between `ossteamd` and `ossteam-steamclient`
//! (`steamworks-abi.md` §3, SPEC §12.4): its messages, their encoding,
//! framing, and the endpoint.
//!
//! The shim links this crate into every game, so it depends on `std` and
//! OS bindings only (SPEC WS-2).

pub mod endpoint;
mod messages;
pub mod wire;

use std::io::{self, Read, Write};

pub use messages::*;
use wire::{DecodeError, Reader, Wire};

/// The protocol version (`steamworks-abi.md` §3.4). Both sides must
/// agree exactly.
pub const PROTOCOL: u32 = 3;

/// The largest frame either side accepts.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Closed,
    TooLarge(usize),
    Decode(DecodeError),
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("connection closed"),
            Self::TooLarge(n) => {
                write!(
                    f,
                    "frame of {n} bytes exceeds the {MAX_FRAME_LEN}-byte limit"
                )
            }
            Self::Decode(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<DecodeError> for FrameError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Encodes one message as a frame, length included.
pub fn encode(msg: &Message) -> Vec<u8> {
    let mut out = vec![0; 4];
    msg.put(&mut out);
    let len = (out.len() - 4) as u32;
    out[..4].copy_from_slice(&len.to_le_bytes());
    out
}

/// Writes one message.
pub fn write_message(w: &mut impl Write, msg: &Message) -> Result<(), FrameError> {
    let frame = encode(msg);
    if frame.len() - 4 > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(frame.len() - 4));
    }
    w.write_all(&frame)?;
    w.flush()?;
    Ok(())
}

/// Reads one message. A clean end of stream before the first length byte
/// is [`FrameError::Closed`].
pub fn read_message(r: &mut impl Read) -> Result<Message, FrameError> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Err(FrameError::Closed),
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let mut reader = Reader::new(&buf);
    let msg = Message::get(&mut reader)?;
    reader.finish()?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(msg: Message) {
        let frame = encode(&msg);
        let back = read_message(&mut frame.as_slice()).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn every_message_round_trips() {
        round_trip(Message::Hello(Hello {
            protocol: PROTOCOL,
            pid: 42,
            app_id: 480,
            arch: "x86".into(),
            version: "0.1.0".into(),
        }));
        let session = Session {
            active: true,
            logged_on: true,
            steam_id: 76561197960287930,
            persona_name: "Ünïcode".into(),
            persona_state: 1,
            ip_country: "DE".into(),
            server_time: 1_790_000_000,
            universe: 1,
            ui_language: "german".into(),
        };
        let app = App {
            app_id: 480,
            owned: true,
            languages: vec!["english".into(), "german".into()],
            depots: vec![481, 482],
            dlc: vec![Dlc {
                app_id: 110902,
                owned: true,
                installed: false,
                available: true,
                name: "Pieterw test DLC".into(),
            }],
            cloud_quota: 1 << 33,
            cloud_max_files: 500,
            ..Default::default()
        };
        round_trip(Message::Welcome(Welcome {
            session: session.clone(),
            app: app.clone(),
        }));
        round_trip(Message::Refused(Refused {
            reason: "no".into(),
        }));
        round_trip(Message::Request(Request {
            id: 7,
            body: RequestBody::AppState(AppStateRequest { app_id: 10 }),
        }));
        round_trip(Message::Response(Response {
            id: 7,
            body: ResponseBody::AppState(AppState {
                owned: true,
                installed: true,
                install_dir: "/games/x".into(),
                build_id: 3,
                depots: vec![11],
            }),
        }));
        round_trip(Message::Response(Response {
            id: 8,
            body: ResponseBody::Error(ErrorReply {
                message: "x".into(),
            }),
        }));
        let schema = StatsSchema {
            version: 3,
            stats: vec![
                StatDef {
                    id: 1,
                    name: "NumGames".into(),
                    kind: 1,
                    default: 0,
                    achievements: vec![],
                },
                StatDef {
                    id: 2,
                    kind: 4,
                    achievements: vec![AchievementDef {
                        bit: 0,
                        name: "ACH_WIN_ONE_GAME".into(),
                        display_name: "Winner".into(),
                        hidden: true,
                        icon: "a.jpg".into(),
                        progress_stat: "NumGames".into(),
                        progress_max: 10,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
        };
        for body in [
            RequestBody::UserStats(UserStatsRequest { steam_id: 5 }),
            RequestBody::StoreStats(StoreStatsRequest {
                crc: 9,
                values: vec![StatValue { id: 1, value: 2 }],
                reset: true,
            }),
            RequestBody::GlobalAchievements(GlobalAchievementsRequest {}),
            RequestBody::AchievementIcon(AchievementIconRequest {
                file: "a.jpg".into(),
            }),
        ] {
            round_trip(Message::Request(Request { id: 1, body }));
        }
        for body in [
            ResponseBody::UserStats(UserStats {
                result: 1,
                steam_id: 5,
                crc: 9,
                schema,
                values: vec![StatValue { id: 2, value: 1 }],
                unlocks: vec![Unlock {
                    stat: 2,
                    bit: 0,
                    time: 7,
                }],
            }),
            ResponseBody::StoreStats(StoreStats {
                result: 1,
                crc: 10,
                reverted: vec![StatValue { id: 1, value: 0 }],
                out_of_date: false,
            }),
            ResponseBody::GlobalAchievements(GlobalAchievements {
                result: 1,
                achievements: vec![AchievementPercent {
                    name: "ACH_WIN_ONE_GAME".into(),
                    percent: 12.5f32.to_bits(),
                }],
            }),
            ResponseBody::Image(Image {
                result: 1,
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
            }),
        ] {
            round_trip(Message::Response(Response { id: 2, body }));
        }
        round_trip(Message::SessionUpdate(session));
        round_trip(Message::AppUpdate(app));
        round_trip(Message::Report(Report {
            interfaces: vec!["SteamUser023".into()],
            unknown: vec!["SteamFoo001".into()],
            calls: vec![Call {
                interface: "SteamUser023".into(),
                method: "GetVoice".into(),
                count: 3,
            }],
        }));
    }

    #[test]
    fn rejects_bad_input() {
        // Unknown tag.
        let frame = [1u8, 0, 0, 0, 99];
        assert!(matches!(
            read_message(&mut frame.as_slice()),
            Err(FrameError::Decode(_))
        ));
        // Trailing bytes.
        let mut frame = encode(&Message::Refused(Refused { reason: "a".into() }));
        frame.push(0);
        frame[0] += 1;
        assert!(matches!(
            read_message(&mut frame.as_slice()),
            Err(FrameError::Decode(_))
        ));
        // A list that claims more items than there are bytes.
        let mut body = vec![8u8];
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&body);
        assert!(matches!(
            read_message(&mut frame.as_slice()),
            Err(FrameError::Decode(_))
        ));
        // Too large, and closed.
        let big = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes();
        assert!(matches!(
            read_message(&mut big.as_slice()),
            Err(FrameError::TooLarge(_))
        ));
        assert!(matches!(
            read_message(&mut [].as_slice()),
            Err(FrameError::Closed)
        ));
    }
}
