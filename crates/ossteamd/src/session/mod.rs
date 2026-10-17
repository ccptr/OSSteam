// SPDX-License-Identifier: GPL-2.0-or-later
//! The session subsystem: the CM connection, sign-in, logon, reconnection
//! and token storage (SPEC §9.1, §9.2; `cm.md` §4 and §5; `auth.md`).
//!
//! One worker thread owns the connection and the keyring, because both
//! block: network calls take seconds and a locked keyring may wait for the
//! user. The core thread sends it [`Cmd`]s and receives [`Report`]s on its
//! command channel. The core keeps the database and answers status
//! queries from the last reported [`State`] (CONC-1).

mod servers;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ConnectionState, response};
use ossteam_net::{CmServer, ConnectError, Connection, Incoming};
use ossteam_types::{CancelToken, Secret, SteamId};

pub use servers::{NoServer, Servers};

use crate::core::{ClientId, Command};
use crate::secrets::Secrets;
use crate::{db, pics};

/// How the worker finds and reaches CM servers. The daemon uses
/// [`Servers`]: the Steam Directory and the WebSocket transport. Only test
/// code has another implementation, a fake CM (FAITH-4).
pub trait Dialer: Send {
    /// The next server to try.
    fn next(&mut self, cell_id: u32) -> Result<CmServer, NoServer>;
    fn failed(&mut self, endpoint: &str);
    fn succeeded(&mut self, endpoint: &str);
    fn connect(
        &mut self,
        server: &CmServer,
        timeout: Duration,
        events: Sender<Incoming>,
        cancel: &CancelToken,
    ) -> Result<Connection, ConnectError>;
}

/// Where a deferred reply goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyTo {
    pub client: ClientId,
    pub id: u64,
}

pub enum Cmd {
    LoginWithCredentials {
        reply: ReplyTo,
        username: String,
        password: Secret,
        remember: bool,
    },
    LoginWithQr {
        reply: ReplyTo,
        remember: bool,
    },
    SubmitGuardCode {
        reply: ReplyTo,
        code: Secret,
        kind: v1::LoginPromptKind,
    },
    /// `reply` is `None` when the core cancels on its own, because the
    /// client that started the login went away.
    CancelLogin {
        reply: Option<ReplyTo>,
    },
    Logout {
        reply: ReplyTo,
    },
    SwitchAccount {
        reply: ReplyTo,
        account: db::Account,
    },
    /// Tells Steam the persona state, if the account is logged on
    /// (`pics.md` §8).
    SetPersonaState {
        steam_id: SteamId,
        state: db::PersonaState,
    },
    /// `secrets.file_fallback` changed (SPEC D-8).
    SetFileFallback(bool),
}

/// What the worker tells the core.
pub enum Report {
    Reply {
        to: ReplyTo,
        /// Boxed: a response is far larger than the other reports.
        result: Box<Result<response::Result, RemoteError>>,
    },
    State(State),
    Prompt(v1::LoginPrompt),
    /// A login is over. The event is `None` when the login failed before it
    /// started; the reply to the login request said why.
    LoginEnded(Option<v1::LoginEnded>),
    /// A refresh token for this account is now stored.
    Remembered {
        steam_id: SteamId,
        account_name: String,
    },
    /// This account's stored token is gone.
    Forgotten {
        steam_id: SteamId,
    },
    /// The account to resume on the next start, if any.
    Active(Option<SteamId>),
    CellId(u32),
    /// The account logged on; its persona state is due (`pics.md` §8).
    /// Content jobs send their requests through `requester` until it
    /// closes.
    LoggedOn {
        steam_id: SteamId,
        requester: ossteam_net::Requester,
    },
    PersonaName {
        steam_id: SteamId,
        name: String,
    },
}

/// The session as clients see it (`GetStatus`, `SessionChanged`).
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub connection: ConnectionState,
    pub account: Option<v1::Account>,
    pub logged_on: bool,
    pub reason: String,
    pub login_required: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            account: None,
            logged_on: false,
            reason: String::new(),
            login_required: false,
        }
    }
}

impl State {
    pub fn event(&self) -> v1::SessionChanged {
        v1::SessionChanged {
            connection: self.connection as i32,
            account: self.account.clone(),
            logged_on: self.logged_on,
            reason: self.reason.clone(),
            login_required: self.login_required,
            // The core fills this in: the choice is in its database.
            persona_state: v1::PersonaState::Unspecified as i32,
        }
    }
}

pub struct Settings {
    pub data_dir: PathBuf,
    pub file_fallback: bool,
    pub cell_id: u32,
    /// Opens token storage; [`Secrets::open`] in the daemon. Called again
    /// when `secrets.file_fallback` changes.
    pub open_secrets: fn(file_fallback: bool, data_dir: &Path) -> Secrets,
    /// The account to log on at start (`cm.md` §5, AUTH-3).
    pub resume: Option<db::Account>,
    /// The PICS worker, which follows the active account and its licenses.
    pub pics: Sender<pics::Cmd>,
}

/// How long shutdown waits for a graceful logoff before it interrupts
/// whatever the worker is blocked on.
const GRACEFUL_STOP: Duration = Duration::from_secs(3);
const FORCED_STOP: Duration = Duration::from_secs(2);

/// The core's side of the worker.
pub struct Handle {
    tx: Option<Sender<Cmd>>,
    /// The current login's token, so a cancel takes effect even while the
    /// worker is blocked in a call for that login.
    login_cancel: Arc<Mutex<CancelToken>>,
    /// Interrupts everything; used only when a graceful stop takes too long.
    stop: CancelToken,
    /// Disconnects when the worker exits.
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn spawn(
        settings: Settings,
        dialer: Box<dyn Dialer>,
        core: Sender<Command>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        let login_cancel = Arc::new(Mutex::new(CancelToken::new()));
        let stop = CancelToken::new();
        let worker =
            worker::Worker::new(settings, dialer, core, login_cancel.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("session".into())
            .spawn(move || {
                let _done = done_tx;
                worker.run(&rx);
            })?;
        Ok(Self {
            tx: Some(tx),
            login_cancel,
            stop,
            done,
            thread: Some(thread),
        })
    }

    pub fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn cancel_login(&self, reply: Option<ReplyTo>) {
        self.login_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
        self.send(Cmd::CancelLogin { reply });
    }

    /// Stops the worker: it logs off and closes the connection (SPEC D-5).
    /// If that takes too long, everything it waits on is interrupted.
    /// Later calls do nothing.
    pub fn shutdown(&mut self) {
        if self.tx.is_none() {
            return;
        }
        self.login_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel();
        // Closing the channel is the stop signal.
        self.tx.take();
        if self.done.recv_timeout(GRACEFUL_STOP) == Err(RecvTimeoutError::Timeout) {
            tracing::warn!("the session did not stop in time; interrupting it");
            self.stop.cancel();
            if self.done.recv_timeout(FORCED_STOP) == Err(RecvTimeoutError::Timeout) {
                // Blocked somewhere that cannot be interrupted, such as a
                // host name lookup. The process exits without it.
                tracing::warn!("the session is still busy; leaving it");
                return;
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
