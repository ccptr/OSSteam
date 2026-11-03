// SPDX-License-Identifier: GPL-2.0-or-later
//! The client endpoint: one acceptor thread, and per connection a reader
//! thread and a writer thread (SPEC CONC-1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use interprocess::local_socket::{Listener, Stream, prelude::*};
use ossteam_ipc::frame::{FrameError, read_frame, write_frame};
use ossteam_ipc::v1::{self, Envelope, ErrorCode, Hello, PeerKind, envelope::Body};
use ossteam_ipc::{Endpoint, PROTOCOL_VERSION, RemoteError, endpoint};

use crate::core::{CLIENT_QUEUE, ClientId, Command};

/// A client must send its Hello within this time.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Server {
    stopping: Arc<AtomicBool>,
    endpoint: Endpoint,
    acceptor: thread::JoinHandle<()>,
}

impl Server {
    /// Starts accepting connections. `alive` is cloned into every writer
    /// thread, so the caller can wait for queued frames to be flushed by
    /// waiting for all clones to drop.
    pub fn start(
        listener: Listener,
        endpoint: Endpoint,
        core: Sender<Command>,
        alive: Sender<()>,
    ) -> std::io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopping);
        let acceptor = thread::Builder::new()
            .name("ipc-accept".into())
            .spawn(move || accept_loop(&listener, &flag, &core, &alive))?;
        Ok(Self {
            stopping,
            endpoint,
            acceptor,
        })
    }

    /// Stops the acceptor. A blocking accept cannot be interrupted, so this
    /// sets the flag and then connects once to wake it.
    pub fn stop(self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.endpoint.connect());
        let _ = self.acceptor.join();
    }
}

fn accept_loop(
    listener: &Listener,
    stopping: &AtomicBool,
    core: &Sender<Command>,
    alive: &Sender<()>,
) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    for conn in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                continue;
            }
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let core = core.clone();
        let alive = alive.clone();
        let spawned = thread::Builder::new()
            .name(format!("ipc-client-{id}"))
            .spawn(move || serve(id, stream, &core, alive));
        if let Err(e) = spawned {
            tracing::error!("cannot spawn a client thread: {e}");
        }
    }
}

fn reject(stream: &mut Stream, err: RemoteError) {
    let _ = write_frame(
        stream,
        &Envelope {
            id: 0,
            body: Some(Body::Error(err.into())),
        },
    );
}

fn serve(client: ClientId, mut stream: Stream, core: &Sender<Command>, alive: Sender<()>) {
    match endpoint::peer_is_same_user(&stream) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(client, "rejected a connection from another user");
            return;
        }
        Err(e) => {
            tracing::warn!(client, "cannot read peer credentials, rejecting: {e}");
            return;
        }
    }

    let hello = match handshake(&mut stream) {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(client, "handshake failed: {e}");
            return;
        }
    };

    // The writer thread writes while this one reads: `&Stream` does both.
    let stream = Arc::new(stream);
    let tx = Arc::clone(&stream);
    let (out_tx, out_rx) = bounded::<Envelope>(CLIENT_QUEUE);
    let writer = thread::Builder::new()
        .name(format!("ipc-write-{client}"))
        .spawn(move || {
            let _alive = alive;
            for env in out_rx {
                if write_frame(&mut &*tx, &env).is_err() {
                    break;
                }
            }
        });
    if writer.is_err() {
        tracing::error!(client, "cannot spawn a writer thread");
        return;
    }
    if core
        .send(Command::Connected {
            client,
            hello,
            out: out_tx,
        })
        .is_err()
    {
        return;
    }
    read_loop(client, &stream, core);
    let _ = core.send(Command::Disconnected { client });
}

/// Sets a receive timeout where the transport supports one. Windows named
/// pipes do not, so there a client that never sends its Hello keeps its
/// thread until it disconnects; only the same user can connect (IPC-2).
fn set_recv_timeout(stream: &Stream, timeout: Option<Duration>) -> Result<(), String> {
    match stream.set_recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn handshake(stream: &mut Stream) -> Result<Hello, String> {
    set_recv_timeout(stream, Some(HELLO_TIMEOUT))?;
    let env = read_frame(stream).map_err(|e| e.to_string())?;
    set_recv_timeout(stream, None)?;
    let hello = match env.body {
        Some(Body::Hello(h)) => h,
        _ => {
            reject(
                stream,
                RemoteError::new(ErrorCode::Protocol, "the first frame must be a Hello"),
            );
            return Err("first frame was not a Hello".into());
        }
    };
    let major = hello.protocol.map(|p| p.major);
    if major != Some(PROTOCOL_VERSION.major) {
        let err = RemoteError::new(
            ErrorCode::IncompatibleVersion,
            format!("protocol major version {major:?} is not supported"),
        )
        .with_detail(format!(
            "supported: {}.0 to {}.x",
            PROTOCOL_VERSION.major, PROTOCOL_VERSION.major
        ));
        reject(stream, err);
        return Err(format!("incompatible protocol version {major:?}"));
    }
    let ours = Hello {
        protocol: Some(PROTOCOL_VERSION),
        kind: PeerKind::Daemon as i32,
        name: "ossteamd".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    write_frame(
        stream,
        &Envelope {
            id: 0,
            body: Some(Body::Hello(ours)),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(hello)
}

fn read_loop(client: ClientId, mut rx: &Stream, core: &Sender<Command>) {
    loop {
        let env = match read_frame(&mut rx) {
            Ok(env) => env,
            Err(FrameError::Closed) => return,
            Err(FrameError::Decode(e)) => {
                let message = format!("malformed envelope: {e}");
                let _ = core.send(Command::ProtocolError {
                    client,
                    id: 0,
                    message,
                });
                return;
            }
            Err(e) => {
                tracing::debug!(client, "read failed: {e}");
                return;
            }
        };
        let cmd = match env.body {
            Some(Body::Request(v1::Request { method })) if env.id != 0 => Command::Request {
                client,
                id: env.id,
                method,
            },
            Some(Body::Request(_)) => Command::ProtocolError {
                client,
                id: 0,
                message: "request id must not be 0".into(),
            },
            _ => Command::ProtocolError {
                client,
                id: env.id,
                message: "only requests are valid after the handshake".into(),
            },
        };
        if core.send(cmd).is_err() {
            return;
        }
    }
}
