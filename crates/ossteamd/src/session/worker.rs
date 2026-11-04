// SPDX-License-Identifier: GPL-2.0-or-later
//! The session worker: one thread that owns the CM connection and runs the
//! session state machine (`cm.md` §4 and §5, `auth.md` §2 to §6).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, never, select};
use ossteam_auth::AuthError;
use ossteam_auth::logon::{self, LogonError};
use ossteam_auth::session::{
    self as auth, CodeKind, DeviceIdentity, Guard, PendingSession, Persistence, Poll, Tokens,
};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ConnectionState, ErrorCode, LoginPromptKind, response};
use ossteam_net::directory::jitter;
use ossteam_net::{CallError, Connection, Incoming, Packet, SessionIds};
use ossteam_proto::steam::{
    CMsgClientAccountInfo, CMsgClientChangeStatus, CMsgClientGamesPlayed, EMsg,
};
use ossteam_types::{CancelToken, EResult, Secret, SteamId};

use super::{Cmd, Dialer, NoServer, ReplyTo, Report, Settings, State};
use crate::core::Command;
use crate::secrets::{self, Secrets};
use crate::{db, pics};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long starting a login or switching accounts keeps trying servers.
const CONNECT_BUDGET: Duration = Duration::from_secs(30);
const LOGON_TIMEOUT: Duration = Duration::from_secs(30);
const LOGOFF_TIMEOUT: Duration = Duration::from_secs(2);
/// The wait between failed attempts to log the active account back on
/// doubles from this, with jitter, up to [`RETRY_CAP`] (`cm.md` §5).
const RETRY_START: Duration = Duration::from_secs(1);
const RETRY_CAP: Duration = Duration::from_secs(60);
/// The longest a blocking wait goes without checking for cancellation
/// (CONC-2).
const SLICE: Duration = Duration::from_secs(1);
/// The worker's wait when nothing is scheduled.
const IDLE: Duration = Duration::from_secs(3600);

/// `ClientLoggedOff` reasons that mean another client took the session.
/// Reconnecting would take it back, so the daemon waits for the user.
const REPLACED: [EResult; 3] = [
    EResult::LOGGED_IN_ELSEWHERE,
    EResult::LOGON_SESSION_REPLACED,
    EResult::ALREADY_LOGGED_IN_ELSEWHERE,
];

/// Steam Guard results that mean the code was wrong or stale.
const BAD_CODE: [EResult; 3] = [
    EResult::INVALID_LOGIN_AUTH_CODE,
    EResult::TWO_FACTOR_CODE_MISMATCH,
    EResult::EXPIRED_LOGIN_AUTH_CODE,
];

/// Sign-in refusals that are about load or rate rather than the account.
const BUSY: [EResult; 6] = [
    EResult::RATE_LIMIT_EXCEEDED,
    EResult::ACCOUNT_LOGIN_DENIED_THROTTLE,
    EResult::SERVICE_UNAVAILABLE,
    EResult::BUSY,
    EResult::TRY_ANOTHER_CM,
    EResult::TIMEOUT,
];

struct Live {
    conn: Connection,
    incoming: Receiver<Incoming>,
}

/// The account that should be logged on.
struct Active {
    steam_id: SteamId,
    account_name: String,
    persona_name: String,
    token: Secret,
    /// Whether the token is stored, as opposed to `--no-remember`.
    remembered: bool,
}

impl Active {
    fn proto(&self) -> v1::Account {
        v1::Account {
            steam_id: self.steam_id.0,
            account_name: self.account_name.clone(),
            persona_name: self.persona_name.clone(),
        }
    }
}

enum Method {
    Credentials { username: String, password: Secret },
    Qr,
}

struct Login {
    pending: PendingSession,
    /// The name typed for a credential login. QR logins learn it at the end.
    username: Option<String>,
    remember: bool,
    next_poll: Instant,
    cancel: CancelToken,
}

enum Attempt {
    /// Every server is backing off; try again after this long.
    Wait(Duration),
    /// There is no server list to try.
    NoServers(String),
    Failed(String),
}

/// Why a logon failed, which decides what happens next (`auth.md` §4).
enum LogonFailure {
    /// The connection or the server failed: try again, elsewhere.
    Transient(String),
    /// Steam rejected the token: forget it and ask for a new login.
    TokenRejected(String),
    /// Steam refused the account for another reason: stop retrying.
    Refused(String),
    Interrupted,
}

impl LogonFailure {
    fn error(&self) -> RemoteError {
        match self {
            Self::Transient(e) => RemoteError::new(
                ErrorCode::Network,
                format!("the logon failed ({e}); the daemon keeps retrying"),
            ),
            Self::TokenRejected(e) => RemoteError::new(
                ErrorCode::NotLoggedIn,
                format!("Steam rejected the stored token ({e}); log in again"),
            ),
            Self::Refused(e) => RemoteError::new(ErrorCode::NotLoggedIn, e.clone()),
            Self::Interrupted => RemoteError::new(ErrorCode::NotLoggedIn, "interrupted"),
        }
    }
}

fn send_report(core: &Sender<Command>, r: Report) {
    let _ = core.send(Command::Session(r));
}

pub struct Worker {
    core: Sender<Command>,
    settings: Settings,
    servers: Box<dyn Dialer>,
    /// Opened on first use: on Linux that may start the Secret Service.
    secrets: Option<Secrets>,
    cell_id: u32,
    identity: DeviceIdentity,
    live: Option<Live>,
    active: Option<Active>,
    logged_on: bool,
    login: Option<Login>,
    /// When to try to log the active account on again. `None` while
    /// logged on, and while retrying is pointless until the user acts.
    retry_at: Option<Instant>,
    retry_failures: u32,
    /// A connection attempt is under way.
    connecting: bool,
    reason: String,
    login_required: bool,
    published: Option<State>,
    /// The account the PICS worker was last told about.
    pics_account: Option<SteamId>,
    login_cancel: Arc<Mutex<CancelToken>>,
    stop: CancelToken,
}

impl Worker {
    pub fn new(
        settings: Settings,
        servers: Box<dyn Dialer>,
        core: Sender<Command>,
        login_cancel: Arc<Mutex<CancelToken>>,
        stop: CancelToken,
    ) -> Self {
        Self {
            core,
            servers,
            cell_id: settings.cell_id,
            settings,
            secrets: None,
            identity: DeviceIdentity {
                friendly_name: host_name(),
            },
            live: None,
            active: None,
            logged_on: false,
            login: None,
            retry_at: None,
            retry_failures: 0,
            connecting: false,
            reason: String::new(),
            login_required: false,
            published: None,
            pics_account: None,
            login_cancel,
            stop,
        }
    }

    /// Runs until the command channel closes, then logs off (SPEC D-5).
    pub fn run(mut self, cmds: &Receiver<Cmd>) {
        if let Some(account) = self.settings.resume.take() {
            self.resume(account);
        }
        self.publish();
        loop {
            let timeout = self
                .next_deadline()
                .map_or(IDLE, |d| d.saturating_duration_since(Instant::now()));
            let incoming = self
                .live
                .as_ref()
                .map_or_else(never, |l| l.incoming.clone());
            select! {
                recv(cmds) -> cmd => match cmd {
                    Ok(cmd) => self.on_cmd(cmd),
                    Err(_) => break,
                },
                recv(incoming) -> m => match m {
                    Ok(Incoming::Message(p)) => self.on_message(&p),
                    Ok(Incoming::Closed(r)) => self.on_closed(&r.to_string()),
                    Err(_) => self.on_closed("the connection's I/O thread is gone"),
                },
                default(timeout) => {}
            }
            self.on_timers();
            self.settle();
            self.publish();
        }
        if self.login.is_some() {
            self.end_login(Err(RemoteError::new(
                ErrorCode::NotLoggedIn,
                "the daemon is stopping",
            )));
        }
        self.logoff();
        self.drop_connection();
    }

    fn report(&self, r: Report) {
        send_report(&self.core, r);
    }

    fn reply(&self, to: ReplyTo, result: Result<response::Result, RemoteError>) {
        self.report(Report::Reply {
            to,
            result: Box::new(result),
        });
    }

    fn secrets(&mut self) -> &Secrets {
        let s = &self.settings;
        self.secrets
            .get_or_insert_with(|| (s.open_secrets)(s.file_fallback, &s.data_dir))
    }

    fn state(&self) -> State {
        let connection = if self.live.is_some() {
            ConnectionState::Connected
        } else if self.connecting || self.retry_at.is_some() {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        State {
            connection,
            account: self.active.as_ref().map(Active::proto),
            logged_on: self.logged_on,
            reason: self.reason.clone(),
            login_required: self.login_required,
        }
    }

    /// Reports the state if it changed since the last report, and tells
    /// the PICS worker when the active account changed.
    fn publish(&mut self) {
        let account = self.active.as_ref().map(|a| a.steam_id);
        if account != self.pics_account {
            self.pics_account = account;
            let _ = self.settings.pics.send(pics::Cmd::Account(account));
        }
        let state = self.state();
        if self.published.as_ref() != Some(&state) {
            self.published = Some(state.clone());
            self.report(Report::State(state));
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        match &self.login {
            Some(l) => Some(l.next_poll),
            None => self.retry_at,
        }
    }

    fn on_timers(&mut self) {
        let now = Instant::now();
        if self.login.as_ref().is_some_and(|l| now >= l.next_poll) {
            self.poll_login();
        } else if self.login.is_none() && self.retry_at.is_some_and(|t| now >= t) {
            self.retry_at = None;
            self.reconnect();
        }
    }

    /// Closes a connection nobody needs: no account and no login.
    fn settle(&mut self) {
        if self.active.is_none() && self.login.is_none() && self.live.is_some() {
            self.drop_connection();
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::LoginWithCredentials {
                reply,
                username,
                password,
                remember,
            } => self.start_login(reply, Method::Credentials { username, password }, remember),
            Cmd::LoginWithQr { reply, remember } => self.start_login(reply, Method::Qr, remember),
            Cmd::SubmitGuardCode { reply, code, kind } => self.submit_code(reply, code, kind),
            Cmd::CancelLogin { reply } => {
                if self.login.is_some() {
                    self.end_login(Err(RemoteError::new(
                        ErrorCode::NotLoggedIn,
                        "the login was cancelled",
                    )));
                }
                if let Some(to) = reply {
                    self.reply(
                        to,
                        Ok(response::Result::CancelLogin(v1::CancelLoginResponse {})),
                    );
                }
            }
            Cmd::Logout { reply } => self.logout(reply),
            Cmd::SwitchAccount { reply, account } => self.switch(reply, account),
            Cmd::SetPersonaState { steam_id, state } => self.set_persona_state(steam_id, state),
            Cmd::GamesPlayed { steam_id, body } => self.games_played(steam_id, &body),
            Cmd::SetFileFallback(on) => {
                if self.settings.file_fallback != on {
                    tracing::info!(on, "secrets.file_fallback changed; reopening token storage");
                    self.settings.file_fallback = on;
                    // Opened again on next use, with the new choice.
                    self.secrets = None;
                }
            }
        }
    }

    // --- Connection -------------------------------------------------------

    /// One attempt at the next server, unless a connection is already up.
    fn connect_once(&mut self) -> Result<(), Attempt> {
        if self.live.as_ref().is_some_and(|l| !l.conn.is_closed()) {
            return Ok(());
        }
        self.live = None;
        let server = match self.servers.next(self.cell_id) {
            Ok(s) => s,
            Err(NoServer::Wait(d)) => return Err(Attempt::Wait(d)),
            Err(NoServer::None(e)) => return Err(Attempt::NoServers(e)),
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        // The connection lives until it is closed here or the daemon is
        // interrupted, so it gets the stop token, not a login's token.
        let conn = match self
            .servers
            .connect(&server, CONNECT_TIMEOUT, tx, &self.stop)
        {
            Ok(c) => c,
            Err(e) => {
                self.servers.failed(&server.endpoint);
                return Err(Attempt::Failed(format!("{}: {e}", server.endpoint)));
            }
        };
        if let Err(e) = conn.hello() {
            self.servers.failed(&server.endpoint);
            return Err(Attempt::Failed(format!("{}: {e}", server.endpoint)));
        }
        self.live = Some(Live { conn, incoming: rx });
        Ok(())
    }

    /// Tries servers until one connects or `budget` runs out.
    fn connect_within(&mut self, budget: Duration, cancel: &CancelToken) -> Result<(), String> {
        let deadline = Instant::now() + budget;
        let mut last = "no server could be reached".to_owned();
        loop {
            if cancel.is_cancelled() || self.stop.is_cancelled() {
                return Err("cancelled".into());
            }
            match self.connect_once() {
                Ok(()) => return Ok(()),
                Err(Attempt::NoServers(e)) => return Err(e),
                Err(Attempt::Failed(e)) => {
                    tracing::info!("CM connection attempt failed: {e}");
                    last = e;
                }
                Err(Attempt::Wait(wait)) => {
                    if Instant::now() + wait >= deadline {
                        return Err(last);
                    }
                    let until = Instant::now() + wait;
                    while Instant::now() < until {
                        if cancel.is_cancelled() || self.stop.is_cancelled() {
                            return Err("cancelled".into());
                        }
                        std::thread::sleep(
                            SLICE.min(until.saturating_duration_since(Instant::now())),
                        );
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(last);
            }
        }
    }

    fn drop_connection(&mut self) {
        if let Some(l) = self.live.take() {
            l.conn.close();
        }
        self.logged_on = false;
    }

    fn on_closed(&mut self, why: &str) {
        let Some(live) = self.live.take() else {
            return;
        };
        let endpoint = live.conn.server().endpoint.clone();
        drop(live);
        let was_logged_on = std::mem::replace(&mut self.logged_on, false);
        if self.login.is_some() {
            self.end_login(Err(RemoteError::new(
                ErrorCode::Network,
                format!("lost the connection to Steam during the login: {why}"),
            )));
        }
        if self.active.is_some() && was_logged_on {
            // Reconnect at once, to another server (NET-4). Further
            // failures back off.
            tracing::info!("connection to {endpoint} lost ({why}); reconnecting");
            self.servers.failed(&endpoint);
            self.reason = format!("connection lost: {why}");
            self.retry_failures = 0;
            self.retry_at = Some(Instant::now());
        }
    }

    fn schedule_retry(&mut self) {
        self.retry_failures = self.retry_failures.saturating_add(1);
        let base = RETRY_START
            .saturating_mul(1 << self.retry_failures.saturating_sub(1).min(16))
            .min(RETRY_CAP);
        let wait = base + jitter(base / 2);
        tracing::info!(
            attempt = self.retry_failures,
            ?wait,
            "next logon attempt scheduled"
        );
        self.retry_at = Some(Instant::now() + wait);
    }

    // --- Logon ------------------------------------------------------------

    /// Logs the active account on over the live connection (`cm.md` §4).
    fn logon(&mut self) -> Result<(), LogonFailure> {
        let Some(active) = &self.active else {
            return Err(LogonFailure::Refused("no account to log on".into()));
        };
        let packet = logon::logon_packet(&active.account_name, &active.token, self.cell_id);
        let Some(live) = &self.live else {
            return Err(LogonFailure::Transient("not connected".into()));
        };
        let endpoint = live.conn.server().endpoint.clone();
        live.conn
            .send(packet)
            .map_err(|e| LogonFailure::Transient(e.to_string()))?;
        let reply = self.wait_for(EMsg::KEMsgClientLogOnResponse, LOGON_TIMEOUT)?;
        let ok = match logon::parse_logon_response(&reply) {
            Ok(ok) => ok,
            Err(e) => {
                return Err(if e.token_rejected() {
                    LogonFailure::TokenRejected(e.to_string())
                } else if e.is_transient() {
                    self.servers.failed(&endpoint);
                    LogonFailure::Transient(e.to_string())
                } else {
                    LogonFailure::Refused(match e {
                        LogonError::Refused { eresult, .. } => {
                            format!("Steam refused the logon: {eresult}")
                        }
                        other => other.to_string(),
                    })
                });
            }
        };

        let live = self.live.as_ref().expect("checked by wait_for");
        live.conn.set_session(SessionIds {
            steam_id: ok.steam_id,
            session_id: ok.session_id,
        });
        match ok.heartbeat {
            Some(interval) => live.conn.set_heartbeat(interval),
            None => tracing::warn!("the logon response set no heartbeat interval"),
        }
        self.servers.succeeded(&endpoint);
        if let Some(cell) = ok.cell_id.filter(|&c| c != self.cell_id) {
            self.cell_id = cell;
            self.report(Report::CellId(cell));
        }
        let active = self.active.as_mut().expect("checked above");
        if active.steam_id != ok.steam_id {
            tracing::warn!(expected = %active.steam_id, got = %ok.steam_id, "logged on with a different Steam id");
            active.steam_id = ok.steam_id;
        }
        tracing::info!(account = %active.account_name, steam_id = %ok.steam_id, "logged on");
        self.pics_account = Some(ok.steam_id);
        let _ = self.settings.pics.send(pics::Cmd::LoggedOn {
            steam_id: ok.steam_id,
            requester: live.conn.requester(),
        });
        self.report(Report::LoggedOn {
            steam_id: ok.steam_id,
            requester: live.conn.requester(),
            ip_country: ok.ip_country.clone().unwrap_or_default(),
            server_time: ok.server_time,
            public_ip: ok.public_ip,
        });
        self.logged_on = true;
        self.retry_at = None;
        self.retry_failures = 0;
        self.reason.clear();
        self.login_required = false;
        Ok(())
    }

    /// Waits for a message of type `emsg`. Account information that
    /// arrives meanwhile is applied; a logoff ends the wait.
    fn wait_for(&mut self, emsg: EMsg, timeout: Duration) -> Result<Packet, LogonFailure> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.stop.is_cancelled() {
                return Err(LogonFailure::Interrupted);
            }
            let Some(live) = &self.live else {
                return Err(LogonFailure::Transient("not connected".into()));
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.drop_connection();
                return Err(LogonFailure::Transient(
                    "Steam did not answer in time".into(),
                ));
            }
            match live.incoming.recv_timeout(left.min(SLICE)) {
                Ok(Incoming::Message(p)) if p.is(emsg) => return Ok(p),
                Ok(Incoming::Message(p)) => {
                    if let Some(r) = logon::parse_logged_off(&p) {
                        self.drop_connection();
                        return Err(LogonFailure::Transient(format!("logged off by Steam: {r}")));
                    }
                    self.on_info(&p);
                }
                Ok(Incoming::Closed(r)) => {
                    self.live = None;
                    return Err(LogonFailure::Transient(r.to_string()));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.live = None;
                    return Err(LogonFailure::Transient("the connection is gone".into()));
                }
            }
        }
    }

    /// Acts on a failed logon of the active account.
    fn after_failed_logon(&mut self, f: &LogonFailure) {
        self.drop_connection();
        match f {
            LogonFailure::Transient(e) => {
                tracing::info!("logon failed: {e}");
                self.reason = format!("logon failed: {e}");
                self.schedule_retry();
            }
            LogonFailure::TokenRejected(e) => {
                tracing::warn!("Steam rejected the stored token: {e}");
                self.forget_active(format!(
                    "Steam rejected the stored token ({e}); log in again"
                ));
            }
            LogonFailure::Refused(e) => {
                tracing::warn!("{e}; not retrying");
                self.reason = e.clone();
                self.retry_at = None;
            }
            LogonFailure::Interrupted => {}
        }
    }

    /// Deletes the active account's token and asks for a new login
    /// (`auth.md` §4, revocation).
    fn forget_active(&mut self, reason: String) {
        if let Some(a) = self.active.take() {
            if a.remembered {
                if let Err(e) = self.secrets().delete(secrets::TOKENS, &a.account_name) {
                    tracing::warn!("could not delete the token of {}: {e}", a.account_name);
                }
                self.report(Report::Forgotten {
                    steam_id: a.steam_id,
                });
            }
            self.report(Report::Active(None));
        }
        self.logged_on = false;
        self.retry_at = None;
        self.reason = reason;
        self.login_required = true;
    }

    /// One reconnection attempt for the active account (NET-4).
    fn reconnect(&mut self) {
        if self.active.is_none() {
            return;
        }
        self.connecting = true;
        self.publish();
        let attempt = self.connect_once();
        self.connecting = false;
        match attempt {
            Err(Attempt::Wait(d)) => self.retry_at = Some(Instant::now() + d),
            Err(Attempt::Failed(e) | Attempt::NoServers(e)) => {
                tracing::info!("reconnection attempt failed: {e}");
                self.reason = format!("cannot reach Steam: {e}");
                self.schedule_retry();
            }
            Ok(()) => {
                if let Err(f) = self.logon() {
                    self.after_failed_logon(&f);
                }
            }
        }
    }

    /// Sends `ClientLogOff` and waits briefly for the server to confirm.
    fn logoff(&mut self) {
        if !self.logged_on {
            return;
        }
        self.logged_on = false;
        let Some(live) = &self.live else {
            return;
        };
        if live.conn.send(logon::logoff_packet()).is_err() {
            return;
        }
        let deadline = Instant::now() + LOGOFF_TIMEOUT;
        while let Ok(m) = live.incoming.recv_deadline(deadline) {
            match m {
                Incoming::Message(p) if logon::parse_logged_off(&p).is_some() => break,
                Incoming::Message(_) => {}
                Incoming::Closed(_) => break,
            }
        }
        tracing::info!("logged off");
    }

    fn resume(&mut self, account: db::Account) {
        match self.secrets().get(secrets::TOKENS, &account.account_name) {
            Ok(Some(token)) => {
                tracing::info!(account = %account.account_name, "resuming the session");
                self.active = Some(Active {
                    steam_id: account.steam_id,
                    account_name: account.account_name,
                    persona_name: account.persona_name,
                    token,
                    remembered: true,
                });
                self.retry_at = Some(Instant::now());
            }
            Ok(None) => {
                self.report(Report::Forgotten {
                    steam_id: account.steam_id,
                });
                self.report(Report::Active(None));
                self.reason = format!(
                    "no token is stored for {}; log in again",
                    account.account_name
                );
                self.login_required = true;
            }
            Err(e) => {
                // The keyring may be locked or absent. The account stays
                // remembered, so a later start can try again.
                tracing::warn!("cannot read the stored token: {e}");
                self.reason = format!(
                    "cannot read the stored token of {}: {e}",
                    account.account_name
                );
            }
        }
    }

    // --- Messages ---------------------------------------------------------

    fn on_message(&mut self, p: &Packet) {
        if let Some(r) = logon::parse_logged_off(p) {
            self.on_logged_off(r);
        } else {
            self.on_info(p);
        }
    }

    fn on_logged_off(&mut self, r: EResult) {
        tracing::warn!(reason = %r, "logged off by Steam");
        self.drop_connection();
        if self.active.is_none() {
            return;
        }
        let as_logon = LogonError::Refused {
            eresult: r,
            extended: None,
        };
        if as_logon.token_rejected() {
            self.forget_active(format!("logged off by Steam ({r}); log in again"));
        } else if REPLACED.contains(&r) {
            self.reason = format!(
                "logged off by Steam ({r}): the account signed in elsewhere. \
                 Use 'ossteam account switch' to sign in here again"
            );
            self.retry_at = None;
        } else {
            self.reason = format!("logged off by Steam ({r})");
            self.schedule_retry();
        }
    }

    /// Applies informational messages.
    fn on_info(&mut self, p: &Packet) {
        if p.is(EMsg::KEMsgClientLicenseList) {
            match ossteam_pics::license::parse(&p.body) {
                Ok(licenses) => {
                    let _ = self.settings.pics.send(pics::Cmd::Licenses(licenses));
                }
                // The last list stays in force (`pics.md` §1).
                Err(e) => tracing::warn!("ignoring a license list: {e}"),
            }
        } else if p.is(EMsg::KEMsgClientAccountInfo) {
            let Ok(info) = p.decode_body::<CMsgClientAccountInfo>() else {
                tracing::debug!("dropping an undecodable ClientAccountInfo");
                return;
            };
            if let (Some(name), Some(active)) = (info.persona_name, self.active.as_mut()) {
                active.persona_name = name.clone();
                let steam_id = active.steam_id;
                self.report(Report::PersonaName { steam_id, name });
            }
        } else if p.is(EMsg::KEMsgClientGameConnectTokens)
            || p.is(EMsg::KEMsgClientAuthListAck)
            || p.is(EMsg::KEMsgClientTicketAuthComplete)
        {
            self.report(Report::Tickets(p.clone()));
        } else if p.is(EMsg::KEMsgClientFromGc) {
            self.report(Report::FromGc(p.clone()));
        } else {
            tracing::trace!(emsg = p.emsg, kind = ?p.kind(), "unhandled CM message");
        }
    }

    /// Sends `ClientChangeStatus` (`pics.md` §8). The state is applied
    /// again after every logon, so a send that fails is not retried.
    fn set_persona_state(&self, steam_id: SteamId, state: db::PersonaState) {
        let (Some(live), Some(active)) = (&self.live, &self.active) else {
            return;
        };
        if !self.logged_on || active.steam_id != steam_id {
            return;
        }
        let body = CMsgClientChangeStatus {
            persona_state: Some(steam_persona_state(state)),
            persona_set_by_user: Some(true),
            ..Default::default()
        };
        match live
            .conn
            .send(Packet::proto(EMsg::KEMsgClientChangeStatus, &body))
        {
            Ok(()) => tracing::info!(?state, "persona state sent"),
            Err(e) => tracing::info!("could not send the persona state: {e}"),
        }
    }

    /// Sends the play session (`launch.md` §6). Like the persona state,
    /// it is sent again after every logon, so a failed send is not retried.
    fn games_played(&self, steam_id: SteamId, body: &CMsgClientGamesPlayed) {
        let (Some(live), Some(active)) = (&self.live, &self.active) else {
            return;
        };
        if !self.logged_on || active.steam_id != steam_id {
            return;
        }
        let apps: Vec<u64> = body.games_played.iter().filter_map(|g| g.game_id).collect();
        match live.conn.send(Packet::proto(
            EMsg::KEMsgClientGamesPlayedWithDataBlob,
            body,
        )) {
            Ok(()) => tracing::info!(?apps, "play session sent"),
            Err(e) => tracing::info!("could not send the play session: {e}"),
        }
    }

    // --- Login (auth.md §2, §3) -------------------------------------------

    fn start_login(&mut self, reply: ReplyTo, method: Method, remember: bool) {
        let started = |r| match &method {
            Method::Credentials { .. } => response::Result::LoginWithCredentials(r),
            Method::Qr => response::Result::LoginWithQr(r),
        };
        let ok = started(v1::LoginStartedResponse {});
        // The core refuses a second login before it gets here; this guards
        // the worker's own invariant. No LoginEnded: the running login
        // still owns that.
        if self.login.is_some() {
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::Conflict,
                    "a login is already in progress",
                )),
            );
            return;
        }
        if remember && let Err(e) = self.secrets().check() {
            self.reply(
                reply,
                Err(RemoteError::new(ErrorCode::Unsupported, e.to_string())),
            );
            self.report(Report::LoginEnded(None));
            return;
        }
        // A new login replaces the active session (auth.md §6). Its token
        // stays stored, so the user can switch back.
        if self.active.is_some() {
            self.logoff();
            self.drop_connection();
            self.active = None;
            self.retry_at = None;
            self.reason.clear();
            self.login_required = false;
            self.report(Report::Active(None));
        }

        let cancel = CancelToken::new();
        *self.login_cancel.lock().unwrap_or_else(|p| p.into_inner()) = cancel.clone();
        self.connecting = true;
        self.publish();
        let connected = self.connect_within(CONNECT_BUDGET, &cancel);
        self.connecting = false;
        // Clients see the connection before the login's first prompt.
        self.publish();
        if let Err(e) = connected {
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::Network,
                    format!("cannot reach Steam: {e}"),
                )),
            );
            self.report(Report::LoginEnded(None));
            return;
        }

        let (username, begun) = match method {
            Method::Credentials { username, password } => {
                let guard_data = self.secrets().get(secrets::GUARD, &username).ok().flatten();
                let persistence = if remember {
                    Persistence::Persistent
                } else {
                    Persistence::Ephemeral
                };
                let live = self.live.as_ref().expect("connected above");
                let begun = auth::begin_credentials(
                    &live.conn,
                    &username,
                    password,
                    persistence,
                    guard_data.as_ref(),
                    &self.identity,
                    &cancel,
                );
                (Some(username), begun)
            }
            Method::Qr => {
                let live = self.live.as_ref().expect("connected above");
                (None, auth::begin_qr(&live.conn, &self.identity, &cancel))
            }
        };
        let pending = match begun {
            Ok(p) => p,
            Err(e) => {
                self.reply(reply, Err(auth_error(e)));
                self.report(Report::LoginEnded(None));
                return;
            }
        };

        self.reply(reply, Ok(ok));
        if let Some(url) = &pending.challenge_url {
            self.report(Report::Prompt(v1::LoginPrompt {
                kind: LoginPromptKind::QrUrl as i32,
                hint: String::new(),
                qr_url: url.clone(),
            }));
        }
        // A QR login is approved by scanning the code (auth.md §3); only
        // credential logins prompt for Steam Guard.
        let prompts = match username {
            Some(_) => guard_prompts(&pending.guards),
            None => Vec::new(),
        };
        // With nothing to wait for, the first poll can go out at once.
        let next_poll = if username.is_some() && prompts.is_empty() {
            Instant::now()
        } else {
            Instant::now() + pending.interval()
        };
        for p in prompts {
            self.report(Report::Prompt(p));
        }
        self.login = Some(Login {
            pending,
            username,
            remember,
            next_poll,
            cancel,
        });
    }

    fn poll_login(&mut self) {
        let (Some(login), Some(live)) = (&mut self.login, &self.live) else {
            return;
        };
        match login.pending.poll(&live.conn, &login.cancel) {
            Ok(Poll::Pending { new_challenge_url }) => {
                if let Some(url) = new_challenge_url {
                    send_report(
                        &self.core,
                        Report::Prompt(v1::LoginPrompt {
                            kind: LoginPromptKind::QrUrl as i32,
                            hint: String::new(),
                            qr_url: url,
                        }),
                    );
                }
                login.next_poll = Instant::now() + login.pending.interval();
            }
            Ok(Poll::Done(tokens)) => {
                let login = self.login.take().expect("matched above");
                self.finish_login(&login, tokens);
            }
            // One slow poll does not end the login; the next may answer.
            Err(AuthError::Call(CallError::Timeout)) => {
                login.next_poll = Instant::now() + login.pending.interval();
            }
            Err(e) => self.end_login(Err(auth_error(e))),
        }
    }

    fn finish_login(&mut self, login: &Login, tokens: Tokens) {
        // Steam's spelling of the name wins over what the user typed.
        let name = Some(tokens.account_name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| login.username.clone())
            .unwrap_or_default();
        if login.remember {
            if let Err(e) = self
                .secrets()
                .set(secrets::TOKENS, &name, &tokens.refresh_token)
            {
                self.end_login(Err(RemoteError::new(
                    ErrorCode::Internal,
                    format!("signed in, but the token could not be stored: {e}"),
                )));
                return;
            }
            if let Some(g) = &tokens.new_guard_data
                && let Err(e) = self.secrets().set(secrets::GUARD, &name, g)
            {
                tracing::warn!("could not store guard data: {e}");
            }
            self.report(Report::Remembered {
                steam_id: tokens.steam_id,
                account_name: name.clone(),
            });
            self.report(Report::Active(Some(tokens.steam_id)));
        }
        tracing::info!(account = %name, remember = login.remember, "signed in");
        self.active = Some(Active {
            steam_id: tokens.steam_id,
            account_name: name,
            persona_name: String::new(),
            token: tokens.refresh_token,
            remembered: login.remember,
        });
        self.reason.clear();
        self.login_required = false;
        match self.logon() {
            Ok(()) => {
                let account = self.active.as_ref().map(Active::proto);
                self.report_login_ended(
                    account.ok_or_else(|| {
                        RemoteError::new(ErrorCode::Internal, "the account vanished")
                    }),
                );
            }
            Err(f) => {
                let mut err = f.error();
                if matches!(f, LogonFailure::Transient(_)) {
                    err.message = format!("signed in, but {}", err.message);
                }
                self.after_failed_logon(&f);
                self.report_login_ended(Err(err));
            }
        }
    }

    fn end_login(&mut self, result: Result<v1::Account, RemoteError>) {
        self.login = None;
        self.report_login_ended(result);
    }

    fn report_login_ended(&self, result: Result<v1::Account, RemoteError>) {
        let ev = match result {
            Ok(a) => v1::LoginEnded {
                account: Some(a),
                error: None,
            },
            Err(e) => {
                tracing::info!("login ended: {e}");
                v1::LoginEnded {
                    account: None,
                    error: Some(e.into()),
                }
            }
        };
        self.report(Report::LoginEnded(Some(ev)));
    }

    fn submit_code(&mut self, reply: ReplyTo, code: Secret, kind: LoginPromptKind) {
        let Some(login) = &mut self.login else {
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::Conflict,
                    "no login is waiting for a code",
                )),
            );
            return;
        };
        let offered: Vec<CodeKind> = login.pending.guards.iter().filter_map(code_kind).collect();
        let chosen = match kind {
            LoginPromptKind::EmailCode => Some(CodeKind::Email),
            LoginPromptKind::DeviceCode => Some(CodeKind::Device),
            LoginPromptKind::Unspecified if offered.len() == 1 => Some(offered[0]),
            _ => None,
        };
        let Some(chosen) = chosen.filter(|k| offered.contains(k)) else {
            let message = if offered.is_empty() {
                "this login does not take a code".to_owned()
            } else {
                format!("this login takes {offered:?} codes; say which kind this is")
            };
            send_report(
                &self.core,
                Report::Reply {
                    to: reply,
                    result: Box::new(Err(RemoteError::new(ErrorCode::InvalidArgument, message))),
                },
            );
            return;
        };
        let Some(live) = &self.live else {
            send_report(
                &self.core,
                Report::Reply {
                    to: reply,
                    result: Box::new(Err(RemoteError::new(
                        ErrorCode::Network,
                        "not connected to Steam",
                    ))),
                },
            );
            return;
        };
        let result = match login
            .pending
            .submit_code(&live.conn, code, chosen, &login.cancel)
        {
            Ok(()) => {
                // The code may have completed the sign-in; look now.
                login.next_poll = Instant::now();
                Ok(response::Result::SubmitGuardCode(
                    v1::SubmitGuardCodeResponse {},
                ))
            }
            Err(AuthError::Call(CallError::Failed { eresult, .. }))
                if BAD_CODE.contains(&eresult) =>
            {
                Err(RemoteError::new(
                    ErrorCode::NotLoggedIn,
                    format!("Steam rejected the code: {eresult}"),
                ))
            }
            Err(e) => Err(auth_error(e)),
        };
        send_report(
            &self.core,
            Report::Reply {
                to: reply,
                result: Box::new(result),
            },
        );
    }

    // --- Accounts (auth.md §6) --------------------------------------------

    fn logout(&mut self, reply: ReplyTo) {
        if self.active.is_none() {
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::NotLoggedIn,
                    "no account is logged in",
                )),
            );
            return;
        }
        self.logoff();
        self.drop_connection();
        let a = self.active.take().expect("checked above");
        let mut result = Ok(response::Result::Logout(v1::LogoutResponse {}));
        if a.remembered {
            if let Err(e) = self.secrets().delete(secrets::TOKENS, &a.account_name) {
                result = Err(RemoteError::new(
                    ErrorCode::Internal,
                    format!("logged off, but the stored token could not be deleted: {e}"),
                ));
            }
            self.report(Report::Forgotten {
                steam_id: a.steam_id,
            });
        }
        tracing::info!(account = %a.account_name, "logged out");
        self.report(Report::Active(None));
        self.retry_at = None;
        self.reason = format!("{} logged out", a.account_name);
        self.login_required = false;
        self.reply(reply, result);
    }

    fn switch(&mut self, reply: ReplyTo, account: db::Account) {
        if self.login.is_some() {
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::Conflict,
                    "a login is in progress",
                )),
            );
            return;
        }
        if let Some(a) = &self.active
            && a.steam_id == account.steam_id
            && self.logged_on
        {
            let r = v1::SwitchAccountResponse {
                account: Some(a.proto()),
            };
            self.reply(reply, Ok(response::Result::SwitchAccount(r)));
            return;
        }
        let token = match self.secrets().get(secrets::TOKENS, &account.account_name) {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.report(Report::Forgotten {
                    steam_id: account.steam_id,
                });
                self.reply(
                    reply,
                    Err(RemoteError::new(
                        ErrorCode::NotLoggedIn,
                        format!(
                            "no token is stored for {}; log in again",
                            account.account_name
                        ),
                    )),
                );
                return;
            }
            Err(e) => {
                self.reply(
                    reply,
                    Err(RemoteError::new(ErrorCode::Internal, e.to_string())),
                );
                return;
            }
        };
        self.logoff();
        self.drop_connection();
        tracing::info!(account = %account.account_name, "switching account");
        self.active = Some(Active {
            steam_id: account.steam_id,
            account_name: account.account_name,
            persona_name: account.persona_name,
            token,
            remembered: true,
        });
        self.report(Report::Active(Some(account.steam_id)));
        self.reason.clear();
        self.login_required = false;
        self.retry_at = None;
        self.retry_failures = 0;

        self.connecting = true;
        self.publish();
        let stop = self.stop.clone();
        let connected = self.connect_within(CONNECT_BUDGET, &stop);
        self.connecting = false;
        if let Err(e) = connected {
            self.reason = format!("cannot reach Steam: {e}");
            self.schedule_retry();
            self.reply(
                reply,
                Err(RemoteError::new(
                    ErrorCode::Network,
                    format!("cannot reach Steam ({e}); the daemon keeps trying"),
                )),
            );
            return;
        }
        let result = match self.logon() {
            Ok(()) => Ok(response::Result::SwitchAccount(v1::SwitchAccountResponse {
                account: self.active.as_ref().map(Active::proto),
            })),
            Err(f) => {
                self.after_failed_logon(&f);
                Err(f.error())
            }
        };
        self.reply(reply, result);
    }
}

/// The prompts for a credential login's guard types (`auth.md` §2 step 4).
fn guard_prompts(guards: &[Guard]) -> Vec<v1::LoginPrompt> {
    let prompt = |kind: LoginPromptKind, hint: &str| v1::LoginPrompt {
        kind: kind as i32,
        hint: hint.into(),
        qr_url: String::new(),
    };
    guards
        .iter()
        .filter_map(|g| match g {
            Guard::EmailCode { hint } => Some(prompt(
                LoginPromptKind::EmailCode,
                hint.as_deref().unwrap_or_default(),
            )),
            Guard::DeviceCode => Some(prompt(LoginPromptKind::DeviceCode, "")),
            Guard::DeviceConfirmation => Some(prompt(LoginPromptKind::DeviceConfirmation, "")),
            Guard::EmailConfirmation => Some(prompt(
                LoginPromptKind::DeviceConfirmation,
                "confirm in email",
            )),
            Guard::None | Guard::MachineToken => None,
            Guard::Other(n) => {
                tracing::warn!(
                    kind = n,
                    "ignoring a Steam Guard type this build does not know"
                );
                None
            }
        })
        .collect()
}

fn code_kind(g: &Guard) -> Option<CodeKind> {
    match g {
        Guard::EmailCode { .. } => Some(CodeKind::Email),
        Guard::DeviceCode => Some(CodeKind::Device),
        _ => None,
    }
}

/// Maps a sign-in failure to an IPC error (exit codes 4 and 9).
fn auth_error(e: AuthError) -> RemoteError {
    match e {
        AuthError::Call(CallError::Failed { eresult, message }) => {
            let code = if BUSY.contains(&eresult) {
                ErrorCode::Network
            } else {
                ErrorCode::NotLoggedIn
            };
            RemoteError::new(code, format!("Steam refused the sign-in: {eresult}"))
                .with_detail(message.unwrap_or_default())
        }
        AuthError::Call(CallError::Cancelled) => {
            RemoteError::new(ErrorCode::NotLoggedIn, "the login was cancelled")
        }
        AuthError::Call(e @ (CallError::Closed | CallError::Timeout)) => {
            RemoteError::new(ErrorCode::Network, format!("sign-in failed: {e}"))
        }
        other => RemoteError::new(ErrorCode::Internal, format!("sign-in failed: {other}")),
    }
}

/// Steam's number for a persona state (`pics.md` §8).
fn steam_persona_state(state: db::PersonaState) -> u32 {
    match state {
        db::PersonaState::Offline => 0,
        db::PersonaState::Online => 1,
        db::PersonaState::Invisible => 7,
    }
}

/// The name shown in the Steam mobile app for this device (`auth.md` §1).
fn host_name() -> String {
    #[cfg(unix)]
    let name = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    if name.is_empty() {
        "ossteam".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_types_become_prompts() {
        let prompts = guard_prompts(&[
            Guard::DeviceConfirmation,
            Guard::DeviceCode,
            Guard::EmailCode {
                hint: Some("example.com".into()),
            },
            Guard::EmailConfirmation,
            Guard::MachineToken,
            Guard::None,
            Guard::Other(99),
        ]);
        let kinds: Vec<_> = prompts.iter().map(|p| p.kind()).collect();
        assert_eq!(
            kinds,
            [
                LoginPromptKind::DeviceConfirmation,
                LoginPromptKind::DeviceCode,
                LoginPromptKind::EmailCode,
                LoginPromptKind::DeviceConfirmation,
            ]
        );
        assert_eq!(prompts[2].hint, "example.com");
        assert_eq!(prompts[3].hint, "confirm in email");
    }

    #[test]
    fn sign_in_failures_map_to_exit_codes() {
        let failed = |eresult| {
            AuthError::Call(CallError::Failed {
                eresult,
                message: None,
            })
        };
        assert_eq!(
            auth_error(failed(EResult::INVALID_PASSWORD)).code,
            ErrorCode::NotLoggedIn
        );
        assert_eq!(
            auth_error(failed(EResult::RATE_LIMIT_EXCEEDED)).code,
            ErrorCode::Network
        );
        assert_eq!(
            auth_error(AuthError::Call(CallError::Timeout)).code,
            ErrorCode::Network
        );
    }

    #[test]
    fn host_name_is_never_empty() {
        assert!(!host_name().is_empty());
    }
}
