// SPDX-License-Identifier: GPL-2.0-or-later
//! The store (`gui.md` §3): the GUI's only copy of daemon state. It
//! changes only through replies and events (GUI-2). Applying an event
//! returns the reads it calls for; the model makes them.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ossteam_ipc::v1::{self, NoticeLevel, event};

use crate::daemon::{CallError, Initial};

/// How long an info notice stays (§2.4).
pub const INFO_NOTICE_TIME: Duration = Duration::from_secs(8);

/// A read the store needs after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Read {
    Accounts,
    /// `ListApps` with the current query; the model throttles these.
    Library,
    Jobs,
    Folders,
    /// `GetApp` and `GetLaunchOptions` for an open App page.
    App(u32),
    /// `GetStatus`, for its `game_isolation` only (§3).
    Isolation,
}

/// The ways out when launches fail for want of a private `~/.steam`
/// (`steamworks-abi.md` §1.1.4).
const WAYS_OUT: &str = "Allow unprivileged user namespaces (or install ossteamd's AppArmor \
     profile), or quit the official Steam client and remove its ~/.steam/sdk32, sdk64 and \
     steam.pid, which it recreates when it starts.";

/// What the GUI says when launched games get no private `~/.steam`
/// (`gui.md` §2.4): `None` while they get one.
pub fn isolation_warning(g: &v1::GameIsolation) -> Option<String> {
    if g.available {
        return None;
    }
    let mut text = format!(
        "Games are not isolated from the real ~/.steam: games get no private ~/.steam ({}).",
        g.reason
    );
    if g.launches_blocked {
        text.push_str(" While the official Steam client owns ~/.steam, games cannot be launched. ");
        text.push_str(WAYS_OUT);
    }
    Some(text)
}

/// The `ListApps` parameters of the Library page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryQuery {
    pub filter: String,
    pub installed_only: bool,
}

/// What the store holds for an open App page.
#[derive(Debug, Default)]
pub struct AppDetails {
    pub info: Option<Result<v1::GetAppResponse, CallError>>,
    pub options: Option<Result<v1::GetLaunchOptionsResponse, CallError>>,
}

impl AppDetails {
    /// Whether `app_id` is a compatibility tool of the app's chain.
    fn chain_has(&self, app_id: u32) -> bool {
        let Some(Ok(info)) = &self.info else {
            return false;
        };
        info.compat
            .iter()
            .flat_map(|c| &c.chain)
            .any(|l| l.tool.as_ref().is_some_and(|t| t.app_id == app_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    LoggedOn(v1::Account),
    Failed(String),
}

/// A login this GUI started (ipc.md §5.2, login flow).
#[derive(Debug, Default)]
pub struct Login {
    /// Started and not ended.
    pub running: bool,
    /// The open prompts, one per kind; a rotated QR code replaces the old.
    pub prompts: Vec<v1::LoginPrompt>,
    /// How the last login ended.
    pub ended: Option<LoginOutcome>,
}

impl Login {
    pub fn prompt(&self, kind: v1::LoginPromptKind) -> Option<&v1::LoginPrompt> {
        self.prompts.iter().find(|p| p.kind == kind as i32)
    }
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub id: u64,
    pub level: NoticeLevel,
    pub text: String,
    pub at: Instant,
}

/// How a game ended (`GameExited`), and the pid it ran as when the GUI
/// knew it was running.
pub struct Exit {
    pub exited: v1::GameExited,
    pub pid: Option<u32>,
}

impl Exit {
    /// Whether `r` is this run's report: a launched game's comes just
    /// before its `GameExited`, with the pid the daemon started.
    pub fn reported(&self, r: &v1::ShimReport) -> bool {
        r.launched && self.pid == Some(r.pid)
    }
}

pub struct Store {
    pub status: v1::GetStatusResponse,
    pub version: v1::GetVersionResponse,
    pub accounts: v1::ListAccountsResponse,
    /// `None` without an active account. Shared: views draw from it every
    /// frame without copying it.
    pub library: Option<Arc<v1::ListAppsResponse>>,
    pub library_query: LibraryQuery,
    /// The query `library` answers: until a reply for `library_query`
    /// arrives, the one before.
    pub library_for: LibraryQuery,
    /// Every name a `ListApps` reply has given, whatever its filter: jobs
    /// and pages name apps that the shown library may leave out.
    names: HashMap<u32, String>,
    pub apps: HashMap<u32, AppDetails>,
    pub jobs: BTreeMap<u64, v1::Job>,
    pub running: BTreeMap<u32, v1::RunningGame>,
    pub shim_reports: HashMap<u32, v1::ShimReport>,
    /// How each app's game last ended, of those seen to end since the GUI
    /// connected: the daemon keeps no record of it.
    pub exits: HashMap<u32, Exit>,
    pub settings: BTreeMap<String, v1::Setting>,
    pub folders: Vec<v1::LibraryFolder>,
    pub login: Login,
    /// The active account's token was rejected or is missing.
    pub login_required: bool,
    pub notices: Vec<Notice>,
    next_notice: u64,
}

impl Store {
    pub fn new(i: Initial) -> Self {
        let mut s = Self {
            status: i.status,
            version: i.version,
            accounts: i.accounts,
            names: i.library.as_ref().map(names).unwrap_or_default(),
            library: i.library.map(Arc::new),
            library_query: LibraryQuery::default(),
            library_for: LibraryQuery::default(),
            apps: HashMap::new(),
            jobs: i
                .jobs
                .unwrap_or_default()
                .into_iter()
                .map(|j| (j.job_id, j))
                .collect(),
            running: i.running.into_iter().map(|g| (g.app_id, g)).collect(),
            shim_reports: HashMap::new(),
            exits: HashMap::new(),
            settings: i.settings.into_iter().map(|s| (s.key.clone(), s)).collect(),
            folders: i.folders,
            login: Login::default(),
            login_required: false,
            notices: Vec::new(),
            next_notice: 0,
        };
        // Once per connection (§2.4).
        if let Some(text) = s.status.game_isolation.as_ref().and_then(isolation_warning) {
            s.push_notice(NoticeLevel::Warning, text);
        }
        s
    }

    pub fn account(&self) -> Option<&v1::Account> {
        self.status.account.as_ref()
    }

    /// Whether the window shows the Login page (§1): no active account, or
    /// one that must log in again.
    pub fn needs_login(&self) -> bool {
        match &self.status.account {
            None => true,
            Some(_) => {
                !self.status.logged_on
                    && (self.login_required || !self.status.session_reason.is_empty())
                    && self.status.connection != v1::ConnectionState::Connecting as i32
            }
        }
    }

    pub fn setting(&self, key: &str) -> Option<&v1::Setting> {
        self.settings.get(key)
    }

    /// The app's name from the library, if any `ListApps` reply listed
    /// it, or from its details.
    pub fn app_name(&self, app_id: u32) -> Option<&str> {
        self.names.get(&app_id).map(String::as_str).or_else(|| {
            let info = self.apps.get(&app_id)?.info.as_ref()?.as_ref().ok()?;
            info.app.as_ref().map(|a| a.name.as_str())
        })
    }

    /// The unfinished job of an app, if any.
    pub fn unfinished_job(&self, app_id: u32) -> Option<&v1::Job> {
        self.jobs
            .values()
            .find(|j| j.app_id == app_id && crate::queue::is_unfinished(j))
    }

    pub fn apply(&mut self, kind: event::Kind) -> Vec<Read> {
        match kind {
            event::Kind::SessionChanged(s) => self.session_changed(s),
            event::Kind::LoginPrompt(p) => {
                if self.login.running {
                    self.login.prompts.retain(|o| o.kind != p.kind);
                    self.login.prompts.push(p);
                }
                Vec::new()
            }
            event::Kind::LoginEnded(e) => {
                self.login.running = false;
                self.login.prompts.clear();
                self.login.ended = Some(match (e.account, e.error) {
                    (Some(a), _) => LoginOutcome::LoggedOn(a),
                    (None, Some(err)) => LoginOutcome::Failed(err.message),
                    (None, None) => LoginOutcome::Failed("the login ended".into()),
                });
                vec![Read::Accounts]
            }
            event::Kind::AppChanged(c) => {
                let mut reads = vec![Read::Library];
                reads.extend(
                    c.app_ids
                        .iter()
                        .filter(|id| self.apps.contains_key(id))
                        .map(|&id| Read::App(id)),
                );
                reads
            }
            event::Kind::SettingChanged(c) => {
                if let Some(s) = c.setting {
                    self.settings.insert(s.key.clone(), s);
                }
                Vec::new()
            }
            event::Kind::JobStateChanged(c) => {
                let Some(job) = c.job else {
                    return Vec::new();
                };
                let mut reads = vec![Read::Folders];
                // The app's page, and those whose tool chain it is in.
                reads.extend(
                    self.apps
                        .iter()
                        .filter(|&(&id, d)| id == job.app_id || d.chain_has(job.app_id))
                        .map(|(&id, _)| Read::App(id)),
                );
                self.jobs.insert(job.job_id, job);
                reads
            }
            event::Kind::JobProgress(p) => {
                if let Some(job) = self.jobs.get_mut(&p.job_id) {
                    job.progress = p.progress;
                }
                Vec::new()
            }
            event::Kind::GameStarted(g) => {
                if let Some(g) = g.game {
                    self.running.insert(g.app_id, g);
                }
                Vec::new()
            }
            event::Kind::GameExited(e) => {
                let pid = self.running.remove(&e.app_id).map(|g| g.pid);
                self.exits.insert(e.app_id, Exit { exited: e, pid });
                Vec::new()
            }
            event::Kind::ShimReport(r) => {
                self.shim_reports.insert(r.app_id, r);
                Vec::new()
            }
            event::Kind::Notice(n) => {
                let level = NoticeLevel::try_from(n.level).unwrap_or(NoticeLevel::Info);
                self.push_notice(level, n.text);
                // The daemon warns when the private `~/.steam` changes
                // (`steamworks-abi.md` §1.1.6).
                if level == NoticeLevel::Warning && self.status.game_isolation.is_some() {
                    vec![Read::Isolation]
                } else {
                    Vec::new()
                }
            }
            // The connection thread ends the connection on this one.
            event::Kind::DaemonStopping(_) => Vec::new(),
        }
    }

    fn session_changed(&mut self, s: v1::SessionChanged) -> Vec<Read> {
        let old = self.status.account.as_ref().map(|a| a.steam_id);
        let new = s.account.as_ref().map(|a| a.steam_id);
        self.status.connection = s.connection;
        self.status.account = s.account;
        self.status.logged_on = s.logged_on;
        self.status.session_reason = s.reason;
        self.status.persona_state = s.persona_state;
        self.login_required = s.login_required && !s.logged_on;
        let mut reads = vec![Read::Accounts];
        if old != new {
            // Library, jobs, details, shim reports and exits are the
            // account's.
            self.apps.clear();
            self.names.clear();
            self.shim_reports.clear();
            self.exits.clear();
            self.jobs.clear();
            self.library = None;
            if new.is_some() {
                reads.extend([Read::Library, Read::Jobs]);
            }
        }
        reads
    }

    // Replies.

    pub fn set_accounts(&mut self, a: v1::ListAccountsResponse) {
        self.accounts = a;
    }

    /// A `ListApps` reply for `query`; dropped when the query changed since.
    pub fn set_library(&mut self, query: &LibraryQuery, r: Option<v1::ListAppsResponse>) {
        if let Some(r) = &r {
            self.names.extend(names(r));
        }
        if *query == self.library_query {
            self.library = r.map(Arc::new);
            self.library_for = query.clone();
        }
    }

    pub fn set_jobs(&mut self, jobs: Option<Vec<v1::Job>>) {
        self.jobs = jobs
            .unwrap_or_default()
            .into_iter()
            .map(|j| (j.job_id, j))
            .collect();
    }

    /// A job a reply carried. Events decide the state of jobs the store
    /// has, because a reply can be overtaken by a newer event on its way
    /// (§3); the reply only adds a job no event has announced yet.
    pub fn add_job(&mut self, job: v1::Job) {
        self.jobs.entry(job.job_id).or_insert(job);
    }

    /// Takes only `game_isolation` from a `GetStatus` reply: the session's
    /// fields follow `SessionChanged`, which may be newer.
    pub fn set_isolation(&mut self, g: Option<v1::GameIsolation>) {
        self.status.game_isolation = g;
    }

    pub fn set_folders(&mut self, folders: Vec<v1::LibraryFolder>) {
        self.folders = folders;
    }

    pub fn set_setting(&mut self, s: v1::Setting) {
        self.settings.insert(s.key.clone(), s);
    }

    pub fn open_app(&mut self, app_id: u32) -> Option<Read> {
        if self.apps.contains_key(&app_id) {
            return None;
        }
        self.apps.insert(app_id, AppDetails::default());
        Some(Read::App(app_id))
    }

    pub fn close_app(&mut self, app_id: u32) {
        self.apps.remove(&app_id);
    }

    /// A `GetApp` reply; dropped when the page closed meanwhile.
    pub fn set_app_info(&mut self, app_id: u32, r: Result<v1::GetAppResponse, CallError>) {
        let Some(d) = self.apps.get_mut(&app_id) else {
            return;
        };
        if let Ok(info) = &r {
            match &info.last_shim_report {
                Some(report) => {
                    self.shim_reports.insert(app_id, report.clone());
                }
                None => {
                    self.shim_reports.remove(&app_id);
                }
            }
        }
        d.info = Some(r);
    }

    pub fn set_launch_options(
        &mut self,
        app_id: u32,
        r: Result<v1::GetLaunchOptionsResponse, CallError>,
    ) {
        if let Some(d) = self.apps.get_mut(&app_id) {
            d.options = Some(r);
        }
    }

    /// `LoginStartedResponse`: prompts and the outcome follow as events.
    pub fn login_started(&mut self) {
        self.login.running = true;
        self.login.prompts.clear();
        self.login.ended = None;
    }

    /// The login could not start, or was cancelled.
    pub fn login_stopped(&mut self, error: Option<String>) {
        self.login.running = false;
        self.login.prompts.clear();
        self.login.ended = error.map(LoginOutcome::Failed);
    }

    // Notices.

    pub fn push_notice(&mut self, level: NoticeLevel, text: String) -> u64 {
        let id = self.next_notice;
        self.next_notice += 1;
        self.notices.push(Notice {
            id,
            level,
            text,
            at: Instant::now(),
        });
        id
    }

    pub fn dismiss_notice(&mut self, id: u64) {
        self.notices.retain(|n| n.id != id);
    }

    /// Removes info notices older than [`INFO_NOTICE_TIME`]; true if any
    /// went.
    pub fn expire_notices(&mut self, now: Instant) -> bool {
        let before = self.notices.len();
        self.notices.retain(|n| {
            n.level != NoticeLevel::Info || now.duration_since(n.at) < INFO_NOTICE_TIME
        });
        self.notices.len() != before
    }
}

/// The named apps of a `ListApps` reply, their DLC included.
fn names(r: &v1::ListAppsResponse) -> HashMap<u32, String> {
    r.apps
        .iter()
        .flat_map(|a| std::iter::once(a).chain(a.dlc.iter()))
        .filter(|a| !a.name.is_empty())
        .map(|a| (a.app_id, a.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use v1::{ConnectionState, JobState};

    fn account(id: u64) -> v1::Account {
        v1::Account {
            steam_id: id,
            account_name: format!("a{id}"),
            persona_name: format!("P{id}"),
        }
    }

    fn job(id: u64, app: u32, state: JobState) -> v1::Job {
        v1::Job {
            job_id: id,
            app_id: app,
            state: state as i32,
            ..Default::default()
        }
    }

    fn store() -> Store {
        store_with(None)
    }

    fn store_with(game_isolation: Option<v1::GameIsolation>) -> Store {
        Store::new(Initial {
            status: v1::GetStatusResponse {
                connection: ConnectionState::Connected as i32,
                account: Some(account(1)),
                logged_on: true,
                game_isolation,
                ..Default::default()
            },
            version: Default::default(),
            accounts: Default::default(),
            library: Some(v1::ListAppsResponse {
                apps: vec![v1::AppSummary {
                    app_id: 480,
                    name: "Spacewar".into(),
                    ..Default::default()
                }],
                complete: true,
                ..Default::default()
            }),
            jobs: Some(vec![job(1, 480, JobState::Running)]),
            running: Vec::new(),
            settings: Vec::new(),
            folders: Vec::new(),
        })
    }

    fn session(account_id: Option<u64>, logged_on: bool) -> event::Kind {
        event::Kind::SessionChanged(v1::SessionChanged {
            connection: ConnectionState::Connected as i32,
            account: account_id.map(account),
            logged_on,
            ..Default::default()
        })
    }

    #[test]
    fn no_private_steam_is_a_warning_on_connecting() {
        let available = v1::GameIsolation {
            available: true,
            ..Default::default()
        };
        assert!(store_with(Some(available)).notices.is_empty());
        assert!(store_with(None).notices.is_empty());

        let blocked = v1::GameIsolation {
            reason: "unshare failed".into(),
            launches_blocked: true,
            ..Default::default()
        };
        let mut s = store_with(Some(blocked));
        assert_eq!(s.notices.len(), 1);
        let n = &s.notices[0];
        assert_eq!(n.level, NoticeLevel::Warning);
        assert!(n.text.contains("(unshare failed)"), "{}", n.text);
        assert!(n.text.contains("cannot be launched"), "{}", n.text);

        // A warning from the daemon reads it again; an info does not.
        let notice = |level: NoticeLevel| {
            event::Kind::Notice(v1::Notice {
                level: level as i32,
                text: "x".into(),
            })
        };
        assert_eq!(s.apply(notice(NoticeLevel::Warning)), [Read::Isolation]);
        assert!(s.apply(notice(NoticeLevel::Info)).is_empty());
        s.set_isolation(Some(v1::GameIsolation::default()));
        assert!(!s.status.game_isolation.as_ref().unwrap().launches_blocked);
    }

    #[test]
    fn a_new_account_drops_what_was_the_old_ones() {
        let mut s = store();
        s.open_app(480);
        s.shim_reports.insert(480, v1::ShimReport::default());
        s.exits.insert(
            480,
            Exit {
                exited: v1::GameExited::default(),
                pid: None,
            },
        );
        // The same account going offline keeps everything.
        assert_eq!(s.apply(session(Some(1), false)), [Read::Accounts]);
        assert_eq!(s.jobs.len(), 1);
        assert!(s.library.is_some());

        let reads = s.apply(session(Some(2), true));
        assert_eq!(reads, [Read::Accounts, Read::Library, Read::Jobs]);
        assert!(s.jobs.is_empty() && s.apps.is_empty() && s.shim_reports.is_empty());
        assert!(s.exits.is_empty());
        assert!(s.library.is_none());

        assert_eq!(s.apply(session(None, false)), [Read::Accounts]);
        assert!(s.needs_login());
    }

    #[test]
    fn login_is_needed_without_an_account_or_with_a_rejected_token() {
        let mut s = store();
        assert!(!s.needs_login());
        s.apply(event::Kind::SessionChanged(v1::SessionChanged {
            connection: ConnectionState::Disconnected as i32,
            account: Some(account(1)),
            reason: "the stored login was rejected".into(),
            login_required: true,
            ..Default::default()
        }));
        assert!(s.needs_login());
        // Reconnecting after a network failure is not a login.
        s.apply(event::Kind::SessionChanged(v1::SessionChanged {
            connection: ConnectionState::Connecting as i32,
            account: Some(account(1)),
            ..Default::default()
        }));
        assert!(!s.needs_login());
    }

    #[test]
    fn a_tool_job_rereads_the_pages_whose_chain_it_is_in() {
        let mut s = store();
        s.open_app(22);
        s.open_app(480);
        let tool = |app_id| v1::CompatChainTool {
            tool: Some(v1::CompatTool {
                app_id,
                ..Default::default()
            }),
            ..Default::default()
        };
        let info = v1::GetAppResponse {
            compat: Some(v1::Compat {
                chain: vec![tool(1628350), tool(1493710)],
                ..Default::default()
            }),
            ..Default::default()
        };
        s.set_app_info(22, Ok(info));
        let reads = s.apply(event::Kind::JobStateChanged(v1::JobStateChanged {
            job: Some(job(1, 1493710, JobState::Done)),
        }));
        assert_eq!(reads, [Read::Folders, Read::App(22)]);
    }

    #[test]
    fn job_events_replace_and_replies_only_add() {
        let mut s = store();
        s.open_app(480);
        let reads = s.apply(event::Kind::JobStateChanged(v1::JobStateChanged {
            job: Some(job(1, 480, JobState::Paused)),
        }));
        assert_eq!(reads, [Read::Folders, Read::App(480)]);
        assert_eq!(s.jobs[&1].state, JobState::Paused as i32);

        // A late ResumeJob reply does not undo a newer event.
        s.apply(event::Kind::JobStateChanged(v1::JobStateChanged {
            job: Some(job(1, 480, JobState::Running)),
        }));
        s.add_job(job(1, 480, JobState::Queued));
        assert_eq!(s.jobs[&1].state, JobState::Running as i32);
        // An Install reply ahead of its event shows the job.
        s.add_job(job(2, 10, JobState::Queued));
        assert!(s.jobs.contains_key(&2));

        s.apply(event::Kind::JobProgress(v1::JobProgress {
            job_id: 1,
            progress: Some(v1::JobProgressInfo {
                bytes_done: 5,
                ..Default::default()
            }),
        }));
        assert_eq!(s.jobs[&1].progress.as_ref().unwrap().bytes_done, 5);
        assert_eq!(s.unfinished_job(480).map(|j| j.job_id), Some(1));
    }

    #[test]
    fn app_changes_read_the_library_and_open_pages() {
        let mut s = store();
        assert_eq!(s.open_app(480), Some(Read::App(480)));
        assert_eq!(s.open_app(480), None);
        let reads = s.apply(event::Kind::AppChanged(v1::AppChanged {
            app_ids: vec![480, 10],
            complete: true,
        }));
        assert_eq!(reads, [Read::Library, Read::App(480)]);

        s.close_app(480);
        s.set_app_info(480, Ok(v1::GetAppResponse::default()));
        assert!(!s.apps.contains_key(&480));
    }

    #[test]
    fn library_replies_for_an_old_query_are_dropped() {
        let mut s = store();
        let old = s.library_query.clone();
        s.library_query.filter = "space".into();
        s.set_library(&old, None);
        assert!(s.library.is_some());
        assert_eq!(s.library_for, old);
        let q = s.library_query.clone();
        s.set_library(&q, None);
        assert!(s.library.is_none());
        assert_eq!(s.library_for, q);
    }

    #[test]
    fn apps_keep_their_names_when_a_filter_leaves_them_out() {
        let mut s = store();
        s.library_query.filter = "other".into();
        let q = s.library_query.clone();
        s.set_library(&q, Some(v1::ListAppsResponse::default()));
        assert!(s.library.as_ref().is_some_and(|l| l.apps.is_empty()));
        assert_eq!(s.app_name(480), Some("Spacewar"));

        s.apply(session(Some(2), true));
        assert_eq!(s.app_name(480), None);
    }

    #[test]
    fn shim_reports_come_from_get_app_and_events() {
        let mut s = store();
        s.open_app(480);
        let report = |pid| v1::ShimReport {
            app_id: 480,
            pid,
            ..Default::default()
        };
        s.set_app_info(
            480,
            Ok(v1::GetAppResponse {
                last_shim_report: Some(report(1)),
                ..Default::default()
            }),
        );
        assert_eq!(s.shim_reports[&480].pid, 1);
        s.apply(event::Kind::ShimReport(report(2)));
        assert_eq!(s.shim_reports[&480].pid, 2);
    }

    #[test]
    fn login_prompts_belong_to_a_running_login() {
        let mut s = store();
        let qr = |url: &str| {
            event::Kind::LoginPrompt(v1::LoginPrompt {
                kind: v1::LoginPromptKind::QrUrl as i32,
                qr_url: url.into(),
                ..Default::default()
            })
        };
        s.apply(qr("https://s.team/q/1/a"));
        assert!(s.login.prompts.is_empty());

        s.login_started();
        s.apply(qr("https://s.team/q/1/a"));
        s.apply(qr("https://s.team/q/1/b"));
        assert_eq!(s.login.prompts.len(), 1);
        assert_eq!(
            s.login.prompt(v1::LoginPromptKind::QrUrl).unwrap().qr_url,
            "https://s.team/q/1/b"
        );

        let reads = s.apply(event::Kind::LoginEnded(v1::LoginEnded {
            account: None,
            error: Some(v1::Error {
                message: "the code expired".into(),
                ..Default::default()
            }),
        }));
        assert_eq!(reads, [Read::Accounts]);
        assert!(!s.login.running && s.login.prompts.is_empty());
        assert_eq!(
            s.login.ended,
            Some(LoginOutcome::Failed("the code expired".into()))
        );
    }

    #[test]
    fn games_and_settings_follow_their_events() {
        let mut s = store();
        s.apply(event::Kind::GameStarted(v1::GameStarted {
            game: Some(v1::RunningGame {
                app_id: 480,
                pid: 9,
                ..Default::default()
            }),
        }));
        assert_eq!(s.running[&480].pid, 9);
        s.apply(event::Kind::GameExited(v1::GameExited {
            app_id: 480,
            seconds: 7,
            ..Default::default()
        }));
        assert!(s.running.is_empty());
        assert_eq!(s.exits[&480].exited.seconds, 7);
        assert_eq!(s.exits[&480].pid, Some(9));
        let report = |pid, launched| v1::ShimReport {
            app_id: 480,
            pid,
            launched,
            ..Default::default()
        };
        assert!(s.exits[&480].reported(&report(9, true)));
        assert!(!s.exits[&480].reported(&report(8, true)));
        assert!(!s.exits[&480].reported(&report(9, false)));

        s.apply(event::Kind::SettingChanged(v1::SettingChanged {
            setting: Some(v1::Setting {
                key: "content.concurrent_jobs".into(),
                value: "2".into(),
                ..Default::default()
            }),
        }));
        assert_eq!(s.setting("content.concurrent_jobs").unwrap().value, "2");
    }

    #[test]
    fn info_notices_expire_and_others_stay() {
        let mut s = store();
        let info = s.push_notice(NoticeLevel::Info, "installed".into());
        let warn = s.push_notice(NoticeLevel::Warning, "slow".into());
        assert!(!s.expire_notices(Instant::now()));
        assert!(s.expire_notices(Instant::now() + INFO_NOTICE_TIME));
        assert_eq!(s.notices.iter().map(|n| n.id).collect::<Vec<_>>(), [warn]);
        s.dismiss_notice(warn);
        assert!(s.notices.is_empty());
        assert_ne!(info, warn);
    }
}
