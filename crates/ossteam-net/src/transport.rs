// SPDX-License-Identifier: GPL-2.0-or-later
//! Transports (NET-2) and connection setup.
//!
//! Every transport is a non-blocking socket driven by the connection's
//! I/O thread (ADR 0001). This module also holds the readiness wait used
//! while a connection is being set up, which honours cancellation at least
//! once a second (CONC-2).

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use ossteam_types::CancelToken;
use polling::{Event, Events, Poller};
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use tungstenite::handshake::HandshakeError;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::directory::{CmKind, CmServer};

/// The longest a single Steam message may be on WebSocket (`cm.md` §8).
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// The longest any blocking wait runs before re-checking cancellation.
pub(crate) const CANCEL_SLICE: Duration = Duration::from_secs(1);

/// One CM transport, carrying whole Steam messages.
///
/// All methods are non-blocking. `send` queues when the socket is full;
/// `flush` writes what it can; `recv` returns `Ok(None)` when no complete
/// message is buffered.
pub trait Transport: Send {
    fn send(&mut self, msg: Vec<u8>) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
    fn recv(&mut self) -> io::Result<Option<Vec<u8>>>;
    /// Whether queued output is waiting for the socket to become writable.
    fn wants_write(&self) -> bool;
    /// The socket to register for readiness.
    fn socket(&self) -> &TcpStream;
    /// Starts an orderly close. Best effort; errors are ignored.
    fn close(&mut self);
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("bad CM endpoint {0:?}")]
    BadEndpoint(String),
    #[error("{0:?} transport is not implemented")]
    Unsupported(CmKind),
    #[error("could not resolve {0}: {1}")]
    Resolve(String, io::Error),
    #[error("connect failed: {0}")]
    Io(#[from] io::Error),
    #[error("TLS setup failed: {0}")]
    Tls(#[from] rustls::Error),
    #[error("WebSocket handshake failed: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
}

/// Opens a transport to `server`.
///
/// Only WebSocket is built. The TCP fallback waits for the universe key to
/// be documented (`cm.md` §2.2), so asking for it fails honestly
/// (FAITH-2).
pub fn connect(
    server: &CmServer,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Box<dyn Transport>, ConnectError> {
    match server.kind {
        CmKind::Websockets => Ok(Box::new(WsTransport::connect(
            &server.endpoint,
            Instant::now() + timeout,
            cancel,
        )?)),
        kind @ CmKind::Netfilter => Err(ConnectError::Unsupported(kind)),
    }
}

type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// WebSocket over TLS (`cm.md` §2.1): one binary frame per Steam message.
pub struct WsTransport {
    ws: WebSocket<TlsStream>,
    /// tungstenite holds unflushed frames (or a queued pong).
    unflushed: bool,
}

impl WsTransport {
    fn connect(
        endpoint: &str,
        deadline: Instant,
        cancel: &CancelToken,
    ) -> Result<Self, ConnectError> {
        let (host, port) = endpoint
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
            .filter(|(h, _)| !h.is_empty())
            .ok_or_else(|| ConnectError::BadEndpoint(endpoint.to_owned()))?;
        let name = ServerName::try_from(host.to_owned())
            .map_err(|_| ConnectError::BadEndpoint(endpoint.to_owned()))?;

        // Name resolution uses the system resolver, which cannot be
        // interrupted; it is the one step not bounded by CANCEL_SLICE.
        let addrs: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|e| ConnectError::Resolve(host.to_owned(), e))?
            .collect();
        let mut last = ConnectError::Resolve(
            host.to_owned(),
            io::Error::new(ErrorKind::NotFound, "no addresses"),
        );
        let mut tcp = None;
        for addr in addrs {
            match connect_tcp(addr, deadline, cancel) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e @ (ConnectError::Cancelled | ConnectError::Timeout)) => return Err(e),
                Err(e) => {
                    tracing::debug!(%addr, error = %e, "CM address failed");
                    last = e;
                }
            }
        }
        let tcp = tcp.ok_or(last)?;

        let tls = ClientConnection::new(crate::tls::client_config()?, name)?;
        let stream = StreamOwned::new(tls, tcp);
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let url = format!("wss://{endpoint}/cmsocket/");
        let mut attempt =
            tungstenite::client::client_with_config(url.as_str(), stream, Some(config));
        loop {
            match attempt {
                Ok((ws, _response)) => {
                    return Ok(Self {
                        ws,
                        unflushed: false,
                    });
                }
                Err(HandshakeError::Failure(e)) => return Err(e.into()),
                Err(HandshakeError::Interrupted(mid)) => {
                    let stream = mid.get_ref().get_ref();
                    let writable = stream.conn.wants_write();
                    wait_ready(&stream.sock, true, writable, deadline, cancel)?;
                    attempt = mid.handshake();
                }
            }
        }
    }
}

impl Transport for WsTransport {
    fn send(&mut self, msg: Vec<u8>) -> io::Result<()> {
        match self.ws.send(Message::binary(msg)) {
            Ok(()) => {
                self.unflushed = false;
                Ok(())
            }
            Err(e) => self.would_block(e),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.ws.flush() {
            Ok(()) => {
                self.unflushed = false;
                Ok(())
            }
            Err(e) => self.would_block(e),
        }
    }

    fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            match self.ws.read() {
                Ok(Message::Binary(b)) => return Ok(Some(b.into())),
                // tungstenite queues the pong; the next flush sends it.
                Ok(Message::Ping(_)) => self.unflushed = true,
                Ok(Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Text(t)) => {
                    tracing::warn!(len = t.len(), "ignoring text frame from CM");
                }
                Ok(Message::Close(frame)) => {
                    self.unflushed = true;
                    let why = frame.map_or_else(
                        || "no reason given".to_owned(),
                        |f| format!("{} {}", f.code, f.reason),
                    );
                    return Err(io::Error::new(
                        ErrorKind::ConnectionAborted,
                        format!("server closed: {why}"),
                    ));
                }
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    return Ok(None);
                }
                Err(e) => return Err(into_io(e)),
            }
        }
    }

    fn wants_write(&self) -> bool {
        self.unflushed || self.ws.get_ref().conn.wants_write()
    }

    fn socket(&self) -> &TcpStream {
        &self.ws.get_ref().sock
    }

    fn close(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

impl WsTransport {
    fn would_block(&mut self, e: tungstenite::Error) -> io::Result<()> {
        match e {
            tungstenite::Error::Io(e) if e.kind() == ErrorKind::WouldBlock => {
                self.unflushed = true;
                Ok(())
            }
            e => Err(into_io(e)),
        }
    }
}

fn into_io(e: tungstenite::Error) -> io::Error {
    match e {
        tungstenite::Error::Io(e) => e,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            io::Error::new(ErrorKind::ConnectionAborted, "connection closed")
        }
        e => io::Error::other(e),
    }
}

/// A non-blocking TCP connect, waited on in cancellable slices.
fn connect_tcp(
    addr: SocketAddr,
    deadline: Instant,
    cancel: &CancelToken,
) -> Result<TcpStream, ConnectError> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    socket.set_tcp_nodelay(true)?;
    match socket.connect(&addr.into()) {
        Ok(()) => {}
        // Windows reports a pending connect as WSAEWOULDBLOCK.
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::INPROGRESS.raw_os_error()) => {}
        Err(e) => return Err(e.into()),
    }
    let stream: TcpStream = socket.into();
    loop {
        wait_ready(&stream, false, true, deadline, cancel)?;
        if let Some(e) = stream.take_error()? {
            return Err(e.into());
        }
        // Writable and no error: connected, unless the wait slice merely
        // timed out, which `peer_addr` tells apart.
        match stream.peer_addr() {
            Ok(_) => return Ok(stream),
            Err(e) if e.kind() == ErrorKind::NotConnected => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Waits until `sock` is ready, the deadline passes, or `cancel` fires,
/// for at most [`CANCEL_SLICE`]. Returning `Ok` does not promise readiness;
/// callers retry their non-blocking operation either way.
pub(crate) fn wait_ready(
    sock: &TcpStream,
    readable: bool,
    writable: bool,
    deadline: Instant,
    cancel: &CancelToken,
) -> Result<(), ConnectError> {
    if cancel.is_cancelled() {
        return Err(ConnectError::Cancelled);
    }
    let now = Instant::now();
    if now >= deadline {
        return Err(ConnectError::Timeout);
    }
    let poller = Poller::new()?;
    // SAFETY: `sock` is borrowed for this whole function and is deleted
    // from the poller below, before the borrow ends.
    unsafe { poller.add(sock, Event::new(0, readable, writable))? };
    let mut events = Events::new();
    let waited = poller.wait(&mut events, Some((deadline - now).min(CANCEL_SLICE)));
    poller.delete(sock)?;
    match waited {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::Interrupted => Ok(()),
        Err(e) => Err(e.into()),
    }
}
