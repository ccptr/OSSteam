// SPDX-License-Identifier: GPL-2.0-or-later
//! The daemon connection (`gui.md` §2). One `Client` is shared by a reader
//! thread, which hands each event to a callback, and by a pool of call
//! threads, which hand each result to the callback the call came with
//! (SPEC CONC-3). The GUI's callbacks send them to the main thread as
//! messages; the daemon's tests take them from a channel.

use std::sync::Arc;
use std::thread;

use ossteam_ipc::v1::{self, ErrorCode, PeerKind, Topic, event};
use ossteam_ipc::{Client, ClientError, ClientInfo, ConnectOptions, Endpoint};

/// Call threads; a call can block for its whole timeout (§2.1).
const CALL_THREADS: usize = 4;

const TOPICS: [Topic; 6] = [
    Topic::Daemon,
    Topic::Session,
    Topic::Library,
    Topic::Jobs,
    Topic::Games,
    Topic::Notices,
];

pub enum Update {
    /// Connected, subscribed, and the initial state read (§2.2). Events
    /// that arrived meanwhile follow as [`Update::Event`]s.
    Ready(Daemon, Box<Initial>),
    Event(event::Kind),
    /// The connection ended, or could not be made; why, in words.
    Closed(String),
}

/// What the GUI reads after subscribing (§2.2). `library` and `jobs` are
/// `None` without an active account.
pub struct Initial {
    pub status: v1::GetStatusResponse,
    pub version: v1::GetVersionResponse,
    pub accounts: v1::ListAccountsResponse,
    pub library: Option<v1::ListAppsResponse>,
    pub jobs: Option<Vec<v1::Job>>,
    pub running: Vec<v1::RunningGame>,
    pub settings: Vec<v1::Setting>,
    pub folders: Vec<v1::LibraryFolder>,
}

/// A failed call: the daemon's `Error`, or what went wrong on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl CallError {
    fn disconnected() -> Self {
        ClientError::Disconnected.into()
    }

    pub fn is(&self, code: ErrorCode) -> bool {
        self.code == Some(code)
    }
}

impl From<ClientError> for CallError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Remote(r) => Self {
                code: Some(r.code),
                message: r.message,
            },
            other => Self {
                code: None,
                message: other.to_string(),
            },
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A call for a call thread; `None` when it cannot be made.
type CallJob = Box<dyn FnOnce(Option<&Client>) + Send>;

/// A handle for making calls. Clones share the connection; the call
/// threads end when the last clone is dropped.
#[derive(Clone)]
pub struct Daemon {
    client: Arc<Client>,
    calls: crossbeam_channel::Sender<CallJob>,
}

impl Daemon {
    fn new(client: Arc<Client>) -> Self {
        let (calls, jobs) = crossbeam_channel::unbounded::<CallJob>();
        for n in 0..CALL_THREADS {
            let jobs = jobs.clone();
            let client = client.clone();
            let spawned = thread::Builder::new()
                .name(format!("daemon-call-{n}"))
                .spawn(move || {
                    for job in jobs {
                        job(Some(&client));
                    }
                });
            if let Err(e) = spawned {
                eprintln!("ossteam-gui: cannot start a call thread: {e}");
            }
        }
        Self { client, calls }
    }

    /// Runs `f` on a call thread and hands its result to `done`, on that
    /// thread; with no call thread left, `done` gets the disconnection at
    /// once.
    pub fn call<T, F>(&self, f: F, done: impl FnOnce(Result<T, CallError>) + Send + 'static)
    where
        F: FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    {
        let job: CallJob = Box::new(move |client| {
            done(match client {
                Some(c) => f(c).map_err(CallError::from),
                None => Err(CallError::disconnected()),
            })
        });
        if let Err(crossbeam_channel::SendError(job)) = self.calls.send(job) {
            job(None);
        }
    }

    /// Like [`call`](Self::call), on a thread of its own, for calls that
    /// wait without a timeout (`Launch`, §2.1).
    pub fn call_unbounded<T, F>(
        &self,
        f: F,
        done: impl FnOnce(Result<T, CallError>) + Send + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    {
        let client = self.client.clone();
        let done = Arc::new(std::sync::Mutex::new(Some(done)));
        let on_error = done.clone();
        let spawned = thread::Builder::new()
            .name("daemon-launch".into())
            .spawn(move || {
                let r = f(&client).map_err(CallError::from);
                if let Some(done) = done.lock().ok().and_then(|mut d| d.take()) {
                    done(r);
                }
            });
        if let Err(e) = spawned
            && let Some(done) = on_error.lock().ok().and_then(|mut d| d.take())
        {
            done(Err(CallError {
                code: None,
                message: format!("cannot start a thread for the call: {e}"),
            }));
        }
    }
}

/// Hands an update to whoever follows the connection; `false` once
/// nobody does, which ends the reader thread.
pub trait OnUpdate: Fn(Update) -> bool + Send + Sync + 'static {}
impl<F: Fn(Update) -> bool + Send + Sync + 'static> OnUpdate for F {}

/// Connects on a new thread (starting the daemon unless `autostart` is
/// off) and reports what happens until the connection ends.
pub fn connect(autostart: bool, on_update: impl OnUpdate) {
    spawn(
        move |info| {
            let mut opts = ConnectOptions::new(info);
            opts.autostart = autostart;
            Client::connect(&opts)
        },
        on_update,
    );
}

/// As [`connect`], to the daemon at `endpoint` and without starting one.
pub fn connect_endpoint(endpoint: Endpoint, on_update: impl OnUpdate) {
    spawn(
        move |info| Client::connect_endpoint(&endpoint, &info),
        on_update,
    );
}

fn spawn(
    connect: impl FnOnce(ClientInfo) -> Result<Client, ClientError> + Send + 'static,
    on_update: impl OnUpdate,
) {
    let on_update = Arc::new(on_update);
    let thread_update = on_update.clone();
    let spawned = thread::Builder::new()
        .name("daemon-connection".into())
        .spawn(move || run(connect, &*thread_update));
    if let Err(e) = spawned {
        on_update(Update::Closed(format!(
            "cannot start the connection thread: {e}"
        )));
    }
}

fn run(
    connect: impl FnOnce(ClientInfo) -> Result<Client, ClientError>,
    on_update: &dyn Fn(Update) -> bool,
) {
    let info = ClientInfo {
        kind: PeerKind::Gui,
        name: "ossteam-gui".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let client = match connect(info) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            on_update(Update::Closed(e.to_string()));
            return;
        }
    };
    let initial = match client
        .subscribe(&TOPICS)
        .and_then(|_| read_initial(&client))
    {
        Ok(i) => i,
        Err(e) => {
            on_update(Update::Closed(e.to_string()));
            return;
        }
    };
    if !on_update(Update::Ready(
        Daemon::new(client.clone()),
        Box::new(initial),
    )) {
        return;
    }
    let reason = loop {
        let Some(e) = client.next_event() else {
            break "the connection to ossteamd was lost".to_owned();
        };
        match e.kind {
            Some(event::Kind::DaemonStopping(_)) => break "ossteamd stopped".to_owned(),
            Some(kind) => {
                if !on_update(Update::Event(kind)) {
                    return;
                }
            }
            // An event without a kind is from a newer daemon; skip it.
            None => {}
        }
    };
    on_update(Update::Closed(reason));
}

fn read_initial(c: &Client) -> Result<Initial, ClientError> {
    Ok(Initial {
        status: c.get_status()?,
        version: c.get_version()?,
        accounts: c.list_accounts()?,
        library: unless_logged_out(c.list_apps("", false))?,
        jobs: unless_logged_out(c.list_jobs())?,
        running: c.list_running()?,
        settings: c.get_settings()?,
        folders: c.list_library_folders()?,
    })
}

/// `None` for `NOT_LOGGED_IN`: the call needs an active account.
pub fn unless_logged_out<T>(r: Result<T, ClientError>) -> Result<Option<T>, ClientError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ClientError::Remote(e)) if e.code == ErrorCode::NotLoggedIn => Ok(None),
        Err(e) => Err(e),
    }
}
