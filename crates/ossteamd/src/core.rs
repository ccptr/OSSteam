// SPDX-License-Identifier: GPL-2.0-or-later
//! The core thread: the single owner of the daemon's mutable state
//! (SPEC CONC-1). Everything else talks to it through [`Command`]s.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{
    self, ConnectionState, Envelope, ErrorCode, Event, Hello, Topic, envelope::Body, event,
    request, response,
};

pub type ClientId = u64;

/// Outgoing frames for one connection are queued here; a full queue means
/// the client stopped reading and it is dropped.
pub const CLIENT_QUEUE: usize = 1024;

pub enum Command {
    Connected {
        client: ClientId,
        hello: Hello,
        out: Sender<Envelope>,
    },
    Disconnected {
        client: ClientId,
    },
    Request {
        client: ClientId,
        id: u64,
        method: Option<request::Method>,
    },
    /// The connection sent something that is not valid at this point.
    ProtocolError {
        client: ClientId,
        id: u64,
        message: String,
    },
    /// A signal or console event asked the daemon to stop.
    Terminate,
}

/// Why the core loop ended.
#[derive(Debug)]
pub enum Exit {
    Requested,
    Signal,
}

struct ClientState {
    hello: Hello,
    out: Sender<Envelope>,
    topics: BTreeSet<i32>,
}

pub struct Core {
    started: Instant,
    clients: HashMap<ClientId, ClientState>,
}

impl Core {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            clients: HashMap::new(),
        }
    }

    /// Runs until a shutdown is requested or every command sender is gone.
    /// On return every connection's queue has been dropped, so their
    /// writer threads finish sending and exit.
    pub fn run(mut self, commands: Receiver<Command>) -> Exit {
        let exit = loop {
            let Ok(cmd) = commands.recv() else {
                break Exit::Signal;
            };
            match cmd {
                Command::Connected { client, hello, out } => {
                    tracing::info!(client, name = %hello.name, version = %hello.version, kind = hello.kind, "client connected");
                    self.clients.insert(
                        client,
                        ClientState {
                            hello,
                            out,
                            topics: BTreeSet::new(),
                        },
                    );
                }
                Command::Disconnected { client } => {
                    if let Some(c) = self.clients.remove(&client) {
                        tracing::info!(client, name = %c.hello.name, "client disconnected");
                    }
                }
                Command::ProtocolError {
                    client,
                    id,
                    message,
                } => {
                    tracing::warn!(client, "protocol error: {message}");
                    self.send(
                        client,
                        id,
                        Body::Error(RemoteError::new(ErrorCode::Protocol, message).into()),
                    );
                }
                Command::Request { client, id, method } => {
                    if self.handle(client, id, method) {
                        break Exit::Requested;
                    }
                }
                Command::Terminate => break Exit::Signal,
            }
        };
        self.broadcast(
            Topic::Daemon,
            event::Kind::DaemonStopping(v1::DaemonStopping {}),
        );
        self.clients.clear();
        exit
    }

    /// Handles one request. Returns true when the daemon should stop.
    fn handle(&mut self, client: ClientId, id: u64, method: Option<request::Method>) -> bool {
        let Some(method) = method else {
            let err = RemoteError::new(
                ErrorCode::Unsupported,
                "this daemon does not implement that method",
            );
            self.send(client, id, Body::Error(err.into()));
            return false;
        };
        let mut stop = false;
        let result = match method {
            request::Method::GetStatus(_) => Ok(response::Result::GetStatus(self.status())),
            request::Method::GetVersion(_) => {
                Ok(response::Result::GetVersion(v1::GetVersionResponse {
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    protocol: Some(ossteam_ipc::PROTOCOL_VERSION),
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                }))
            }
            request::Method::Shutdown(req) => {
                // No game can be running yet: launching arrives in M1c. When
                // it does, a non-forced shutdown with running games returns
                // Conflict here (SPEC D-5).
                tracing::info!(client, force = req.force, "shutdown requested");
                stop = true;
                Ok(response::Result::Shutdown(v1::ShutdownResponse {}))
            }
            request::Method::Subscribe(req) => self
                .change_topics(client, &req.topics, true)
                .map(|topics| response::Result::Subscribe(v1::SubscribeResponse { topics })),
            request::Method::Unsubscribe(req) => self
                .change_topics(client, &req.topics, false)
                .map(|topics| response::Result::Unsubscribe(v1::UnsubscribeResponse { topics })),
        };
        let body = match result {
            Ok(r) => Body::Response(v1::Response { result: Some(r) }),
            Err(e) => Body::Error(e.into()),
        };
        self.send(client, id, body);
        stop
    }

    fn status(&self) -> v1::GetStatusResponse {
        v1::GetStatusResponse {
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_secs: self.started.elapsed().as_secs(),
            // The daemon has no Steam connection until M1a adds one.
            connection: ConnectionState::Disconnected as i32,
            account: None,
            connected_clients: self.clients.len() as u32,
            active_jobs: 0,
            running_games: 0,
        }
    }

    fn change_topics(
        &mut self,
        client: ClientId,
        topics: &[i32],
        add: bool,
    ) -> Result<Vec<i32>, RemoteError> {
        for &t in topics {
            if !matches!(Topic::try_from(t), Ok(topic) if topic != Topic::Unspecified) {
                return Err(RemoteError::new(
                    ErrorCode::InvalidArgument,
                    format!("unknown topic {t}"),
                ));
            }
        }
        let Some(c) = self.clients.get_mut(&client) else {
            return Ok(Vec::new());
        };
        for &t in topics {
            if add {
                c.topics.insert(t);
            } else {
                c.topics.remove(&t);
            }
        }
        Ok(c.topics.iter().copied().collect())
    }

    fn send(&mut self, client: ClientId, id: u64, body: Body) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        match c.out.try_send(Envelope {
            id,
            body: Some(body),
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!(client, "client is not reading; dropping it");
                self.clients.remove(&client);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.clients.remove(&client);
            }
        }
    }

    fn broadcast(&mut self, topic: Topic, kind: event::Kind) {
        let targets: Vec<ClientId> = self
            .clients
            .iter()
            .filter(|(_, c)| c.topics.contains(&(topic as i32)))
            .map(|(&id, _)| id)
            .collect();
        for client in targets {
            self.send(
                client,
                0,
                Body::Event(Event {
                    kind: Some(kind.clone()),
                }),
            );
        }
    }
}
