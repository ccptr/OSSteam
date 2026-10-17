// SPDX-License-Identifier: GPL-2.0-or-later
//! Frame encoding: a little-endian `u32` length, then that many bytes of
//! protobuf-encoded [`Envelope`] (SPEC IPC-4).

use std::io::{self, Read, Write};

use prost::Message;
use zeroize::Zeroizing;

use crate::v1::Envelope;

/// The largest frame either side accepts.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("connection closed")]
    Closed,
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(usize),
    #[error("malformed envelope: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Reads one frame. A clean end of stream before the first length byte is
/// [`FrameError::Closed`]; anything else that ends early is an I/O error.
pub fn read_frame(r: &mut impl Read) -> Result<Envelope, FrameError> {
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
    // Frame buffers are zeroed after use: a login request carries a
    // password (IPC-10, SEC-2).
    let mut buf = Zeroizing::new(vec![0u8; len]);
    r.read_exact(&mut buf)?;
    Ok(Envelope::decode(buf.as_slice())?)
}

/// Writes one frame and flushes it.
pub fn write_frame(w: &mut impl Write, env: &Envelope) -> Result<(), FrameError> {
    let len = env.encoded_len();
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = Zeroizing::new(Vec::with_capacity(4 + len));
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    env.encode(&mut *buf).expect("Vec has room");
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::{Hello, envelope::Body};

    #[test]
    fn round_trip() {
        let env = Envelope {
            id: 7,
            body: Some(Body::Hello(Hello {
                name: "t".into(),
                ..Default::default()
            })),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &env).unwrap();
        assert_eq!(
            u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize,
            buf.len() - 4
        );
        let mut r = buf.as_slice();
        assert_eq!(read_frame(&mut r).unwrap(), env);
        assert!(matches!(read_frame(&mut r), Err(FrameError::Closed)));
    }

    #[test]
    fn rejects_oversized_and_truncated_frames() {
        let big = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes();
        assert!(matches!(
            read_frame(&mut big.as_slice()),
            Err(FrameError::TooLarge(_))
        ));
        let truncated = [5u8, 0, 0, 0, 1];
        assert!(matches!(
            read_frame(&mut truncated.as_slice()),
            Err(FrameError::Io(_))
        ));
        let short_len = [5u8, 0];
        assert!(matches!(
            read_frame(&mut short_len.as_slice()),
            Err(FrameError::Io(_))
        ));
    }
}
