// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of launching (`launch.md`, `ipc.md` §5.7): the
//! methods and their checks, running games, play sessions (PLAY-1) and
//! the discovery entries (`steamworks-abi.md` §1).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode, Topic, event, response};
use ossteam_launch::discovery::{self, Entries, Libraries};
use ossteam_launch::options::{self, ChoiceError, LaunchOption, Platform};
use ossteam_launch::process::{self, Exit, Game, Stopper};
use ossteam_launch::{command, env, steamstub};
use ossteam_proto::steam::{CMsgClientGamesPlayed, c_msg_client_games_played::GamePlayed};
use ossteam_types::{AppId, SteamId};

use super::{Command, Core, internal, invalid};
use crate::content::folders::Layout;
use crate::launch::{self, Exited};
use crate::session::Cmd;

pub struct Running {
    pub proto: v1::RunningGame,
    stopper: Stopper,
    started: Instant,
    /// Who launched it, and what: for the play session.
    steam_id: SteamId,
    build_id: u32,
    branch: String,
}

/// The launch state the core keeps.
pub struct Games {
    pub settings: launch::Settings,
    pub entries: Entries,
    /// Whether ossteam's discovery entries are in place.
    pub entries_made: bool,
    pub running: BTreeMap<AppId, Running>,
    /// `<state>/games`.
    pub log_dir: PathBuf,
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
            running: BTreeMap::new(),
            log_dir: state.join("games"),
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

fn discovery_error(e: discovery::Error) -> RemoteError {
    match e {
        discovery::Error::Conflict(_) => conflict(e.to_string()),
        discovery::Error::NoLibrary(_) | discovery::Error::Unsupported => {
            unsupported(e.to_string())
        }
        discovery::Error::Io { .. } => RemoteError::new(ErrorCode::Internal, e.to_string()),
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

    fn make_discovery(&mut self) -> Result<(), discovery::Error> {
        let account = self
            .session_state
            .account
            .as_ref()
            .map_or(0, |a| SteamId(a.steam_id).account_id());
        let libs = self.libraries()?;
        self.games
            .entries
            .create(&libs, std::process::id(), account)?;
        if !self.games.entries_made {
            tracing::info!("discovery entries created");
        }
        self.games.entries_made = true;
        Ok(())
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

    fn launch_options(&self, app: AppId) -> Result<Vec<LaunchOption>, RemoteError> {
        let info = self
            .app_info(app)
            .ok_or_else(|| not_found(format!("the info of app {app} is not available")))?;
        Ok(options::parse(&info))
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
        let all = self.launch_options(app)?;
        let branch = self.branch_of(app);
        let owns = |d: AppId| snapshot.library.find(d).is_some();
        let language = self.config.language();
        let platform = Platform::native();
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

    /// `launch.md` §2, in order.
    pub(super) fn launch(
        &mut self,
        req: &v1::LaunchRequest,
    ) -> Result<response::Result, RemoteError> {
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
        let platform = Platform::native()
            .ok_or_else(|| unsupported("Steam has no launch options for this OS"))?;
        let all = self.launch_options(app)?;
        let branch = self.branch_of(app);
        let owns = |d: AppId| snapshot.library.find(d).is_some();
        let option = options::choose(&all, req.option, platform, &branch, &owns)
            .map_err(|e| match e {
                ChoiceError::NoneQualifies { .. } => unsupported(e.to_string()),
                _ => invalid(e.to_string()),
            })?
            .clone();

        let folder = self
            .content
            .folders
            .iter()
            .find(|f| f.id == installed.folder_id)
            .ok_or_else(|| internal(&anyhow::anyhow!("app {app}'s library folder is gone")))?;
        let app_dir = Layout::new(&folder.path).app_dir(&installed.install_dir);
        // A path that leaves the install directory is refused like a missing
        // executable (`launch.md` §2).
        let executable =
            command::under(&app_dir, &option.executable).map_err(|e| not_found(e.to_string()))?;
        if !executable.is_file() {
            return Err(not_found(format!(
                "{} is missing; verify app {app}",
                executable.display()
            )));
        }
        let working_dir = command::working_dir(&app_dir, &option.working_dir, &executable)
            .map_err(|e| invalid(e.to_string()))?;
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
        self.make_discovery().map_err(discovery_error)?;

        let command_line = if cfg!(windows) {
            command::windows_command_line(&executable, &option.arguments, &req.extra_args)
        } else {
            command::shell_command_line(&executable, &option.arguments, &req.extra_args)
        };
        std::fs::create_dir_all(&self.games.log_dir)
            .map_err(|e| internal(&anyhow::Error::from(e)))?;
        let log = self.games.log_dir.join(format!("{app}.log"));
        let spec = process::Spec {
            app,
            executable,
            command_line,
            working_dir,
            env: env::launch_vars(app, &account.account_name),
            log: log.clone(),
            reaper: self.games.settings.reaper.clone(),
        };
        tracing::info!(app = %app, option = option.index, command = %spec.command_line, "launching");
        let game = Game::spawn(&spec).map_err(|e| {
            RemoteError::new(
                ErrorCode::Internal,
                format!("starting {} failed: {e}", spec.executable.display()),
            )
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
        Ok(response::Result::Launch(v1::LaunchResponse {
            game: Some(proto),
        }))
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
