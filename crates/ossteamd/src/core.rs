// SPDX-License-Identifier: GPL-2.0-or-later
//! The core thread: the single owner of the daemon's mutable state
//! (SPEC CONC-1). Everything else talks to it through [`Command`]s.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{
    self, Envelope, ErrorCode, Event, Hello, LoginPromptKind, Topic, envelope::Body, event,
    request, response,
};
use ossteam_pics::Cache;
use ossteam_types::{Secret, SteamId};

use crate::config::{self, Config};
use crate::db::{Db, PersonaState};
use crate::session::{self, Cmd, ReplyTo, Report};
use crate::{library, pics};

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
    /// From the session worker.
    Session(Report),
    /// From the PICS worker.
    Pics(pics::Update),
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

/// A request's outcome. `None` means the session worker replies later.
type Outcome = Option<Result<response::Result, RemoteError>>;

pub struct Core {
    started: Instant,
    clients: HashMap<ClientId, ClientState>,
    db: Db,
    config: Config,
    session: session::Handle,
    /// The session as the worker last reported it.
    session_state: session::State,
    /// The client whose login is in progress. The login is cancelled when
    /// that client disconnects.
    login_owner: Option<ClientId>,
    pics: pics::Handle,
    /// Read for `GetApp`; the PICS worker writes it.
    pics_cache: Cache,
    /// The library as the PICS worker last reported it.
    library: Option<Arc<pics::Snapshot>>,
}

impl Core {
    pub fn new(
        db: Db,
        config: Config,
        session: session::Handle,
        pics: pics::Handle,
        pics_cache: Cache,
    ) -> Self {
        Self {
            started: Instant::now(),
            clients: HashMap::new(),
            db,
            config,
            session,
            session_state: session::State::default(),
            login_owner: None,
            pics,
            pics_cache,
            library: None,
        }
    }

    /// Runs until a shutdown is requested or every command sender is gone.
    /// On return the session has logged off and every connection's queue
    /// has been dropped, so their writer threads finish sending and exit.
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
                    if self.login_owner == Some(client) {
                        tracing::info!(
                            client,
                            "the client that started the login left; cancelling it"
                        );
                        self.login_owner = None;
                        self.session.cancel_login(None);
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
                    if self.handle(ReplyTo { client, id }, method) {
                        break Exit::Requested;
                    }
                }
                Command::Terminate => break Exit::Signal,
                Command::Session(report) => self.on_session(report),
                Command::Pics(update) => self.on_library(update),
            }
        };
        // Log off before telling clients the daemon is going (SPEC D-5).
        // Reports sent meanwhile are not needed any more. With the
        // connection closed, PICS requests in flight fail at once.
        self.session.shutdown();
        self.pics.shutdown();
        self.broadcast(
            Topic::Daemon,
            event::Kind::DaemonStopping(v1::DaemonStopping {}),
        );
        self.clients.clear();
        exit
    }

    /// Handles one request. Returns true when the daemon should stop.
    fn handle(&mut self, to: ReplyTo, method: Option<request::Method>) -> bool {
        let Some(method) = method else {
            let err = RemoteError::new(
                ErrorCode::Unsupported,
                "this daemon does not implement that method",
            );
            self.send(to.client, to.id, Body::Error(err.into()));
            return false;
        };
        let mut stop = false;
        let outcome: Outcome = match method {
            request::Method::GetStatus(_) => Some(Ok(response::Result::GetStatus(self.status()))),
            request::Method::GetVersion(_) => {
                Some(Ok(response::Result::GetVersion(v1::GetVersionResponse {
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    protocol: Some(ossteam_ipc::PROTOCOL_VERSION),
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                })))
            }
            request::Method::Shutdown(req) => {
                // No game can be running yet: launching arrives in M1c. When
                // it does, a non-forced shutdown with running games returns
                // Conflict here (SPEC D-5).
                tracing::info!(client = to.client, force = req.force, "shutdown requested");
                stop = true;
                Some(Ok(response::Result::Shutdown(v1::ShutdownResponse {})))
            }
            request::Method::Subscribe(req) => Some(
                self.change_topics(to.client, &req.topics, true)
                    .map(|topics| response::Result::Subscribe(v1::SubscribeResponse { topics })),
            ),
            request::Method::Unsubscribe(req) => Some(
                self.change_topics(to.client, &req.topics, false)
                    .map(|topics| {
                        response::Result::Unsubscribe(v1::UnsubscribeResponse { topics })
                    }),
            ),
            request::Method::LoginWithCredentials(mut req) => {
                // Moved, not copied, into a type that zeroes it (IPC-10).
                let password = Secret::new(std::mem::take(&mut req.password));
                let username = req.username.trim().to_owned();
                if let Some(busy) = self.login_busy() {
                    Some(Err(busy))
                } else if username.is_empty() || password.is_empty() {
                    Some(Err(invalid("a username and a password are required")))
                } else {
                    self.login_owner = Some(to.client);
                    self.session.send(Cmd::LoginWithCredentials {
                        reply: to,
                        username,
                        password,
                        remember: req.remember,
                    });
                    None
                }
            }
            request::Method::LoginWithQr(req) => {
                if let Some(busy) = self.login_busy() {
                    Some(Err(busy))
                } else {
                    self.login_owner = Some(to.client);
                    self.session.send(Cmd::LoginWithQr {
                        reply: to,
                        remember: req.remember,
                    });
                    None
                }
            }
            request::Method::SubmitGuardCode(mut req) => {
                let code = Secret::new(std::mem::take(&mut req.code));
                let code = Secret::new(code.expose().trim().to_owned());
                match LoginPromptKind::try_from(req.kind) {
                    _ if code.is_empty() => Some(Err(invalid("the code is empty"))),
                    Ok(kind) => {
                        self.session.send(Cmd::SubmitGuardCode {
                            reply: to,
                            code,
                            kind,
                        });
                        None
                    }
                    Err(_) => Some(Err(invalid(format!("unknown prompt kind {}", req.kind)))),
                }
            }
            request::Method::CancelLogin(_) => {
                self.session.cancel_login(Some(to));
                None
            }
            request::Method::Logout(_) => {
                self.session.send(Cmd::Logout { reply: to });
                None
            }
            request::Method::ListAccounts(_) => Some(self.list_accounts()),
            request::Method::SwitchAccount(req) => match self.db.account(SteamId(req.steam_id)) {
                Ok(Some(account)) => {
                    self.session.send(Cmd::SwitchAccount { reply: to, account });
                    None
                }
                Ok(None) => Some(Err(RemoteError::new(
                    ErrorCode::NotFound,
                    format!("no remembered account has Steam id {}", req.steam_id),
                ))),
                Err(e) => Some(Err(internal(&e))),
            },
            request::Method::SetPersonaState(req) => Some(self.set_persona_state(req.state)),
            request::Method::ListApps(req) => Some(
                self.snapshot()
                    .map(|s| response::Result::ListApps(library::list_apps(&s, &req))),
            ),
            request::Method::GetApp(req) => Some(self.snapshot().and_then(|s| {
                library::get_app(&s, &self.pics_cache, req.app_id).map(response::Result::GetApp)
            })),
            request::Method::GetSettings(_) => {
                Some(Ok(response::Result::GetSettings(v1::GetSettingsResponse {
                    settings: self.config.all(),
                })))
            }
            request::Method::SetSetting(req) => Some(self.set_setting(&req)),
        };
        if let Some(result) = outcome {
            self.reply(to, result);
        }
        stop
    }

    /// One login runs at a time. Refusing here keeps `login_owner` naming
    /// the client whose login is running.
    fn login_busy(&self) -> Option<RemoteError> {
        self.login_owner
            .map(|_| RemoteError::new(ErrorCode::Conflict, "a login is already in progress"))
    }

    fn reply(&mut self, to: ReplyTo, result: Result<response::Result, RemoteError>) {
        let body = match result {
            Ok(r) => Body::Response(v1::Response { result: Some(r) }),
            Err(e) => Body::Error(e.into()),
        };
        self.send(to.client, to.id, body);
    }

    fn on_session(&mut self, report: Report) {
        let stored = match report {
            Report::Reply { to, result } => {
                self.reply(to, result);
                Ok(())
            }
            Report::State(state) => {
                if state != self.session_state {
                    tracing::debug!(?state, "session changed");
                    self.session_state = state;
                    self.broadcast_session();
                }
                Ok(())
            }
            Report::LoggedOn { steam_id } => self.db.persona_state(steam_id).map(|state| {
                self.session.send(Cmd::SetPersonaState { steam_id, state });
            }),
            Report::Prompt(p) => {
                self.broadcast(Topic::Session, event::Kind::LoginPrompt(p));
                Ok(())
            }
            Report::LoginEnded(ev) => {
                self.login_owner = None;
                if let Some(ev) = ev {
                    self.broadcast(Topic::Session, event::Kind::LoginEnded(ev));
                }
                Ok(())
            }
            Report::Remembered {
                steam_id,
                account_name,
            } => self.db.remember(steam_id, &account_name),
            Report::Forgotten { steam_id } => self.db.forget(steam_id),
            Report::Active(steam_id) => self.db.set_active_account(steam_id),
            Report::CellId(cell) => self.db.set_cell_id(cell),
            Report::PersonaName { steam_id, name } => self.db.set_persona_name(steam_id, &name),
        };
        if let Err(e) = stored {
            tracing::error!("updating the state database failed: {e:#}");
        }
    }

    fn on_library(&mut self, update: pics::Update) {
        let old = self.library.take();
        let mut changed = library::changed_apps(old.as_deref(), update.snapshot.as_deref());
        changed.extend(update.refreshed);
        let complete = update.snapshot.as_ref().is_some_and(|s| s.complete);
        let was_complete = old.as_ref().is_some_and(|s| s.complete);
        self.library = update.snapshot;
        if !changed.is_empty() || complete != was_complete {
            self.broadcast(
                Topic::Library,
                event::Kind::AppChanged(v1::AppChanged {
                    app_ids: changed.into_iter().map(|a| a.0).collect(),
                    complete,
                }),
            );
        }
    }

    /// The active account's library. Until the PICS worker has reported
    /// one, it is empty and incomplete.
    fn snapshot(&self) -> Result<Arc<pics::Snapshot>, RemoteError> {
        let Some(account) = &self.session_state.account else {
            return Err(RemoteError::new(
                ErrorCode::NotLoggedIn,
                "no account is logged in",
            ));
        };
        let steam_id = SteamId(account.steam_id);
        Ok(match &self.library {
            Some(s) if s.steam_id == steam_id => s.clone(),
            _ => Arc::new(pics::Snapshot {
                steam_id,
                library: ossteam_pics::Library::default(),
                complete: false,
            }),
        })
    }

    fn set_persona_state(&mut self, raw: i32) -> Result<response::Result, RemoteError> {
        let state = match v1::PersonaState::try_from(raw) {
            Ok(v1::PersonaState::Online) => PersonaState::Online,
            Ok(v1::PersonaState::Invisible) => PersonaState::Invisible,
            Ok(v1::PersonaState::Offline) => PersonaState::Offline,
            _ => return Err(invalid(format!("unknown persona state {raw}"))),
        };
        let Some(account) = &self.session_state.account else {
            return Err(RemoteError::new(
                ErrorCode::NotLoggedIn,
                "no account is logged in",
            ));
        };
        let steam_id = SteamId(account.steam_id);
        self.db
            .set_persona_state(steam_id, state)
            .map_err(|e| internal(&e))?;
        // Applied now if logged on; otherwise at the next logon.
        self.session.send(Cmd::SetPersonaState { steam_id, state });
        self.broadcast_session();
        Ok(response::Result::SetPersonaState(
            v1::SetPersonaStateResponse {},
        ))
    }

    fn set_setting(
        &mut self,
        req: &v1::SetSettingRequest,
    ) -> Result<response::Result, RemoteError> {
        let key = req.key.trim();
        let Some(def) = config::def(key) else {
            return Err(RemoteError::new(
                ErrorCode::NotFound,
                format!("there is no setting {key:?}"),
            ));
        };
        let value = if req.reset {
            None
        } else {
            Some(def.parse(&req.value).map_err(invalid)?)
        };
        let stored = value.map(|v| v.to_string());
        self.db
            .set_setting_override(def.key, stored.as_deref())
            .map_err(|e| internal(&e))?;
        let before = self.config.proto(def);
        self.config.set_override(def, value);
        let after = self.config.proto(def);
        tracing::info!(key = def.key, value = %after.value, reset = req.reset, "setting changed");
        self.apply_settings();
        if after != before {
            self.broadcast(
                Topic::Daemon,
                event::Kind::SettingChanged(v1::SettingChanged {
                    setting: Some(after.clone()),
                }),
            );
        }
        Ok(response::Result::SetSetting(v1::SetSettingResponse {
            setting: Some(after),
        }))
    }

    /// Hands the settings in effect to the subsystems that use them.
    fn apply_settings(&self) {
        self.session
            .send(Cmd::SetFileFallback(self.config.file_fallback()));
    }

    /// The active account's persona state, for clients.
    fn persona_state(&self) -> v1::PersonaState {
        let Some(account) = &self.session_state.account else {
            return v1::PersonaState::Unspecified;
        };
        match self.db.persona_state(SteamId(account.steam_id)) {
            Ok(PersonaState::Online) => v1::PersonaState::Online,
            Ok(PersonaState::Invisible) => v1::PersonaState::Invisible,
            Ok(PersonaState::Offline) => v1::PersonaState::Offline,
            Err(e) => {
                tracing::error!("reading the persona state failed: {e:#}");
                v1::PersonaState::Unspecified
            }
        }
    }

    fn broadcast_session(&mut self) {
        let mut ev = self.session_state.event();
        ev.persona_state = self.persona_state() as i32;
        self.broadcast(Topic::Session, event::Kind::SessionChanged(ev));
    }

    fn status(&self) -> v1::GetStatusResponse {
        let s = &self.session_state;
        v1::GetStatusResponse {
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_secs: self.started.elapsed().as_secs(),
            connection: s.connection as i32,
            account: s.account.clone(),
            connected_clients: self.clients.len() as u32,
            active_jobs: 0,
            running_games: 0,
            logged_on: s.logged_on,
            session_reason: s.reason.clone(),
            persona_state: self.persona_state() as i32,
        }
    }

    fn list_accounts(&self) -> Result<response::Result, RemoteError> {
        let accounts = self.db.accounts().map_err(|e| internal(&e))?;
        Ok(response::Result::ListAccounts(v1::ListAccountsResponse {
            accounts: accounts
                .into_iter()
                .map(|a| v1::Account {
                    steam_id: a.steam_id.0,
                    account_name: a.account_name,
                    persona_name: a.persona_name,
                })
                .collect(),
            active: self
                .session_state
                .account
                .as_ref()
                .map_or(0, |a| a.steam_id),
        }))
    }

    fn change_topics(
        &mut self,
        client: ClientId,
        topics: &[i32],
        add: bool,
    ) -> Result<Vec<i32>, RemoteError> {
        for &t in topics {
            if !matches!(Topic::try_from(t), Ok(topic) if topic != Topic::Unspecified) {
                return Err(invalid(format!("unknown topic {t}")));
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

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::InvalidArgument, message)
}

fn internal(e: &anyhow::Error) -> RemoteError {
    tracing::error!("{e:#}");
    RemoteError::new(ErrorCode::Internal, format!("{e:#}"))
}
