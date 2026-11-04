// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of launching (`launch.md`, `ipc.md` §5.7): the
//! methods and their checks, running games, play sessions (PLAY-1) and
//! the discovery entries (`steamworks-abi.md` §1).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode, Topic, event, response};
use ossteam_launch::discovery::{self, Entries, Libraries};
use ossteam_launch::options::{self, ChoiceError, LaunchOption, Platform};
use ossteam_launch::process::{self, Exit, Game, SpawnError, Stopper};
use ossteam_launch::{command, env, isolation, steamstub};
use ossteam_proto::steam::{CMsgClientGamesPlayed, c_msg_client_games_played::GamePlayed};
use ossteam_types::{AppId, SteamId};

use super::compat::{self, Prepared};
use super::{Command, Core, Outcome, internal, invalid};
use crate::content::folders::Layout;
use crate::launch::{self, Exited};
use crate::session::{Cmd, ReplyTo};
use ossteam_launch::compat::{CompatEnv, Verb, chain};

pub struct Running {
    pub proto: v1::RunningGame,
    stopper: Stopper,
    started: Instant,
    /// Who launched it, and what: for the play session.
    steam_id: SteamId,
    build_id: u32,
    branch: String,
}

/// What [`Core::try_launch`] did.
enum Started {
    Now(v1::RunningGame),
    Waiting(Vec<u64>),
}

/// A launch that waits for its tools.
pub struct Pending {
    to: ReplyTo,
    req: v1::LaunchRequest,
    jobs: Vec<u64>,
}

/// Whether launches get a private `~/.steam` (`steamworks-abi.md`
/// §1.1.2).
pub enum Isolation {
    /// Not Linux: there is none to make.
    NotApplicable,
    Available,
    /// With the failed step and what can be read about why.
    Unavailable(String),
}

impl Isolation {
    fn available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// The launch state the core keeps.
pub struct Games {
    pub settings: launch::Settings,
    pub entries: Entries,
    /// Whether ossteam's discovery entries are in place: on Linux, at
    /// least those of the private `~/.steam`.
    pub entries_made: bool,
    pub isolation: Isolation,
    /// The daemon's data directory, which holds the private `~/.steam`.
    pub data: PathBuf,
    pub running: BTreeMap<AppId, Running>,
    /// Launches waiting for jobs that install their tools
    /// (`compat-tools.md` §4).
    pub pending: BTreeMap<AppId, Pending>,
    /// `<state>/games`.
    pub log_dir: PathBuf,
    /// `<data>/compat-client` (`compat-tools.md` §6.2).
    pub client_dir: PathBuf,
    /// `<data>/steam-sdk`, which a container must see (§9).
    pub sdk_dir: PathBuf,
    /// For the supervisor threads.
    pub commands: Sender<Command>,
}

impl Games {
    pub fn new(
        settings: launch::Settings,
        data: &std::path::Path,
        state: &std::path::Path,
        commands: Sender<Command>,
    ) -> Self {
        Self {
            entries: Entries::new(&settings.home, data),
            settings,
            entries_made: false,
            isolation: Isolation::NotApplicable,
            data: data.to_owned(),
            running: BTreeMap::new(),
            pending: BTreeMap::new(),
            log_dir: state.join("games"),
            client_dir: data.join("compat-client"),
            sdk_dir: data.join("steam-sdk"),
            commands,
        }
    }

    /// Whether `pid` belongs to the running game launched for `app`
    /// (`steamworks-abi.md` §3.5).
    pub fn launched(&self, app: AppId, pid: u32) -> bool {
        self.running
            .get(&app)
            .is_some_and(|r| r.stopper.contains(pid))
    }
}

fn unsupported(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::Unsupported, m)
}

fn conflict(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::Conflict, m)
}

fn not_found(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::NotFound, m)
}

fn discovery_error(e: discovery::Error, isolation: &Isolation) -> RemoteError {
    match e {
        discovery::Error::Conflict(_) => match isolation {
            // §1.1.4: the reason, and the ways out.
            Isolation::Unavailable(reason) => conflict(format!(
                "{e}, so games cannot be launched: games get no private ~/.steam ({reason}). \
                 Allow unprivileged user namespaces (or install ossteamd's AppArmor profile), \
                 or quit the official Steam client and remove its ~/.steam/sdk32, sdk64 and \
                 steam.pid, which it recreates when it starts"
            )),
            _ => conflict(format!("{e}, so games cannot be launched")),
        },
        discovery::Error::NoLibrary(_) | discovery::Error::Unsupported => {
            unsupported(e.to_string())
        }
        discovery::Error::Io { .. } => RemoteError::new(ErrorCode::Internal, e.to_string()),
    }
}

/// The platform whose launch options `app` runs: the one its tool runs
/// programs of, or `native` without a tool (`launch.md` §2).
fn tool_platform(
    app: AppId,
    native: Platform,
    mapping: Option<&ossteam_launch::compat::Mapping<'_>>,
) -> Result<Platform, RemoteError> {
    match mapping.and_then(|m| m.tool.platform()) {
        Some(os) if os.eq_ignore_ascii_case("windows") => Ok(Platform {
            os: "windows",
            ..native
        }),
        Some(os) if !os.eq_ignore_ascii_case(native.os) => Err(unsupported(format!(
            "app {app}'s compatibility tool {} runs {os} programs, which ossteam cannot",
            mapping.map_or("", |m| m.tool.display_name.as_str())
        ))),
        _ => Ok(native),
    }
}

impl Core {
    fn libraries(&self) -> Result<Libraries, discovery::Error> {
        let dir = self
            .games
            .settings
            .library_dir
            .as_deref()
            .ok_or_else(|| discovery::Error::NoLibrary("the daemon's directory".into()))?;
        discovery::find_libraries(dir)
    }

    /// Removes entries a crashed daemon left (DISC-2).
    pub(super) fn clean_discovery(&mut self) {
        if let Err(e) = self.games.entries.remove_stale(std::process::id()) {
            tracing::warn!("cleaning up old discovery entries failed: {e}");
        }
    }

    /// Whether launches get a private `~/.steam`, checked once at start
    /// (`steamworks-abi.md` §1.1.2), and the warning of §1.1.6 when not.
    pub(super) fn check_isolation(&mut self) {
        if !cfg!(target_os = "linux") {
            return;
        }
        let checked = if self.games.settings.private_steam {
            isolation::check(&self.games.data).map_err(|f| f.to_string())
        } else {
            Err("turned off in the daemon's settings".to_owned())
        };
        self.games.isolation = match checked {
            Ok(()) => {
                tracing::info!("games get a private ~/.steam");
                Isolation::Available
            }
            Err(mut reason) => {
                let why = isolation::restrictions();
                if !why.is_empty() {
                    reason = format!("{reason}, likely because {}", why.join(", and "));
                }
                // Launches fail because of it while an official client owns
                // the real entries.
                let fails = matches!(
                    self.games.entries.check(),
                    Err(discovery::Error::Conflict(_))
                );
                let consequence = if fails {
                    "; while an official Steam client owns ~/.steam, games cannot be launched"
                } else {
                    ""
                };
                tracing::warn!("games get no private ~/.steam ({reason}){consequence}");
                eprintln!(
                    "ossteamd: warning: games get no private ~/.steam ({reason}){consequence}"
                );
                Isolation::Unavailable(reason)
            }
        };
    }

    /// A launch's private `~/.steam` failed although the check at start
    /// passed: the launch fails, and the next ones get none until the
    /// daemon restarts (`steamworks-abi.md` §1.1.2).
    fn isolation_failed(&mut self, app: AppId, f: &isolation::Failure) -> RemoteError {
        let reason = f.to_string();
        tracing::warn!(app = %app, "games get no private ~/.steam any more: {reason}");
        self.games.isolation = Isolation::Unavailable(reason.clone());
        self.broadcast(
            Topic::Notices,
            event::Kind::Notice(v1::Notice {
                level: v1::NoticeLevel::Warning as i32,
                text: format!(
                    "App {app} was not launched: games get no private ~/.steam any more \
                     ({reason}). Until ossteamd restarts, games find ossteam only through \
                     the real ~/.steam."
                ),
            }),
        );
        unsupported(format!(
            "app {app} was not launched: games get no private ~/.steam: {reason}"
        ))
    }

    fn make_discovery(&mut self) -> Result<(), discovery::Error> {
        let account = self
            .session_state
            .account
            .as_ref()
            .map_or(0, |a| SteamId(a.steam_id).account_id());
        let libs = self.libraries()?;
        let made = self
            .games
            .entries
            .create(&libs, std::process::id(), account);
        // On Linux a conflict leaves the private `~/.steam`'s entries made
        // (`steamworks-abi.md` §1.5).
        let conflict = matches!(made, Err(discovery::Error::Conflict(_)));
        if made.is_ok() || (cfg!(target_os = "linux") && conflict) {
            if !self.games.entries_made {
                match &made {
                    Ok(()) => tracing::info!("discovery entries created"),
                    Err(e) => tracing::info!(
                        "the private ~/.steam's discovery entries created; not the real ones: {e}"
                    ),
                }
            }
            self.games.entries_made = true;
        }
        match made {
            // Launches find ossteam in their private `~/.steam` (§1.1.4).
            Err(discovery::Error::Conflict(_)) if self.games.isolation.available() => Ok(()),
            r => r,
        }
    }

    pub(super) fn remove_discovery(&mut self) {
        if !self.games.entries_made {
            return;
        }
        match self.games.entries.remove() {
            Ok(()) => tracing::info!("discovery entries removed"),
            Err(e) => tracing::warn!("removing the discovery entries failed: {e}"),
        }
        self.games.entries_made = false;
    }

    /// The entries exist while an account is active (DISC-1).
    pub(super) fn sync_discovery(&mut self) {
        if self.session_state.account.is_none() {
            self.remove_discovery();
        } else if !self.games.entries_made
            && let Err(e) = self.make_discovery()
        {
            tracing::info!("no discovery entries: {e}");
        }
    }

    pub(super) fn running_games(&self) -> u32 {
        self.games.running.len() as u32
    }

    pub(super) fn games_running(&self) -> bool {
        !self.games.running.is_empty()
    }

    /// The installed branch, or `public`.
    fn branch_of(&self, app: AppId) -> String {
        self.content
            .installed
            .get(&app)
            .map(|a| a.branch.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "public".into())
    }

    pub(super) fn get_launch_options(
        &self,
        req: &v1::GetLaunchOptionsRequest,
    ) -> Result<response::Result, RemoteError> {
        let snapshot = self.snapshot()?;
        let app = AppId(req.app_id);
        if snapshot.library.find(app).is_none() {
            return Err(not_found(format!("app {app} is not in the library")));
        }
        let info = self
            .app_info(app)
            .ok_or_else(|| not_found(format!("the info of app {app} is not available")))?;
        let all = options::parse(&info);
        let branch = self.branch_of(app);
        let owns = |d: AppId| snapshot.library.find(d).is_some();
        let language = self.config.language();
        // The options of the platform the app's tool runs, as `Launch`
        // chooses from (`launch.md` §2).
        let list = self.tool_list();
        let mapping = list
            .as_ref()
            .and_then(|l| self.compat_mapping(l, app, Some(&info)));
        let platform =
            Platform::native().and_then(|n| tool_platform(app, n, mapping.as_ref()).ok());
        let options: Vec<v1::LaunchOption> = all
            .iter()
            .map(|o| {
                let mut p = option_proto(o, language);
                p.qualifies = platform.is_some_and(|pl| o.qualifies(pl, &branch, &owns));
                p
            })
            .collect();
        let single = options.iter().filter(|o| o.qualifies).count() == 1;
        Ok(response::Result::GetLaunchOptions(
            v1::GetLaunchOptionsResponse { options, single },
        ))
    }

    /// `Launch`: replies at once, or once the jobs that install the app's
    /// tools are done (`compat-tools.md` §4).
    pub(super) fn launch(&mut self, to: ReplyTo, req: v1::LaunchRequest) -> Outcome {
        let app = AppId(req.app_id);
        if self.games.pending.contains_key(&app) {
            return Some(Err(conflict(format!(
                "app {app} is being launched; its compatibility tools are being installed"
            ))));
        }
        match self.try_launch(&req) {
            Ok(Started::Now(game)) => Some(Ok(response::Result::Launch(v1::LaunchResponse {
                game: Some(game),
            }))),
            Ok(Started::Waiting(jobs)) => {
                tracing::info!(app = %app, ?jobs, "the launch waits for its tools");
                self.games.pending.insert(app, Pending { to, req, jobs });
                None
            }
            Err(e) => Some(Err(e)),
        }
    }

    /// Goes on with the launches whose tool jobs have ended.
    pub(super) fn resume_launches(&mut self) {
        let ready: Vec<AppId> = self
            .games
            .pending
            .iter()
            .filter(|(_, p)| p.jobs.iter().all(|j| !self.job_unfinished(*j)))
            .map(|(a, _)| *a)
            .collect();
        for app in ready {
            let Some(p) = self.games.pending.remove(&app) else {
                continue;
            };
            let failed = p.jobs.iter().find_map(|j| {
                let e = self.content.jobs.get(j)?;
                (e.row.state != v1::JobState::Done).then(|| (*j, e.row.clone()))
            });
            let outcome = match failed {
                Some((j, row)) => Some(Err(conflict(format!(
                    "app {app} was not launched: job {j}, which installs one of its \
                     compatibility tools, ended {}{}",
                    row.state.as_str_name().to_ascii_lowercase(),
                    row.error.map(|(_, m)| format!(": {m}")).unwrap_or_default()
                )))),
                None => self.launch(p.to, p.req),
            };
            if let Some(result) = outcome {
                self.reply(p.to, result);
            }
        }
    }

    /// `launch.md` §2, in order.
    fn try_launch(&mut self, req: &v1::LaunchRequest) -> Result<Started, RemoteError> {
        let Some(account) = self.session_state.account.clone() else {
            return Err(RemoteError::new(
                ErrorCode::NotLoggedIn,
                "no account is logged in",
            ));
        };
        let snapshot = self.snapshot()?;
        let app = AppId(req.app_id);
        if snapshot.library.find(app).is_none() {
            return Err(RemoteError::new(
                ErrorCode::NotOwned,
                format!("app {app} is not in the account's library"),
            ));
        }
        let Some(installed) = self.content.installed.get(&app).cloned() else {
            return Err(not_found(format!("app {app} is not installed")));
        };
        if self.games.running.contains_key(&app) || self.game_exiting(app) {
            return Err(conflict(format!("app {app} is already running")));
        }
        if let Some(j) = self.unfinished_job(app) {
            return Err(conflict(format!(
                "app {app} has job {j}; it can be launched once that job is done or cancelled"
            )));
        }
        if installed.update_available && !req.allow_stale {
            return Err(conflict(format!(
                "app {app} has an update; update it, or launch the installed build anyway with allow_stale"
            )));
        }
        let info = self
            .app_info(app)
            .ok_or_else(|| not_found(format!("the info of app {app} is not available")))?;
        let native = Platform::native()
            .ok_or_else(|| unsupported("Steam has no launch options for this OS"))?;

        // The app's tool decides whose launch options and depots it uses
        // (`compat-tools.md` §2, §4).
        let list = self.tool_list();
        let mapping = list
            .as_ref()
            .and_then(|l| self.compat_mapping(l, app, Some(&info)));
        let platform = tool_platform(app, native, mapping.as_ref())?;
        let installed_os = match installed.platform.as_str() {
            "" => native.os,
            os => os,
        };
        if compat::ENABLED && installed_os != platform.os {
            return Err(conflict(format!(
                "app {app} is installed for {installed_os}, but its compatibility tool runs {} \
                 programs; update it",
                platform.os
            )));
        }
        let all = options::parse(&info);
        let branch = self.branch_of(app);
        let owns = |d: AppId| snapshot.library.find(d).is_some();
        let option = options::choose(&all, req.option, platform, &branch, &owns)
            .map_err(|e| match e {
                // Without the tool list, no app gets Proton (§1).
                ChoiceError::NoneQualifies { .. } if compat::ENABLED && list.is_none() => {
                    unsupported(format!(
                        "{e}; the compatibility tool list (app {}) is not available",
                        ossteam_launch::compat::MANIFESTS_APP
                    ))
                }
                ChoiceError::NoneQualifies { .. } => unsupported(e.to_string()),
                _ => invalid(e.to_string()),
            })?
            .clone();

        let folder = self
            .content
            .folders
            .iter()
            .find(|f| f.id == installed.folder_id)
            .cloned()
            .ok_or_else(|| internal(&anyhow::anyhow!("app {app}'s library folder is gone")))?;
        let layout = Layout::new(&folder.path);
        let app_dir = layout.app_dir(&installed.install_dir);
        // A path that leaves the install directory is refused like a missing
        // executable (`launch.md` §2).
        let mut executable =
            command::under(&app_dir, &option.executable).map_err(|e| not_found(e.to_string()))?;
        // Windows and Wine find names without regard to case (`launch.md` §2).
        let windows = platform.os == "windows";
        if windows
            && !executable.is_file()
            && let Some(found) = command::find_ignoring_case(&app_dir, &executable)
        {
            executable = found;
        }
        if !executable.is_file() {
            return Err(not_found(format!(
                "{} is missing; verify app {app}",
                executable.display()
            )));
        }
        let mut working_dir = command::working_dir(&app_dir, &option.working_dir)
            .map_err(|e| invalid(e.to_string()))?;
        if windows
            && !working_dir.is_dir()
            && let Some(found) = command::find_ignoring_case(&app_dir, &working_dir)
        {
            working_dir = found;
        }
        match steamstub::is_protected(&executable) {
            Ok(false) => {}
            Ok(true) => {
                return Err(unsupported(format!(
                    "{} is protected by Steam DRM (SteamStub), which ossteam supports from M2 on",
                    executable.display()
                )));
            }
            Err(e) => {
                return Err(not_found(format!(
                    "reading {} failed: {e}; verify app {app}",
                    executable.display()
                )));
            }
        }
        self.make_discovery()
            .map_err(|e| discovery_error(e, &self.games.isolation))?;

        let ready = match (&list, &mapping) {
            (Some(list), Some(m)) => {
                let first = m.tool.app;
                match self.prepare_chain(SteamId(account.steam_id), list, first, &folder.id)? {
                    Prepared::Ready(r) => Some(r),
                    Prepared::Waiting(jobs) => return Ok(Started::Waiting(jobs)),
                }
            }
            _ => None,
        };

        let game_line = if cfg!(windows) {
            command::windows_command_line(&executable, &option.arguments, &req.extra_args)
        } else {
            command::shell_command_line(&executable, &option.arguments, &req.extra_args)
        };
        let base_env: Vec<(String, String)> = env::launch_vars(app, &account.account_name)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        let (command_line, env, interrupt_first) = match &ready {
            None => (game_line, base_env, false),
            Some(r) => {
                let data_dir = layout.compat_data(app);
                let client_dir = &self.games.client_dir;
                for d in [&data_dir, &client_dir.join("legacycompat")] {
                    std::fs::create_dir_all(d)
                        .map_err(|e| internal(&anyhow::anyhow!("creating {}: {e}", d.display())))?;
                }
                let list = list.as_ref().expect("a chain has a list");
                let config = list.config(app);
                let mut library_paths = vec![folder.path.clone()];
                library_paths.extend(r.tool_folders.iter().cloned());
                let home = &self.games.settings.home;
                let mounts = if self.games.sdk_dir.starts_with(home) {
                    Vec::new()
                } else {
                    vec![self.games.sdk_dir.clone()]
                };
                let compat_env = CompatEnv {
                    app,
                    install_dir: &app_dir,
                    data_dir: &data_dir,
                    client_dir,
                    library_paths,
                    tool_paths: r.tools.iter().rev().map(|t| t.dir.clone()).collect(),
                    mounts,
                    config: config.map(|c| c.config.as_str()),
                };
                // An app's environment is for Proton (§5.3).
                let extra = config
                    .filter(|_| platform.os == "windows")
                    .map(|c| c.environment.as_str());
                let line = format!(
                    "{} {game_line}",
                    chain::command_prefix(&r.tools, Verb::WaitForExitAndRun)
                );
                let interrupt = r
                    .tools
                    .first()
                    .is_some_and(|t| t.manifest.use_subprocess_reaper);
                (
                    line,
                    compat::chain_env(base_env, &compat_env, extra),
                    interrupt,
                )
            }
        };
        std::fs::create_dir_all(&self.games.log_dir)
            .map_err(|e| internal(&anyhow::Error::from(e)))?;
        let log = self.games.log_dir.join(format!("{app}.log"));
        let mut spec = process::Spec {
            app,
            executable,
            command_line,
            working_dir,
            env,
            log: log.clone(),
            reaper: self.games.settings.reaper.clone(),
            interrupt_first,
            isolation: None,
        };
        if self.games.isolation.available() {
            let plan = isolation::Plan::new(&self.games.settings.home, &self.games.data)
                .map_err(|f| self.isolation_failed(app, &f))?;
            spec.isolation = Some(Arc::new(plan));
        }
        tracing::info!(
            app = %app,
            option = option.index,
            command = %spec.command_line,
            private_steam = spec.isolation.is_some(),
            "launching"
        );
        let game = Game::spawn(&spec).map_err(|e| match e {
            SpawnError::Isolation(f) => self.isolation_failed(app, &f),
            SpawnError::Io(e) => RemoteError::new(
                ErrorCode::Internal,
                format!("starting {} failed: {e}", spec.executable.display()),
            ),
        })?;
        let pid = game.pid();
        let stopper = game.stopper();
        if let Err(e) = launch::supervise(app, game, self.games.commands.clone()) {
            // Without a supervisor nobody would see it end.
            stopper.stop(std::time::Duration::ZERO);
            return Err(internal(&anyhow::anyhow!(
                "starting the supervisor failed: {e}"
            )));
        }
        let proto = v1::RunningGame {
            app_id: app.0,
            option: option.index,
            pid,
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            log: log.to_string_lossy().into_owned(),
        };
        self.games.running.insert(
            app,
            Running {
                proto: proto.clone(),
                stopper,
                started: Instant::now(),
                steam_id: SteamId(account.steam_id),
                build_id: installed.build_id,
                branch,
            },
        );
        self.broadcast(
            Topic::Games,
            event::Kind::GameStarted(v1::GameStarted {
                game: Some(proto.clone()),
            }),
        );
        self.send_games_played();
        Ok(Started::Now(proto))
    }

    pub(super) fn list_running(&self) -> response::Result {
        response::Result::ListRunning(v1::ListRunningResponse {
            games: self
                .games
                .running
                .values()
                .map(|r| r.proto.clone())
                .collect(),
        })
    }

    pub(super) fn stop_game(
        &mut self,
        req: &v1::StopGameRequest,
    ) -> Result<response::Result, RemoteError> {
        let app = AppId(req.app_id);
        let r = self
            .games
            .running
            .get(&app)
            .ok_or_else(|| not_found(format!("app {app} is not running")))?;
        tracing::info!(app = %app, "stopping the game");
        r.stopper.stop(process::STOP_TIMEOUT);
        Ok(response::Result::StopGame(v1::StopGameResponse {}))
    }

    pub(super) fn on_game(&mut self, e: Exited) {
        if self
            .games
            .running
            .get(&e.app)
            .is_none_or(|r| r.proto.pid != e.pid)
        {
            return;
        }
        let r = self.games.running.remove(&e.app).expect("checked above");
        let seconds = r.started.elapsed().as_secs();
        self.game_gone(e, r, seconds);
    }

    /// The game's end, once its connections to the game endpoint closed
    /// (`steamworks-abi.md` §3.5).
    pub(super) fn announce_exit(&mut self, e: Exited, r: Running, seconds: u64) {
        let status = match e.exit {
            Ok(Exit::Code(c)) => v1::game_exited::Status::ExitCode(c),
            Ok(Exit::Signal(s)) => v1::game_exited::Status::Signal(s),
            Err(err) => {
                tracing::error!(app = %e.app, "waiting for the game failed: {err}");
                v1::game_exited::Status::ExitCode(-1)
            }
        };
        tracing::info!(app = %e.app, ?status, seconds, "game exited");
        self.broadcast(
            Topic::Games,
            event::Kind::GameExited(v1::GameExited {
                app_id: e.app.0,
                status: Some(status),
                seconds,
            }),
        );
        // An empty list ends the session (`launch.md` §6).
        if self
            .games
            .running
            .values()
            .all(|g| g.steam_id != r.steam_id)
        {
            self.session.send(Cmd::GamesPlayed {
                steam_id: r.steam_id,
                body: CMsgClientGamesPlayed::default(),
            });
        } else {
            self.send_games_played();
        }
    }

    /// Tells Steam what the active account plays (`launch.md` §6). Sent
    /// at every change and after every logon.
    pub(super) fn send_games_played(&self) {
        let Some(account) = &self.session_state.account else {
            return;
        };
        let steam_id = SteamId(account.steam_id);
        let games_played = self
            .games
            .running
            .iter()
            .filter(|(_, r)| r.steam_id == steam_id)
            .map(|(app, r)| GamePlayed {
                game_id: Some(u64::from(app.0)),
                process_id: Some(r.proto.pid),
                owner_id: Some(steam_id.account_id()),
                game_build_id: (r.build_id != 0).then_some(r.build_id),
                beta_name: (r.branch != "public").then(|| r.branch.clone()),
                ..Default::default()
            })
            .collect();
        self.session.send(Cmd::GamesPlayed {
            steam_id,
            body: CMsgClientGamesPlayed {
                games_played,
                ..Default::default()
            },
        });
    }
}

fn option_proto(o: &LaunchOption, language: &str) -> v1::LaunchOption {
    v1::LaunchOption {
        index: o.index,
        executable: o.executable.clone(),
        arguments: o.arguments.clone(),
        description: o.description_in(language).to_owned(),
        r#type: o.kind.clone(),
        oslist: o.oslist.clone(),
        osarch: o.osarch.clone(),
        betakey: o.betakey.clone(),
        working_dir: o.working_dir.clone(),
        owns_dlc: o.owns_dlc.iter().map(|d| d.0).collect(),
        realm: o.realm.clone(),
        qualifies: false,
    }
}

/// `GetApp`'s options: every option, without `qualifies`.
pub fn app_launch_options(appinfo: &ossteam_vdf::Object, language: &str) -> Vec<v1::LaunchOption> {
    options::parse(appinfo)
        .iter()
        .map(|o| option_proto(o, language))
        .collect()
}
