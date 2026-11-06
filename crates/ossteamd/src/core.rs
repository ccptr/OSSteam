// SPDX-License-Identifier: GPL-2.0-or-later
//! The core thread: the single owner of the daemon's mutable state
//! (SPEC CONC-1). Everything else talks to it through [`Command`]s.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{
    self, Envelope, ErrorCode, Event, Hello, LoginPromptKind, Topic, envelope::Body, event,
    request, response,
};
use ossteam_pics::{AssetKind, Cache};
use ossteam_types::{Secret, SteamId};

use crate::config::{self, Config};
use crate::db::{Db, PersonaState};
use crate::session::{self, Cmd, ReplyTo, Report};
use crate::tickets as auth;
use crate::{assets, content, launch, library, pics, stats};

mod compat;
mod games;
mod gc;
mod http;
mod jobs;
mod networking;
mod scripts;
mod shim;
mod takeover;
mod tickets;
mod update_hold;

pub use games::app_launch_options;

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
    /// From the image downloader.
    Asset(assets::Done),
    /// From a content job.
    Content(content::Report),
    /// From a game's supervisor thread.
    Game(launch::Exited),
    /// From a game endpoint connection.
    Shim(crate::gameipc::Event),
    /// From the stats worker: the answer to a game's request.
    Stats(stats::Done),
    /// About game tickets (`isteamuser.md` §2).
    Tickets(auth::Event),
    /// A game's connections had their time to close (`steamworks-abi.md`
    /// §3.5).
    GameExitWait(ossteam_types::AppId),
    /// A networking certificate request ended
    /// (`isteamnetworkingsocketsserialized.md` §3).
    Networking(networking::CertReply),
    /// An update hold ran out (`isteamappdisableupdate.md` §3).
    UpdateHoldOver(ossteam_types::AppId, Instant),
    /// A game's HTTP request has news (`isteamhttp.md` §8).
    Http(http::HttpEvent),
    /// From a launch's install scripts (`install-scripts.md` §3.1).
    Scripts(scripts::Report),
    /// Linux: `~/.steam` changed, or the take-over's interval passed
    /// (`steamworks-abi.md` §1.1.5).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    SteamDir,
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
    assets: assets::Handle,
    /// `<cache>/assets`.
    assets_dir: PathBuf,
    /// Images being downloaded, and the requests waiting for each.
    fetching: HashMap<PathBuf, Vec<ReplyTo>>,
    /// URLs the CDN answered 404 for since the daemon started.
    missing_assets: HashSet<String>,
    /// Folders, installs and jobs.
    content: jobs::Content,
    /// The running jobs' threads.
    content_jobs: content::Handle,
    /// Running games and the discovery entries.
    games: games::Games,
    /// Game endpoint connections and their reports.
    shims: shim::Shims,
    /// Games' stats requests.
    stats: stats::Handle,
    /// Games' tickets and auth sessions.
    tickets: auth::Tickets,
    /// Games' networking certificates and relay tickets.
    networking: networking::Networking,
    /// Apps whose automatic updates wait, until when.
    update_holds: HashMap<ossteam_types::AppId, Instant>,
    /// Games' HTTP requests and cookie containers.
    http: http::Http,
}

/// What a `Core` is made of.
pub struct Parts {
    pub db: Db,
    pub config: Config,
    pub session: session::Handle,
    pub pics: pics::Handle,
    pub pics_cache: Cache,
    pub assets: assets::Handle,
    pub assets_dir: PathBuf,
    /// `<cache>/http`, games' HTTP cache (`isteamhttp.md` §3.1).
    pub http_cache: PathBuf,
    pub stats: stats::Handle,
    pub content_jobs: content::Handle,
    /// `<data>/library`.
    pub default_folder: PathBuf,
    pub launch: launch::Settings,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    /// `<config>`, which holds the take-over's record.
    pub config_dir: PathBuf,
    /// The core's own command channel, for game supervisors.
    pub commands: Sender<Command>,
}

impl Core {
    pub fn new(p: Parts) -> Self {
        Self {
            started: Instant::now(),
            clients: HashMap::new(),
            db: p.db,
            config: p.config,
            session: p.session,
            session_state: session::State::default(),
            login_owner: None,
            pics: p.pics,
            pics_cache: p.pics_cache,
            library: None,
            assets: p.assets,
            assets_dir: p.assets_dir,
            fetching: HashMap::new(),
            missing_assets: HashSet::new(),
            content: jobs::Content {
                default_folder: p.default_folder,
                ..Default::default()
            },
            content_jobs: p.content_jobs,
            games: games::Games::new(
                p.launch,
                &p.data_dir,
                &p.state_dir,
                &p.config_dir,
                p.commands,
            ),
            shims: shim::Shims::new(&p.data_dir),
            stats: p.stats,
            tickets: auth::Tickets::default(),
            networking: networking::Networking::default(),
            update_holds: HashMap::new(),
            http: http::Http::new(p.http_cache),
        }
        .loaded()
    }

    fn loaded(mut self) -> Self {
        self.take_over_at_start();
        self.clean_discovery();
        self.check_isolation();
        if let Err(e) = self.load_content() {
            tracing::error!("reading folders, installs and jobs: {e:#}");
        }
        self
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
                Command::Asset(done) => self.on_asset(done),
                Command::Content(report) => self.on_content(report),
                Command::Game(exited) => self.on_game(exited),
                Command::Shim(e) => self.on_shim(e),
                Command::Stats(done) => self.on_stats(done),
                Command::Tickets(e) => self.on_ticket_event(e),
                Command::Networking(r) => self.on_cert_reply(r),
                Command::UpdateHoldOver(app, until) => self.end_update_hold(app, Some(until)),
                Command::Http(e) => self.on_http(e),
                Command::GameExitWait(app) => self.exit_wait_over(app),
                Command::SteamDir => self.sync_take_over(),
                Command::Scripts(r) => self.on_scripts(r),
            }
        };
        // Running jobs stop first and keep their journals (`content.md`
        // §9): stopped while the session is up, they end as cancelled and
        // are queued again, rather than failing on a closed connection.
        self.content_jobs.shutdown();
        self.drain_jobs(&commands);
        // Log off before telling clients the daemon is going (SPEC D-5).
        // Reports sent meanwhile are not needed any more. With the
        // connection closed, PICS requests in flight fail at once.
        self.session.shutdown();
        // Running games keep going (SPEC D-5), but no client is here for
        // them any more.
        self.remove_discovery();
        self.pics.shutdown();
        self.assets.shutdown();
        self.stats.shutdown();
        self.tickets.stop.cancel();
        self.networking.stop.cancel();
        self.http.stop.cancel();
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
                tracing::info!(client = to.client, force = req.force, "shutdown requested");
                if self.games_running() && !req.force {
                    Some(Err(RemoteError::new(
                        ErrorCode::Conflict,
                        "games launched by the daemon are running; stop them, or force the shutdown",
                    )))
                } else {
                    stop = true;
                    Some(Ok(response::Result::Shutdown(v1::ShutdownResponse {})))
                }
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
            request::Method::Logout(_) if self.games_running() => Some(Err(games_block("log out"))),
            request::Method::Logout(_) => {
                self.session.send(Cmd::Logout { reply: to });
                None
            }
            request::Method::ListAccounts(_) => Some(self.list_accounts()),
            request::Method::SwitchAccount(_) if self.games_running() => {
                Some(Err(games_block("switch accounts")))
            }
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
            request::Method::ListApps(req) => Some(self.snapshot().map(|s| {
                response::Result::ListApps(library::list_apps(&s, &req, &|a| self.install_state(a)))
            })),
            request::Method::GetApp(req) => Some(self.snapshot().and_then(|s| {
                library::get_app(&s, &self.pics_cache, req.app_id, &|a| self.install_state(a)).map(
                    |mut r| {
                        let app = ossteam_types::AppId(req.app_id);
                        r.installed = self.installed_proto(app);
                        r.last_shim_report = self.last_shim_report(app);
                        if r.app.as_ref().is_some_and(|a| a.has_info) {
                            r.compat = self.compat_proto(app, self.app_info(app).as_ref());
                        }
                        response::Result::GetApp(r)
                    },
                )
            })),
            request::Method::GetSettings(_) => {
                Some(Ok(response::Result::GetSettings(v1::GetSettingsResponse {
                    settings: self.config.all(),
                })))
            }
            request::Method::SetSetting(req) => Some(self.set_setting(&req)),
            request::Method::GetAsset(req) => self.get_asset(to, &req),
            request::Method::ListLibraryFolders(_) => Some(self.list_folders()),
            request::Method::AddLibraryFolder(req) => Some(self.add_folder(&req)),
            request::Method::RemoveLibraryFolder(req) => Some(self.remove_folder(&req)),
            request::Method::Install(req) => Some(self.install(&req)),
            request::Method::Update(req) => Some(self.update(&req)),
            request::Method::Verify(req) => Some(self.verify(&req)),
            request::Method::Uninstall(req) => Some(self.uninstall(&req)),
            request::Method::ListJobs(_) => Some(self.list_jobs()),
            request::Method::PauseJob(req) => Some(self.pause_job(req.job_id)),
            request::Method::ResumeJob(req) => Some(self.resume_job(req.job_id)),
            request::Method::CancelJob(req) => Some(self.cancel_job(req.job_id)),
            request::Method::SetJobPriority(req) => Some(self.set_job_priority(&req)),
            request::Method::GetLaunchOptions(req) => Some(self.get_launch_options(&req)),
            request::Method::Launch(req) => self.launch(to, req),
            request::Method::ListRunning(_) => Some(Ok(self.list_running())),
            request::Method::StopGame(req) => Some(self.stop_game(&req)),
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
                self.reply(to, *result);
                Ok(())
            }
            Report::State(state) => {
                if state != self.session_state {
                    tracing::debug!(?state, "session changed");
                    let logged_on = state.logged_on;
                    if state.account.is_none() {
                        self.shims.clock = None;
                        self.shims.ip_country.clear();
                    }
                    self.session_state = state;
                    self.sync_discovery();
                    self.broadcast_session();
                    self.update_shims();
                    if logged_on {
                        self.schedule();
                    } else {
                        self.set_requester(None);
                        self.tickets_logged_off();
                    }
                }
                Ok(())
            }
            Report::LoggedOn {
                steam_id,
                requester,
                ip_country,
                server_time,
                public_ip,
            } => {
                self.set_requester(Some(requester));
                self.shim_logged_on(ip_country, server_time);
                self.tickets_logged_on(public_ip);
                // The server forgot the play session with the connection.
                if self.games_running() {
                    self.send_games_played();
                }
                self.db.persona_state(steam_id).map(|state| {
                    self.session.send(Cmd::SetPersonaState { steam_id, state });
                })
            }
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
            Report::Tickets(p) => {
                self.on_ticket_message(&p);
                Ok(())
            }
            Report::FromGc(p) => {
                self.on_gc_message(&p);
                Ok(())
            }
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
        self.check_updates();
        self.schedule();
        self.update_shims();
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

    fn get_asset(&mut self, to: ReplyTo, req: &v1::GetAssetRequest) -> Outcome {
        let kind = match v1::AssetKind::try_from(req.kind) {
            Ok(v1::AssetKind::Header) => AssetKind::Header,
            Ok(v1::AssetKind::Capsule) => AssetKind::Capsule,
            Ok(v1::AssetKind::Hero) => AssetKind::Hero,
            Ok(v1::AssetKind::HeroBlur) => AssetKind::HeroBlur,
            Ok(v1::AssetKind::Logo) => AssetKind::Logo,
            _ => return Some(Err(invalid(format!("unknown asset kind {}", req.kind)))),
        };
        let asset = match self.snapshot().and_then(|s| {
            library::asset(
                &s,
                &self.pics_cache,
                &self.assets_dir,
                req.app_id,
                kind,
                req.high_dpi,
            )
        }) {
            Ok(a) => a,
            Err(e) => return Some(Err(e)),
        };
        if asset.path.is_file() {
            return Some(asset_response(&asset.path));
        }
        if self.missing_assets.contains(&asset.url) {
            return Some(Err(asset_missing(&asset)));
        }
        match self.fetching.entry(asset.path.clone()) {
            Entry::Occupied(mut e) => e.get_mut().push(to),
            Entry::Vacant(e) => {
                e.insert(vec![to]);
                self.assets.fetch(asset);
            }
        }
        None
    }

    fn on_asset(&mut self, done: assets::Done) {
        let waiting = self.fetching.remove(&done.asset.path).unwrap_or_default();
        let result = match done.outcome {
            assets::Outcome::Fetched => asset_response(&done.asset.path),
            assets::Outcome::Missing => {
                let e = asset_missing(&done.asset);
                self.missing_assets.insert(done.asset.url);
                Err(e)
            }
            assets::Outcome::Failed(m) => Err(RemoteError::new(
                ErrorCode::Network,
                format!("downloading the image failed: {m}"),
            )),
        };
        for to in waiting {
            self.reply(to, result.clone());
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
                low_violence: Default::default(),
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
        self.update_shims();
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
        if let Some(config::Value::Text(name)) = &value {
            self.check_tool_setting(def.key, name)?;
        }
        let stored = value.as_ref().map(ToString::to_string);
        self.db
            .set_setting_override(&config::canonical(key), stored.as_deref())
            .map_err(|e| internal(&e))?;
        let before = self.config.proto(key);
        let took_over = self.config.take_over_official();
        self.config.set_override(key, value);
        let after = self.config.proto(key);
        tracing::info!(key = %after.key, value = %after.value, reset = req.reset, "setting changed");
        self.apply_settings();
        self.update_shims();
        if after != before && def.key.starts_with("compat.") {
            // An installed app's platform may change (`compat-tools.md` §8).
            self.check_updates();
        }
        if after != before
            && let Some(app) = config::optional_dlc_app(&after.key)
        {
            self.optional_dlc_changed(app);
        }
        if def.key == config::DISCOVERY_TAKE_OVER_OFFICIAL {
            self.take_over_setting_changed(took_over);
        }
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
    fn apply_settings(&mut self) {
        self.session
            .send(Cmd::SetFileFallback(self.config.file_fallback()));
        self.content_jobs
            .set_bandwidth(self.config.bandwidth_limit());
        // More jobs may run now.
        self.schedule();
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
            active_jobs: self.running_jobs(),
            running_games: self.running_games(),
            logged_on: s.logged_on,
            session_reason: s.reason.clone(),
            persona_state: self.persona_state() as i32,
            game_isolation: self.game_isolation(),
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

fn asset_response(path: &std::path::Path) -> Result<response::Result, RemoteError> {
    let path = path.to_str().ok_or_else(|| {
        RemoteError::new(
            ErrorCode::Internal,
            format!("the cache path {} is not UTF-8", path.display()),
        )
    })?;
    Ok(response::Result::GetAsset(v1::GetAssetResponse {
        path: path.into(),
    }))
}

fn asset_missing(asset: &assets::Asset) -> RemoteError {
    RemoteError::new(
        ErrorCode::NotFound,
        format!("Steam has no image at {}", asset.url),
    )
}

/// Refuses what would take the account away from running games.
fn games_block(what: &str) -> RemoteError {
    RemoteError::new(
        ErrorCode::Conflict,
        format!("games are running on this account; stop them before you {what}"),
    )
}

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::InvalidArgument, message)
}

fn internal(e: &anyhow::Error) -> RemoteError {
    tracing::error!("{e:#}");
    RemoteError::new(ErrorCode::Internal, format!("{e:#}"))
}
