// SPDX-License-Identifier: GPL-2.0-or-later
//! Blocking client library.
//!
//! A [`Client`] performs the `Hello` handshake, then hands the read half of
//! the connection to one reader thread. That thread routes each response to
//! the call waiting for its id and queues events for [`Client::next_event`].
//! Calls may be made from any number of threads at once.

use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use interprocess::local_socket::Stream;
use ossteam_types::Secret;

use crate::endpoint::Endpoint;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::v1::{
    self, ErrorCode, Event, Hello, PeerKind, Topic, envelope::Body, request, response,
};
use crate::{PROTOCOL_VERSION, RemoteError};

/// How long a call waits for its response by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a call that waits on Steam may take: starting a login or
/// switching accounts may first have to try several servers.
pub const STEAM_TIMEOUT: Duration = Duration::from_secs(90);

/// Who is connecting; sent in the `Hello`.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub kind: PeerKind,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub info: ClientInfo,
    /// Start the daemon if it is not running (SPEC D-2).
    pub autostart: bool,
    /// How long to wait for a freshly started daemon to accept connections.
    pub start_timeout: Duration,
}

impl ConnectOptions {
    pub fn new(info: ClientInfo) -> Self {
        Self {
            info,
            autostart: true,
            start_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the daemon is not reachable at {address}: {source}")]
    Unavailable {
        address: String,
        #[source]
        source: io::Error,
    },
    #[error("could not start the daemon: {0}")]
    StartFailed(String),
    #[error("{0}")]
    Remote(#[from] RemoteError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("the connection to the daemon was lost")]
    Disconnected,
    #[error("timed out waiting for the daemon")]
    Timeout,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

type Reply = Result<response::Result, RemoteError>;

struct Inner {
    /// The connection, which the reader thread reads at the same time:
    /// `&Stream` reads and writes. The lock keeps frames whole.
    writer: Mutex<Arc<Stream>>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
}

/// A connection to the daemon.
pub struct Client {
    inner: Arc<Inner>,
    events: Receiver<Event>,
    server: Hello,
    spawned_pid: Option<u32>,
}

impl Client {
    /// Connects, starting the daemon first if allowed and needed.
    pub fn connect(opts: &ConnectOptions) -> Result<Self, ClientError> {
        let ep = Endpoint::client().map_err(|source| ClientError::Unavailable {
            address: "(unknown)".into(),
            source,
        })?;
        match ep.connect() {
            Ok(stream) => Self::handshake(stream, &opts.info),
            Err(e) if opts.autostart && daemon_absent(&e) => {
                let (stream, pid) = start_daemon(&ep, opts.start_timeout)?;
                let mut client = Self::handshake(stream, &opts.info)?;
                client.spawned_pid = Some(pid);
                Ok(client)
            }
            Err(source) => Err(ClientError::Unavailable {
                address: ep.address,
                source,
            }),
        }
    }

    /// Connects to the daemon listening at `ep`, without starting one.
    pub fn connect_endpoint(ep: &Endpoint, info: &ClientInfo) -> Result<Self, ClientError> {
        let stream = ep.connect().map_err(|source| ClientError::Unavailable {
            address: ep.address.clone(),
            source,
        })?;
        Self::handshake(stream, info)
    }

    fn handshake(stream: Stream, info: &ClientInfo) -> Result<Self, ClientError> {
        let stream = Arc::new(stream);
        let hello = Hello {
            protocol: Some(PROTOCOL_VERSION),
            kind: info.kind as i32,
            name: info.name.clone(),
            version: info.version.clone(),
        };
        write_frame(
            &mut &*stream,
            &v1::Envelope {
                id: 0,
                body: Some(Body::Hello(hello)),
            },
        )?;
        let server = match read_frame(&mut &*stream)? {
            v1::Envelope {
                body: Some(Body::Hello(h)),
                ..
            } => h,
            v1::Envelope {
                body: Some(Body::Error(e)),
                ..
            } => return Err(RemoteError::from(e).into()),
            other => {
                return Err(ClientError::Protocol(format!(
                    "expected Hello, got {other:?}"
                )));
            }
        };
        let major = server.protocol.map(|p| p.major);
        if major != Some(PROTOCOL_VERSION.major) {
            return Err(ClientError::Protocol(format!(
                "daemon speaks protocol major {major:?}, this client {}",
                PROTOCOL_VERSION.major
            )));
        }

        let inner = Arc::new(Inner {
            writer: Mutex::new(Arc::clone(&stream)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let (ev_tx, ev_rx) = unbounded();
        let reader_inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("ossteam-ipc-reader".into())
            .spawn(move || reader_loop(&stream, &reader_inner, &ev_tx))
            .map_err(|e| ClientError::Protocol(format!("spawn reader thread: {e}")))?;
        Ok(Self {
            inner,
            events: ev_rx,
            server,
            spawned_pid: None,
        })
    }

    /// The PID of the daemon process this connection spawned, if it spawned
    /// one. When several clients start the daemon at once only one process
    /// survives, so compare this with the PID in `GetStatus` before claiming
    /// to have started the daemon.
    pub fn spawned_daemon_pid(&self) -> Option<u32> {
        self.spawned_pid
    }

    /// The daemon's `Hello`.
    pub fn server_hello(&self) -> &Hello {
        &self.server
    }

    /// Sends a request and waits for its response.
    pub fn call(
        &self,
        method: request::Method,
        timeout: Duration,
    ) -> Result<response::Result, ClientError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(ClientError::Disconnected);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = bounded(1);
        self.inner.pending.lock().unwrap().insert(id, tx);
        let mut env = v1::Envelope {
            id,
            body: Some(Body::Request(v1::Request {
                method: Some(method),
            })),
        };
        let sent = write_frame(&mut &**self.inner.writer.lock().unwrap(), &env);
        scrub(&mut env);
        if let Err(e) = sent {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(e.into());
        }
        match rx.recv_timeout(timeout) {
            Ok(reply) => Ok(reply?),
            Err(RecvTimeoutError::Timeout) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err(ClientError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected),
        }
    }

    /// Waits for the next event on a subscribed topic. Returns `None` once
    /// the connection is closed and every queued event has been taken.
    pub fn next_event(&self) -> Option<Event> {
        self.events.recv().ok()
    }

    /// The event queue, for use with `select!` or from another thread.
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// Waits until the daemon closes the connection, discarding any events
    /// that arrive meanwhile. Returns false on timeout.
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.events.recv_deadline(deadline) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => return true,
                Err(RecvTimeoutError::Timeout) => return self.is_closed(),
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    pub fn get_status(&self) -> Result<v1::GetStatusResponse, ClientError> {
        match self.call(
            request::Method::GetStatus(v1::GetStatusRequest {}),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::GetStatus(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn get_version(&self) -> Result<v1::GetVersionResponse, ClientError> {
        match self.call(
            request::Method::GetVersion(v1::GetVersionRequest {}),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::GetVersion(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn shutdown(&self, force: bool) -> Result<(), ClientError> {
        match self.call(
            request::Method::Shutdown(v1::ShutdownRequest { force }),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::Shutdown(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn subscribe(&self, topics: &[Topic]) -> Result<Vec<Topic>, ClientError> {
        let req = v1::SubscribeRequest {
            topics: topics.iter().map(|t| *t as i32).collect(),
        };
        match self.call(request::Method::Subscribe(req), DEFAULT_TIMEOUT)? {
            response::Result::Subscribe(r) => Ok(topics_from(&r.topics)),
            other => Err(unexpected(other)),
        }
    }

    pub fn unsubscribe(&self, topics: &[Topic]) -> Result<Vec<Topic>, ClientError> {
        let req = v1::UnsubscribeRequest {
            topics: topics.iter().map(|t| *t as i32).collect(),
        };
        match self.call(request::Method::Unsubscribe(req), DEFAULT_TIMEOUT)? {
            response::Result::Unsubscribe(r) => Ok(topics_from(&r.topics)),
            other => Err(unexpected(other)),
        }
    }
}

/// Session calls (`docs/spec/ipc.md` §5.2).
impl Client {
    /// Starts a credential login. Subscribe to [`Topic::Session`] first:
    /// the prompts and the outcome arrive as events.
    pub fn login_with_credentials(
        &self,
        username: &str,
        password: &Secret,
        remember: bool,
    ) -> Result<(), ClientError> {
        let req = v1::LoginWithCredentialsRequest {
            username: username.into(),
            password: password.expose().into(),
            remember,
        };
        match self.call(request::Method::LoginWithCredentials(req), STEAM_TIMEOUT)? {
            response::Result::LoginWithCredentials(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Starts a QR login; see [`login_with_credentials`](Self::login_with_credentials).
    pub fn login_with_qr(&self, remember: bool) -> Result<(), ClientError> {
        let req = v1::LoginWithQrRequest { remember };
        match self.call(request::Method::LoginWithQr(req), STEAM_TIMEOUT)? {
            response::Result::LoginWithQr(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn submit_guard_code(
        &self,
        code: &Secret,
        kind: v1::LoginPromptKind,
    ) -> Result<(), ClientError> {
        let req = v1::SubmitGuardCodeRequest {
            code: code.expose().into(),
            kind: kind as i32,
        };
        match self.call(request::Method::SubmitGuardCode(req), STEAM_TIMEOUT)? {
            response::Result::SubmitGuardCode(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn cancel_login(&self) -> Result<(), ClientError> {
        match self.call(
            request::Method::CancelLogin(v1::CancelLoginRequest {}),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::CancelLogin(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn logout(&self) -> Result<(), ClientError> {
        match self.call(request::Method::Logout(v1::LogoutRequest {}), STEAM_TIMEOUT)? {
            response::Result::Logout(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn list_accounts(&self) -> Result<v1::ListAccountsResponse, ClientError> {
        match self.call(
            request::Method::ListAccounts(v1::ListAccountsRequest {}),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::ListAccounts(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn switch_account(&self, steam_id: u64) -> Result<v1::Account, ClientError> {
        let req = v1::SwitchAccountRequest { steam_id };
        match self.call(request::Method::SwitchAccount(req), STEAM_TIMEOUT)? {
            response::Result::SwitchAccount(r) => r
                .account
                .ok_or_else(|| ClientError::Protocol("switch response has no account".into())),
            other => Err(unexpected(other)),
        }
    }

    pub fn set_persona_state(&self, state: v1::PersonaState) -> Result<(), ClientError> {
        let req = v1::SetPersonaStateRequest {
            state: state as i32,
        };
        match self.call(request::Method::SetPersonaState(req), DEFAULT_TIMEOUT)? {
            response::Result::SetPersonaState(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

/// Library calls (`docs/spec/ipc.md` §5.3).
impl Client {
    pub fn list_apps(
        &self,
        filter: &str,
        installed_only: bool,
    ) -> Result<v1::ListAppsResponse, ClientError> {
        let req = v1::ListAppsRequest {
            filter: filter.into(),
            installed_only,
        };
        match self.call(request::Method::ListApps(req), DEFAULT_TIMEOUT)? {
            response::Result::ListApps(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn get_app(&self, app_id: u32) -> Result<v1::GetAppResponse, ClientError> {
        let req = v1::GetAppRequest { app_id };
        match self.call(request::Method::GetApp(req), DEFAULT_TIMEOUT)? {
            response::Result::GetApp(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    /// The path of one of an app's images, downloaded into the daemon's
    /// cache first if needed.
    pub fn get_asset(
        &self,
        app_id: u32,
        kind: v1::AssetKind,
        high_dpi: bool,
    ) -> Result<String, ClientError> {
        let req = v1::GetAssetRequest {
            app_id,
            kind: kind as i32,
            high_dpi,
        };
        match self.call(request::Method::GetAsset(req), STEAM_TIMEOUT)? {
            response::Result::GetAsset(r) => Ok(r.path),
            other => Err(unexpected(other)),
        }
    }
}

/// Settings calls (`docs/spec/ipc.md` §5.4).
impl Client {
    pub fn get_settings(&self) -> Result<Vec<v1::Setting>, ClientError> {
        let req = v1::GetSettingsRequest {};
        match self.call(request::Method::GetSettings(req), DEFAULT_TIMEOUT)? {
            response::Result::GetSettings(r) => Ok(r.settings),
            other => Err(unexpected(other)),
        }
    }

    /// Overrides `key` with `value`, or with `value` `None` removes the
    /// override. Returns the setting as it is afterwards.
    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<v1::Setting, ClientError> {
        let req = v1::SetSettingRequest {
            key: key.into(),
            value: value.unwrap_or_default().into(),
            reset: value.is_none(),
        };
        match self.call(request::Method::SetSetting(req), DEFAULT_TIMEOUT)? {
            response::Result::SetSetting(r) => r.setting.ok_or_else(|| {
                ClientError::Protocol("SetSetting answered without a setting".into())
            }),
            other => Err(unexpected(other)),
        }
    }
}

/// Library folder calls (`docs/spec/ipc.md` §5.5).
impl Client {
    pub fn list_library_folders(&self) -> Result<Vec<v1::LibraryFolder>, ClientError> {
        let req = v1::ListLibraryFoldersRequest {};
        match self.call(request::Method::ListLibraryFolders(req), DEFAULT_TIMEOUT)? {
            response::Result::ListLibraryFolders(r) => Ok(r.folders),
            other => Err(unexpected(other)),
        }
    }

    pub fn add_library_folder(&self, path: &str) -> Result<v1::LibraryFolder, ClientError> {
        let req = v1::AddLibraryFolderRequest { path: path.into() };
        match self.call(request::Method::AddLibraryFolder(req), DEFAULT_TIMEOUT)? {
            response::Result::AddLibraryFolder(r) => r.folder.ok_or_else(|| {
                ClientError::Protocol("AddLibraryFolder answered without a folder".into())
            }),
            other => Err(unexpected(other)),
        }
    }

    pub fn remove_library_folder(&self, path: &str) -> Result<(), ClientError> {
        let req = v1::RemoveLibraryFolderRequest { path: path.into() };
        match self.call(request::Method::RemoveLibraryFolder(req), DEFAULT_TIMEOUT)? {
            response::Result::RemoveLibraryFolder(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

/// Job calls (`docs/spec/ipc.md` §5.6).
impl Client {
    /// Queues an install. An empty `folder` is the default folder, an
    /// empty `branch` is `public`.
    pub fn install(&self, app_id: u32, folder: &str, branch: &str) -> Result<v1::Job, ClientError> {
        let req = v1::InstallRequest {
            app_id,
            folder: folder.into(),
            branch: branch.into(),
        };
        match self.call(request::Method::Install(req), DEFAULT_TIMEOUT)? {
            response::Result::Install(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    /// Queues updates of `app_ids`, or with `all` of every installed app
    /// with a known update. Returns the jobs created.
    pub fn update(&self, app_ids: &[u32], all: bool) -> Result<Vec<v1::Job>, ClientError> {
        let req = v1::UpdateRequest {
            app_ids: app_ids.to_vec(),
            all,
        };
        match self.call(request::Method::Update(req), DEFAULT_TIMEOUT)? {
            response::Result::Update(r) => Ok(r.jobs),
            other => Err(unexpected(other)),
        }
    }

    pub fn verify(&self, app_id: u32) -> Result<v1::Job, ClientError> {
        let req = v1::VerifyRequest { app_id };
        match self.call(request::Method::Verify(req), DEFAULT_TIMEOUT)? {
            response::Result::Verify(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    /// Uninstalls an app, and on Linux its prefix (`compat-tools.md`
    /// §6.1).
    pub fn uninstall(&self, app_id: u32) -> Result<v1::Job, ClientError> {
        let req = v1::UninstallRequest { app_id };
        match self.call(request::Method::Uninstall(req), DEFAULT_TIMEOUT)? {
            response::Result::Uninstall(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn list_jobs(&self) -> Result<Vec<v1::Job>, ClientError> {
        let req = v1::ListJobsRequest {};
        match self.call(request::Method::ListJobs(req), DEFAULT_TIMEOUT)? {
            response::Result::ListJobs(r) => Ok(r.jobs),
            other => Err(unexpected(other)),
        }
    }

    pub fn pause_job(&self, job_id: u64) -> Result<v1::Job, ClientError> {
        let req = v1::PauseJobRequest { job_id };
        match self.call(request::Method::PauseJob(req), DEFAULT_TIMEOUT)? {
            response::Result::PauseJob(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn resume_job(&self, job_id: u64) -> Result<v1::Job, ClientError> {
        let req = v1::ResumeJobRequest { job_id };
        match self.call(request::Method::ResumeJob(req), DEFAULT_TIMEOUT)? {
            response::Result::ResumeJob(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn cancel_job(&self, job_id: u64) -> Result<v1::Job, ClientError> {
        let req = v1::CancelJobRequest { job_id };
        match self.call(request::Method::CancelJob(req), DEFAULT_TIMEOUT)? {
            response::Result::CancelJob(r) => job(r),
            other => Err(unexpected(other)),
        }
    }

    pub fn set_job_priority(&self, job_id: u64, priority: i32) -> Result<v1::Job, ClientError> {
        let req = v1::SetJobPriorityRequest { job_id, priority };
        match self.call(request::Method::SetJobPriority(req), DEFAULT_TIMEOUT)? {
            response::Result::SetJobPriority(r) => job(r),
            other => Err(unexpected(other)),
        }
    }
}

impl Client {
    /// The app's launch options, each marked with whether it can be
    /// launched here (`launch.md` §8).
    pub fn get_launch_options(
        &self,
        app_id: u32,
    ) -> Result<v1::GetLaunchOptionsResponse, ClientError> {
        let req = v1::GetLaunchOptionsRequest { app_id };
        match self.call(request::Method::GetLaunchOptions(req), DEFAULT_TIMEOUT)? {
            response::Result::GetLaunchOptions(r) => Ok(r),
            other => Err(unexpected(other)),
        }
    }

    /// Launches an installed app. `option` `None` launches the only option
    /// that qualifies. Waits without a timeout: the daemon replies once the
    /// compatibility tools the launch needs are installed, which can take
    /// as long as their downloads.
    pub fn launch(
        &self,
        app_id: u32,
        option: Option<u32>,
        extra_args: &[String],
        allow_stale: bool,
    ) -> Result<v1::RunningGame, ClientError> {
        let req = v1::LaunchRequest {
            app_id,
            option,
            extra_args: extra_args.to_vec(),
            allow_stale,
        };
        match self.call(request::Method::Launch(req), Duration::MAX)? {
            response::Result::Launch(r) => r.game.ok_or_else(|| {
                ClientError::Protocol("a launch was answered without a game".into())
            }),
            other => Err(unexpected(other)),
        }
    }

    /// The games the daemon launched that are running.
    pub fn list_running(&self) -> Result<Vec<v1::RunningGame>, ClientError> {
        match self.call(
            request::Method::ListRunning(v1::ListRunningRequest {}),
            DEFAULT_TIMEOUT,
        )? {
            response::Result::ListRunning(r) => Ok(r.games),
            other => Err(unexpected(other)),
        }
    }

    /// Asks a running game to close; `GameExited` follows when it has.
    pub fn stop_game(&self, app_id: u32) -> Result<(), ClientError> {
        let req = v1::StopGameRequest { app_id };
        match self.call(request::Method::StopGame(req), DEFAULT_TIMEOUT)? {
            response::Result::StopGame(_) => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

fn job(r: v1::JobResponse) -> Result<v1::Job, ClientError> {
    r.job
        .ok_or_else(|| ClientError::Protocol("a job request was answered without a job".into()))
}

/// Zeroes the secrets a request carried once it has been sent (IPC-10).
fn scrub(env: &mut v1::Envelope) {
    use zeroize::Zeroize;
    if let Some(Body::Request(v1::Request {
        method: Some(method),
    })) = &mut env.body
    {
        match method {
            request::Method::LoginWithCredentials(r) => r.password.zeroize(),
            request::Method::SubmitGuardCode(r) => r.code.zeroize(),
            _ => {}
        }
    }
}

fn topics_from(raw: &[i32]) -> Vec<Topic> {
    raw.iter()
        .filter_map(|&t| Topic::try_from(t).ok())
        .collect()
}

fn unexpected(r: response::Result) -> ClientError {
    ClientError::Protocol(format!("response does not match the request: {r:?}"))
}

fn reader_loop(mut rx: &Stream, inner: &Inner, events: &Sender<Event>) {
    loop {
        let env = match read_frame(&mut rx) {
            Ok(env) => env,
            Err(FrameError::Closed) => break,
            Err(e) => {
                tracing::warn!("daemon connection failed: {e}");
                break;
            }
        };
        let reply = match env.body {
            Some(Body::Event(ev)) => {
                let _ = events.send(ev);
                continue;
            }
            Some(Body::Response(v1::Response { result: Some(r) })) => Ok(r),
            Some(Body::Response(v1::Response { result: None })) => Err(RemoteError {
                code: ErrorCode::Unsupported,
                message: "the daemon sent a response this client does not understand".into(),
                detail: String::new(),
            }),
            Some(Body::Error(e)) => Err(RemoteError::from(e)),
            other => {
                tracing::warn!("unexpected frame from daemon: {other:?}");
                continue;
            }
        };
        if env.id == 0 {
            // A connection-level error, e.g. sent just before the daemon
            // closes the connection. Nothing waits on id 0.
            if let Err(e) = reply {
                tracing::warn!("daemon: {e}");
            }
            continue;
        }
        if let Some(tx) = inner.pending.lock().unwrap().remove(&env.id) {
            let _ = tx.send(reply);
        }
    }
    inner.closed.store(true, Ordering::Release);
    // Dropping the senders wakes every waiting call with Disconnected.
    inner.pending.lock().unwrap().clear();
}

/// Errors that mean nothing is listening, as opposed to something that
/// refused us.
fn daemon_absent(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

/// The daemon executable: `$OSSTEAM_DAEMON` if set, else `ossteamd` next to
/// the running executable if present, else `ossteamd` from `PATH`.
fn daemon_program() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("OSSTEAM_DAEMON") {
        return p.into();
    }
    let file = format!("ossteamd{}", std::env::consts::EXE_SUFFIX);
    if let Some(dir) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(|p| p.parent())
    {
        let candidate = dir.join(&file);
        if candidate.is_file() {
            return candidate;
        }
    }
    file.into()
}

/// Starts the daemon detached from this process, waits until a daemon
/// accepts connections, and returns the first connection with the spawned
/// process's PID.
fn start_daemon(
    ep: &Endpoint,
    timeout: Duration,
) -> Result<(interprocess::local_socket::Stream, u32), ClientError> {
    let program = daemon_program();
    let mut cmd = Command::new(&program);
    cmd.arg("--background")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = spawn_detached(&mut cmd)
        .map_err(|e| ClientError::StartFailed(format!("{}: {e}", program.display())))?;
    let pid = child.id();

    let deadline = Instant::now() + timeout;
    let stream = loop {
        match ep.connect() {
            Ok(s) => break s,
            Err(e) if daemon_absent(&e) => {}
            Err(source) => {
                return Err(ClientError::Unavailable {
                    address: ep.address.clone(),
                    source,
                });
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Status 0 means another daemon won the race to start; keep
            // waiting for it to listen.
            if !status.success() {
                return Err(ClientError::StartFailed(format!(
                    "ossteamd exited with {status}"
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(ClientError::StartFailed(format!(
                "ossteamd did not start listening within {timeout:?}"
            )));
        }
        thread::sleep(Duration::from_millis(20));
    };
    // Reap the daemon when it eventually exits, so a long-lived client such
    // as the GUI does not keep a zombie around.
    let _ = thread::Builder::new()
        .name("ossteamd-reaper".into())
        .spawn(move || child.wait());
    Ok((stream, pid))
}

#[cfg(unix)]
fn spawn_detached(cmd: &mut Command) -> io::Result<std::process::Child> {
    // The daemon calls setsid() itself when given --background.
    cmd.spawn()
}

#[cfg(windows)]
fn spawn_detached(cmd: &mut Command) -> io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let base = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    // Leave the caller's job object if it allows that, so closing the
    // terminal that ran the CLI does not take the daemon with it.
    match cmd.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(c) => Ok(c),
        Err(_) => cmd.creation_flags(base).spawn(),
    }
}
