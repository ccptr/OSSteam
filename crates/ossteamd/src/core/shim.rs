// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of the game IPC (`steamworks-abi.md` §3, §4): what
//! the shim is told, what it may ask, the reports it sends, and when a
//! launched game's end is announced.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, TrySendError};
use ossteam_gameipc::{
    App, AppState, Dlc, ErrorReply, Message, RequestBody, Response, ResponseBody, Session, Welcome,
};
use ossteam_ipc::v1::{self, Topic, event};
use ossteam_pics::AppInfo;
use ossteam_types::{AppId, SteamId};

use super::{Command, Core};
use crate::content::folders::Layout;
use crate::gameipc::{ConnId, Event};
use crate::launch::Exited;

/// How long a launched game's end waits for its connections to close
/// (§3.5).
pub const EXIT_WAIT: Duration = Duration::from_secs(2);

/// One connection from a game process.
struct Conn {
    pid: u32,
    app: AppId,
    /// The process belongs to the running game the daemon launched for
    /// `app` (§3.5).
    launched: bool,
    out: Sender<Message>,
    /// The session last sent, with the clock's base instead of its value.
    session: Session,
    app_state: App,
}

/// What one process's shim reported (§4).
#[derive(Default)]
struct Process {
    app: AppId,
    launched: bool,
    conns: usize,
    interfaces: Vec<String>,
    unknown: Vec<String>,
    /// Counts by interface and method, in the order first seen.
    calls: Vec<(String, String, u64)>,
}

impl Process {
    fn add(&mut self, r: ossteam_gameipc::Report) {
        for i in r.interfaces {
            if !self.interfaces.contains(&i) {
                self.interfaces.push(i);
            }
        }
        for u in r.unknown {
            if !self.unknown.contains(&u) {
                self.unknown.push(u);
            }
        }
        for c in r.calls {
            match self
                .calls
                .iter_mut()
                .find(|(i, m, _)| *i == c.interface && *m == c.method)
            {
                Some(entry) => entry.2 += u64::from(c.count),
                None => self.calls.push((c.interface, c.method, u64::from(c.count))),
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.interfaces.is_empty() && self.unknown.is_empty() && self.calls.is_empty()
    }
}

/// A launched game whose tree is gone, waiting for its connections.
struct Exiting {
    exited: Exited,
    running: super::games::Running,
    /// How long the tree ran.
    seconds: u64,
}

#[derive(Default)]
pub struct Shims {
    conns: HashMap<ConnId, Conn>,
    processes: HashMap<u32, Process>,
    exiting: BTreeMap<AppId, Exiting>,
    /// `<data>/userdata`.
    pub user_data: PathBuf,
    /// Steam's clock at the last logon and when it was received.
    pub clock: Option<(u32, Instant)>,
    /// The country Steam gave at the last logon.
    pub ip_country: String,
}

impl Shims {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            user_data: data_dir.join("userdata"),
            ..Default::default()
        }
    }
}

impl Core {
    pub(super) fn on_shim(&mut self, e: Event) {
        match e {
            Event::Connected { conn, hello, out } => self.shim_connected(conn, hello, out),
            Event::Message { conn, msg } => match *msg {
                Message::Request(r) => {
                    let body = self.shim_request(conn, r.body);
                    self.shim_send(conn, Message::Response(Response { id: r.id, body }));
                }
                Message::Report(r) => {
                    if let Some(c) = self.shims.conns.get(&conn) {
                        let pid = c.pid;
                        self.shims.processes.entry(pid).or_default().add(r);
                    }
                }
                _ => {}
            },
            Event::Disconnected { conn } => self.shim_disconnected(conn),
        }
    }

    fn shim_connected(
        &mut self,
        conn: ConnId,
        hello: ossteam_gameipc::Hello,
        out: Sender<Message>,
    ) {
        let app = AppId(hello.app_id);
        let launched = self.games.launched(app, hello.pid);
        tracing::info!(conn, pid = hello.pid, app = %app, launched, arch = %hello.arch, "a game connected");
        let session = self.shim_session();
        let app_state = self.shim_app(app, launched);
        let welcome = Message::Welcome(Welcome {
            session: self.with_clock(session.clone()),
            app: app_state.clone(),
        });
        if out.try_send(welcome).is_err() {
            return;
        }
        let p = self.shims.processes.entry(hello.pid).or_default();
        p.app = app;
        p.launched = launched;
        p.conns += 1;
        self.shims.conns.insert(
            conn,
            Conn {
                pid: hello.pid,
                app,
                launched,
                out,
                session,
                app_state,
            },
        );
    }

    fn shim_disconnected(&mut self, conn: ConnId) {
        let Some(c) = self.shims.conns.remove(&conn) else {
            return;
        };
        tracing::debug!(conn, pid = c.pid, "a game disconnected");
        if let Some(p) = self.shims.processes.get_mut(&c.pid) {
            p.conns = p.conns.saturating_sub(1);
            if p.conns == 0 && !p.launched {
                let p = self.shims.processes.remove(&c.pid).expect("present");
                if !p.is_empty() {
                    let report = shim_report(c.app, c.pid, false, [p].into_iter());
                    self.broadcast(Topic::Games, event::Kind::ShimReport(report));
                }
            }
        }
        if c.launched
            && self.shims.exiting.contains_key(&c.app)
            && !self
                .shims
                .conns
                .values()
                .any(|o| o.launched && o.app == c.app)
        {
            self.finish_game(c.app);
        }
    }

    fn shim_send(&mut self, conn: ConnId, msg: Message) {
        let Some(c) = self.shims.conns.get(&conn) else {
            return;
        };
        match c.out.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!(conn, "a game connection is not reading; dropping it");
                self.shims.conns.remove(&conn);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// GIPC-1: only what §3.4 lists.
    fn shim_request(&self, conn: ConnId, body: RequestBody) -> ResponseBody {
        if !self.shims.conns.contains_key(&conn) {
            return error("the connection is closing");
        }
        match body {
            RequestBody::AppState(r) => {
                let app = AppId(r.app_id);
                let a = self.shim_app(app, false);
                ResponseBody::AppState(AppState {
                    owned: a.owned,
                    installed: a.installed,
                    install_dir: a.install_dir,
                    build_id: a.build_id,
                    depots: a.depots,
                })
            }
        }
    }

    /// The session for the shim, with the clock's base (§3.4).
    fn shim_session(&self) -> Session {
        let s = &self.session_state;
        let account = s.account.as_ref();
        let persona_state = match self.persona_state() {
            v1::PersonaState::Online => 1,
            v1::PersonaState::Invisible => 7,
            _ => 0,
        };
        Session {
            active: account.is_some(),
            logged_on: s.logged_on,
            steam_id: account.map_or(0, |a| a.steam_id),
            persona_name: account.map(|a| a.persona_name.clone()).unwrap_or_default(),
            persona_state,
            ip_country: self.shims.ip_country.clone(),
            server_time: self.shims.clock.map_or(0, |(t, _)| t),
            universe: 1,
            ui_language: self.config.language().to_owned(),
        }
    }

    /// The session as sent: Steam's clock now.
    fn with_clock(&self, mut s: Session) -> Session {
        if let Some((t, at)) = self.shims.clock {
            let elapsed = at.elapsed().as_secs().min(u64::from(u32::MAX)) as u32;
            s.server_time = t.saturating_add(elapsed);
        }
        s
    }

    /// What the shim is told about `app` (§3.4).
    fn shim_app(&self, app: AppId, launched: bool) -> App {
        let account = self
            .session_state
            .account
            .as_ref()
            .map(|a| SteamId(a.steam_id));
        let library = self
            .library
            .as_ref()
            .filter(|s| Some(s.steam_id) == account);
        let owns = |a: AppId| library.is_some_and(|s| s.library.find(a).is_some());
        let owned = owns(app);
        let installed = self.content.installed.get(&app);
        let install_dir = installed
            .and_then(|i| {
                let folder = self.content.folders.iter().find(|f| f.id == i.folder_id)?;
                Some(Layout::new(&folder.path).app_dir(&i.install_dir))
            })
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let depots: Vec<u32> =
            installed.map_or_else(Vec::new, |i| i.depots.iter().map(|d| d.depot.0).collect());
        let info = self.app_info(app);
        let view = info.as_ref().map(AppInfo);
        let dlc = view.map_or_else(Vec::new, |v| {
            let all_depots = v.depots();
            v.dlc()
                .into_iter()
                .map(|d| {
                    let own = owns(d);
                    let of_dlc: Vec<u32> = all_depots
                        .iter()
                        .filter(|x| x.dlc_app_id == Some(d))
                        .map(|x| x.id.0)
                        .collect();
                    let installed = own
                        && installed.is_some()
                        && (of_dlc.is_empty() || of_dlc.iter().any(|x| depots.contains(x)));
                    let dlc_info = self.app_info(d);
                    let dlc_view = dlc_info.as_ref().map(AppInfo);
                    Dlc {
                        app_id: d.0,
                        owned: own,
                        installed,
                        available: dlc_view
                            .is_some_and(|i| i.release_state().eq_ignore_ascii_case("released")),
                        name: dlc_view.map(|i| i.name().to_owned()).unwrap_or_default(),
                    }
                })
                .collect()
        });
        App {
            app_id: app.0,
            owned,
            launched,
            installed: installed.is_some(),
            install_dir,
            build_id: installed.map_or(0, |i| i.build_id),
            branch: installed
                .map(|i| i.branch.clone())
                .filter(|b| b != "public")
                .unwrap_or_default(),
            language: self.config.language().to_owned(),
            languages: view.map_or_else(Vec::new, |v| v.languages()),
            depots,
            dlc,
            user_dir: account
                .filter(|_| app.0 != 0)
                .map(|id| {
                    self.shims
                        .user_data
                        .join(id.account_id().to_string())
                        .join(app.0.to_string())
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default(),
            owner: if owned {
                account.map_or(0, |id| id.0)
            } else {
                0
            },
            low_violence: library.is_some_and(|s| s.low_violence.contains(&app)),
        }
    }

    /// Sends what changed to every game connection (§3.7). Called after
    /// anything that can change a `Session` or an `App`.
    pub(super) fn update_shims(&mut self) {
        if self.shims.conns.is_empty() {
            return;
        }
        let session = self.shim_session();
        let ids: Vec<ConnId> = self.shims.conns.keys().copied().collect();
        for id in ids {
            let (app, launched, old_session, old_app) = {
                let c = &self.shims.conns[&id];
                (c.app, c.launched, c.session.clone(), c.app_state.clone())
            };
            if session != old_session {
                let msg = Message::SessionUpdate(self.with_clock(session.clone()));
                self.shim_send(id, msg);
            }
            let app_state = self.shim_app(app, launched);
            if app_state != old_app {
                self.shim_send(id, Message::AppUpdate(app_state.clone()));
            }
            if let Some(c) = self.shims.conns.get_mut(&id) {
                c.session = session.clone();
                c.app_state = app_state;
            }
        }
    }

    /// The account logged on: Steam's clock and the country (§3.4).
    pub(super) fn shim_logged_on(&mut self, ip_country: String, server_time: Option<u32>) {
        self.shims.ip_country = ip_country;
        self.shims.clock = server_time.map(|t| (t, Instant::now()));
        self.update_shims();
    }

    /// The game's tree is gone. Its end is announced once its connections
    /// have closed, or after [`EXIT_WAIT`] (§3.5).
    pub(super) fn game_gone(
        &mut self,
        exited: Exited,
        running: super::games::Running,
        seconds: u64,
    ) {
        let app = exited.app;
        let open = self
            .shims
            .conns
            .values()
            .any(|c| c.launched && c.app == app);
        self.shims.exiting.insert(
            app,
            Exiting {
                exited,
                running,
                seconds,
            },
        );
        if !open {
            self.finish_game(app);
            return;
        }
        let commands = self.games.commands.clone();
        let timer = std::thread::Builder::new()
            .name(format!("game-{app}-exit"))
            .spawn(move || {
                std::thread::sleep(EXIT_WAIT);
                let _ = commands.send(Command::GameExitWait(app));
            });
        if timer.is_err() {
            tracing::error!(app = %app, "cannot start the exit timer; announcing the end now");
            self.finish_game(app);
        }
    }

    /// From the exit timer.
    pub(super) fn exit_wait_over(&mut self, app: AppId) {
        if self.shims.exiting.contains_key(&app) {
            tracing::debug!(app = %app, "the game's connections did not close in time");
            self.finish_game(app);
        }
    }

    /// Whether `app`'s end is still being announced: it cannot be
    /// launched again until then.
    pub(super) fn game_exiting(&self, app: AppId) -> bool {
        self.shims.exiting.contains_key(&app)
    }

    fn finish_game(&mut self, app: AppId) {
        let Some(Exiting {
            exited,
            running,
            seconds,
        }) = self.shims.exiting.remove(&app)
        else {
            return;
        };
        let pids: Vec<u32> = self
            .shims
            .processes
            .iter()
            .filter(|(_, p)| p.launched && p.app == app)
            .map(|(&pid, _)| pid)
            .collect();
        let processes: Vec<Process> = pids
            .iter()
            .filter_map(|pid| self.shims.processes.remove(pid))
            .collect();
        if !processes.is_empty() {
            let report = shim_report(app, running.proto.pid, true, processes.into_iter());
            self.broadcast(Topic::Games, event::Kind::ShimReport(report));
        }
        self.announce_exit(exited, running, seconds);
    }
}

fn error(message: &str) -> ResponseBody {
    ResponseBody::Error(ErrorReply {
        message: message.into(),
    })
}

/// Adds the processes' reports up (§4).
fn shim_report(
    app: AppId,
    pid: u32,
    launched: bool,
    processes: impl Iterator<Item = Process>,
) -> v1::ShimReport {
    let mut all = Process::default();
    for p in processes {
        all.add(ossteam_gameipc::Report {
            interfaces: p.interfaces,
            unknown: p.unknown,
            calls: Vec::new(),
        });
        for (i, m, n) in p.calls {
            match all
                .calls
                .iter_mut()
                .find(|(ai, am, _)| *ai == i && *am == m)
            {
                Some(e) => e.2 += n,
                None => all.calls.push((i, m, n)),
            }
        }
    }
    // Stable, so that equal counts keep the order first seen.
    all.calls.sort_by_key(|c| std::cmp::Reverse(c.2));
    v1::ShimReport {
        app_id: app.0,
        pid,
        launched,
        interfaces: all.interfaces,
        unknown_versions: all.unknown,
        unimplemented: all
            .calls
            .into_iter()
            .map(|(interface, method, count)| v1::ShimCall {
                interface,
                method,
                count,
            })
            .collect(),
    }
}
