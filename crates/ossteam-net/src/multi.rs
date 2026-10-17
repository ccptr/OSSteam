// SPDX-License-Identifier: GPL-2.0-or-later
//! `Multi` messages: several messages in one, optionally gzip-compressed
//! (`docs/spec/cm.md` §3.4).

use std::io::Read;

use ossteam_proto::steam::CMsgMulti;

/// The largest inflated payload accepted (SEC-4).
pub const MAX_INFLATED: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MultiError {
    #[error("unzipped size {0} exceeds the limit")]
    TooLarge(u32),
    #[error("gzip payload is corrupt: {0}")]
    Gzip(String),
    #[error("payload inflated to {actual} bytes, header said {declared}")]
    SizeMismatch { declared: u32, actual: usize },
    #[error("inner message length {len} at offset {at} runs past the payload")]
    Truncated { at: usize, len: u32 },
}

/// The inner messages of a `Multi`, in order. Each is a complete Steam
/// message that may itself be a `Multi`.
pub fn unpack(multi: &CMsgMulti) -> Result<Vec<Vec<u8>>, MultiError> {
    let body = multi.message_body();
    let declared = multi.size_unzipped();
    let inflated;
    let payload: &[u8] = if declared == 0 {
        body
    } else {
        if declared as usize > MAX_INFLATED {
            return Err(MultiError::TooLarge(declared));
        }
        let mut out = Vec::with_capacity(declared as usize);
        // One byte more than declared, so an oversized stream is detected
        // without inflating all of it.
        flate2::read::GzDecoder::new(body)
            .take(u64::from(declared) + 1)
            .read_to_end(&mut out)
            .map_err(|e| MultiError::Gzip(e.to_string()))?;
        if out.len() != declared as usize {
            return Err(MultiError::SizeMismatch {
                declared,
                actual: out.len(),
            });
        }
        inflated = out;
        &inflated
    };

    let mut messages = Vec::new();
    let mut at = 0;
    while at < payload.len() {
        let len_bytes = payload
            .get(at..at + 4)
            .ok_or(MultiError::Truncated { at, len: 0 })?;
        let len = u32::from_le_bytes(len_bytes.try_into().expect("4 bytes"));
        let start = at + 4;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= payload.len())
            .ok_or(MultiError::Truncated { at, len })?;
        messages.push(payload[start..end].to_vec());
        at = end;
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn framed(parts: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(&(p.len() as u32).to_le_bytes());
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn plain_payload() {
        let m = CMsgMulti {
            size_unzipped: None,
            message_body: Some(framed(&[b"abc", b"", b"de"])),
        };
        assert_eq!(
            unpack(&m).unwrap(),
            vec![b"abc".to_vec(), vec![], b"de".to_vec()]
        );
    }

    #[test]
    fn gzip_payload() {
        let raw = framed(&[b"hello", b"world"]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        let m = CMsgMulti {
            size_unzipped: Some(raw.len() as u32),
            message_body: Some(gz.finish().unwrap()),
        };
        assert_eq!(
            unpack(&m).unwrap(),
            vec![b"hello".to_vec(), b"world".to_vec()]
        );

        let wrong = CMsgMulti {
            size_unzipped: Some(raw.len() as u32 - 1),
            ..m
        };
        assert!(matches!(
            unpack(&wrong),
            Err(MultiError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_truncation_and_huge_sizes() {
        let mut body = framed(&[b"abc"]);
        body.truncate(5);
        let m = CMsgMulti {
            size_unzipped: None,
            message_body: Some(body),
        };
        assert_eq!(unpack(&m), Err(MultiError::Truncated { at: 0, len: 3 }));
        let m = CMsgMulti {
            size_unzipped: Some(u32::MAX),
            message_body: Some(vec![]),
        };
        assert_eq!(unpack(&m), Err(MultiError::TooLarge(u32::MAX)));
    }
}
