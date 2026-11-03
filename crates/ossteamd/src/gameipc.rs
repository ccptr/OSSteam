// SPDX-License-Identifier: GPL-2.0-or-later
//! The game endpoint (`steamworks-abi.md` §3): one acceptor thread, and
//! per connection a reader thread and a writer thread, as for clients
//! (`server.rs`). The core answers what the shim asks.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use interprocess::local_socket::{Listener, Stream, prelude::*};
use ossteam_gameipc::{FrameError, Message, PROTOCOL, Refused, read_message, write_message};
use ossteam_ipc::{Endpoint, endpoint};

use crate::core::{CLIENT_QUEUE, Command};

/// A shim must send its Hello within this time.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub type ConnId = u64;

/// From a game connection's threads to the core.
pub enum Event {
    /// The handshake passed; the core answers with `Welcome`.
    Connected {
        conn: ConnId,
        hello: ossteam_gameipc::Hello,
        out: Sender<Message>,
    },
    /// A `Request` or a `Report`.
    Message {
        conn: ConnId,
        msg: Box<Message>,
    },
    Disconnected {
        conn: ConnId,
    },
}

/// The game endpoint for `addr`, as an `ossteam_ipc` endpoint so that it
/// is created and secured as the client endpoint is (`ipc.md` §1).
pub fn endpoint(client: &Endpoint, addr: &ossteam_gameipc::endpoint::Address) -> Endpoint {
    Endpoint {
        #[cfg(unix)]
        dir: addr.dir.clone(),
        address: addr.name.clone(),
        lock_file: client.lock_file.clone(),
    }
}

/// Linux: the listener on the abstract socket that games in a container
/// reach (`compat-tools.md` §9).
pub struct Abstract {
    listener: Listener,
    name: String,
}

/// Listens on the abstract socket `name`. It fails when the name is
/// taken, by an earlier daemon or by another user.
#[cfg(target_os = "linux")]
pub fn listen_abstract(name: &str) -> io::Result<Abstract> {
    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
    let listener = ListenerOptions::new()
        .name(name.to_ns_name::<GenericNamespaced>()?)
        .create_sync()?;
    Ok(Abstract {
        listener,
        name: name.to_owned(),
    })
}

#[cfg(target_os = "linux")]
fn connect_abstract(name: &str) -> io::Result<Stream> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    Stream::connect(name.to_ns_name::<GenericNamespaced>()?)
}

#[cfg(not(target_os = "linux"))]
fn connect_abstract(_: &str) -> io::Result<Stream> {
    Err(io::ErrorKind::Unsupported.into())
}

pub struct Server {
    stopping: Arc<AtomicBool>,
    endpoint: Endpoint,
    abstract_name: Option<String>,
    acceptors: Vec<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(
        listener: Listener,
        endpoint: Endpoint,
        abstract_listener: Option<Abstract>,
        core: Sender<Command>,
    ) -> io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let abstract_name = abstract_listener.as_ref().map(|a| a.name.clone());
        let mut acceptors = Vec::new();
        let listeners = std::iter::once(("game-accept", listener))
            .chain(abstract_listener.map(|a| ("game-accept-abstract", a.listener)));
        for (name, listener) in listeners {
            let flag = Arc::clone(&stopping);
            let core = core.clone();
            acceptors.push(
                thread::Builder::new()
                    .name(name.into())
                    .spawn(move || accept_loop(&listener, &flag, &core))?,
            );
        }
        Ok(Self {
            stopping,
            endpoint,
            abstract_name,
            acceptors,
        })
    }

    /// Stops the acceptors, as `server::Server::stop` does. Connections
    /// end when the core drops their queues.
    pub fn stop(self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.endpoint.connect());
        if let Some(name) = &self.abstract_name {
            drop(connect_abstract(name));
        }
        for a in self.acceptors {
            let _ = a.join();
        }
    }
}

fn accept_loop(listener: &Listener, stopping: &AtomicBool, core: &Sender<Command>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    for conn in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("game endpoint: accept failed: {e}");
                continue;
            }
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let core = core.clone();
        let spawned = thread::Builder::new()
            .name(format!("game-conn-{id}"))
            .spawn(move || serve(id, stream, &core));
        if let Err(e) = spawned {
            tracing::error!("cannot spawn a game connection thread: {e}");
        }
    }
}

fn refuse(stream: &mut Stream, conn: ConnId, reason: String) {
    tracing::info!(conn, "refused a game connection: {reason}");
    let _ = write_message(stream, &Message::Refused(Refused { reason }));
}

/// Sets a receive timeout where the transport has one (see
/// `server::set_recv_timeout`).
fn set_recv_timeout(stream: &Stream, timeout: Option<Duration>) -> io::Result<()> {
    match stream.set_recv_timeout(timeout) {
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Ok(()),
        r => r,
    }
}

fn serve(conn: ConnId, mut stream: Stream, core: &Sender<Command>) {
    match endpoint::peer_is_same_user(&stream) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(conn, "rejected a game connection from another user");
            return;
        }
        Err(e) => {
            tracing::warn!(
                conn,
                "cannot read a game peer's credentials, rejecting: {e}"
            );
            return;
        }
    }
    // A `pid_t` on Unix, a `u32` on Windows.
    #[allow(clippy::useless_conversion)]
    let peer_pid = stream
        .peer_creds()
        .ok()
        .and_then(|c| c.pid())
        .and_then(|p| u32::try_from(p).ok());

    if set_recv_timeout(&stream, Some(HELLO_TIMEOUT)).is_err() {
        return;
    }
    let hello = match read_message(&mut stream) {
        Ok(Message::Hello(h)) => h,
        Ok(_) => return refuse(&mut stream, conn, "the first message must be Hello".into()),
        Err(e) => {
            tracing::debug!(conn, "game handshake failed: {e}");
            return;
        }
    };
    if set_recv_timeout(&stream, None).is_err() {
        return;
    }
    if hello.protocol != PROTOCOL {
        return refuse(
            &mut stream,
            conn,
            format!(
                "the game's ossteam-steamclient speaks game IPC protocol {}, this daemon {PROTOCOL}; restart the game",
                hello.protocol
            ),
        );
    }
    if let Some(pid) = peer_pid
        && pid != hello.pid
    {
        return refuse(
            &mut stream,
            conn,
            format!("Hello names pid {}, but the peer is pid {pid}", hello.pid),
        );
    }

    // The writer thread writes while this one reads: `&Stream` does both.
    let stream = Arc::new(stream);
    let tx = Arc::clone(&stream);
    let (out_tx, out_rx) = bounded::<Message>(CLIENT_QUEUE);
    let writer = thread::Builder::new()
        .name(format!("game-write-{conn}"))
        .spawn(move || {
            for msg in out_rx {
                if write_message(&mut &*tx, &msg).is_err() {
                    break;
                }
            }
        });
    if writer.is_err() {
        tracing::error!(conn, "cannot spawn a game writer thread");
        return;
    }
    let connected = Event::Connected {
        conn,
        hello,
        out: out_tx,
    };
    if core.send(Command::Shim(connected)).is_err() {
        return;
    }
    read_loop(conn, &stream, core);
    let _ = core.send(Command::Shim(Event::Disconnected { conn }));
}

fn read_loop(conn: ConnId, mut rx: &Stream, core: &Sender<Command>) {
    loop {
        let msg = match read_message(&mut rx) {
            Ok(m @ (Message::Request(_) | Message::Report(_))) => m,
            Ok(other) => {
                tracing::warn!(conn, "game connection sent {other:?}; closing it");
                return;
            }
            Err(FrameError::Closed) => return,
            Err(e) => {
                tracing::warn!(conn, "game connection: {e}; closing it");
                return;
            }
        };
        if core
            .send(Command::Shim(Event::Message {
                conn,
                msg: Box::new(msg),
            }))
            .is_err()
        {
            return;
        }
    }
}
