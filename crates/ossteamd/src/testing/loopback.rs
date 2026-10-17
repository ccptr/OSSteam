// SPDX-License-Identifier: GPL-2.0-or-later
//! A loopback transport between the daemon and the fake CM: whole Steam
//! messages, each behind a 4-byte little-endian length, over a local TCP
//! connection. Test code only (FAITH-4).

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::Duration;

use ossteam_net::Packet;
use ossteam_net::transport::{MAX_MESSAGE, Transport};

/// Opens a connected pair: the daemon's end and the fake CM's end.
pub fn pair() -> io::Result<(ClientEnd, ServerEnd)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    server.set_read_timeout(Some(POLL))?;
    Ok((
        ClientEnd {
            stream: client,
            input: Vec::new(),
            output: Vec::new(),
        },
        ServerEnd {
            stream: server,
            input: Vec::new(),
        },
    ))
}

/// How often the fake CM's reads come back to check whether it should
/// drop the connection.
const POLL: Duration = Duration::from_millis(50);

/// Takes one whole frame off the front of `buf`.
fn take_frame(buf: &mut Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    let Some(len) = buf.get(..4) else {
        return Ok(None);
    };
    let len = u32::from_le_bytes(len.try_into().expect("4 bytes")) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(ErrorKind::InvalidData, "frame too long"));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let frame = buf[4..4 + len].to_vec();
    buf.drain(..4 + len);
    Ok(Some(frame))
}

fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + msg.len());
    out.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    out.extend_from_slice(msg);
    out
}

/// The daemon's end, driven by its connection's I/O thread.
pub struct ClientEnd {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
}

impl Transport for ClientEnd {
    fn send(&mut self, msg: Vec<u8>) -> io::Result<()> {
        self.output.extend_from_slice(&frame(&msg));
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        while !self.output.is_empty() {
            match self.stream.write(&self.output) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if let Some(msg) = take_frame(&mut self.input)? {
                return Ok(Some(msg));
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "the fake CM closed the connection",
                    ));
                }
                Ok(n) => self.input.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    fn wants_write(&self) -> bool {
        !self.output.is_empty()
    }

    fn socket(&self) -> &TcpStream {
        &self.stream
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// The fake CM's end: blocking, with short read timeouts.
pub struct ServerEnd {
    stream: TcpStream,
    input: Vec<u8>,
}

impl ServerEnd {
    pub fn send(&mut self, packet: &Packet) -> io::Result<()> {
        self.stream.write_all(&frame(&packet.encode()))
    }

    /// The next message, or `None` when none arrived within [`POLL`].
    /// Fails once the daemon closes its end.
    pub fn recv(&mut self) -> io::Result<Option<Packet>> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if let Some(msg) = take_frame(&mut self.input)? {
                return Packet::parse(&msg)
                    .map(Some)
                    .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()));
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.input.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    pub fn close(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_across_reads_are_joined() {
        let mut buf = frame(b"hello");
        buf.extend_from_slice(&frame(b"")[..2]);
        assert_eq!(
            take_frame(&mut buf).unwrap().as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(take_frame(&mut buf).unwrap(), None);
        buf.extend_from_slice(&frame(b"")[2..]);
        assert_eq!(take_frame(&mut buf).unwrap().as_deref(), Some(&b""[..]));
    }
}
