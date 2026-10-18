// SPDX-License-Identifier: GPL-2.0-or-later
//! Pipes: one game IPC connection each, with its callback queue and API
//! call results (`steamworks-abi.md` §2.3, §2.5, §3).

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use ossteam_gameipc::endpoint::{self, Address};
use ossteam_gameipc::{
    App, FrameError, Hello, Message, PROTOCOL, Request, RequestBody, ResponseBody, Session,
    read_message, write_message,
};

use crate::callbacks;
use crate::log::log;

/// How long the shim waits for the daemon's welcome and for answers.
const TIMEOUT: Duration = Duration::from_secs(5);

/// `ESteamAPICallFailure`.
pub const FAILURE_NONE: i32 = -1;
pub const FAILURE_STEAM_GONE: i32 = 0;
pub const FAILURE_INVALID_HANDLE: i32 = 2;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the daemon last said.
pub struct Info {
    pub session: Session,
    pub app: App,
    /// When `session` arrived, for Steam's clock.
    pub received: Instant,
}

impl Info {
    /// Steam's clock now, or 0 before the first logon.
    pub fn server_time(&self) -> u32 {
        if self.session.server_time == 0 {
            return 0;
        }
        let elapsed = self.received.elapsed().as_secs();
        self.session
            .server_time
            .saturating_add(elapsed.min(u64::from(u32::MAX)) as u32)
    }
}

struct Callback {
    id: i32,
    data: Box<[u8]>,
}

/// A completed API call.
struct Done {
    callback: i32,
    data: Vec<u8>,
    failed: bool,
    reason: i32,
}

/// Makes an API call's result structure from the daemon's answer, or
/// `None` when the call failed.
pub type Decode = Box<dyn FnOnce(&Pipe, &ResponseBody) -> Option<Vec<u8>> + Send>;

/// Runs when the daemon answers a request, or with `None` when the
/// connection closed first.
pub type Then = Box<dyn FnOnce(&Pipe, Option<&ResponseBody>) + Send>;

/// A request waiting for the daemon.
enum Pending {
    /// An API call (§2.5).
    Call {
        call: u64,
        callback: i32,
        size: usize,
        decode: Decode,
    },
    Then(Then),
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Callback>,
    /// The front item was handed to the game and not freed yet.
    handed: bool,
    results: HashMap<u64, Done>,
}

pub struct Pipe {
    pub handle: i32,
    user: AtomicI32,
    closed: AtomicBool,
    writer: Mutex<Box<dyn Write + Send>>,
    closer: Box<dyn Fn() + Send + Sync>,
    info: RwLock<Info>,
    queue: Mutex<Queue>,
    next_request: AtomicU32,
    waiters: Mutex<HashMap<u32, mpsc::Sender<ResponseBody>>>,
    /// API calls and other requests answered later, by request id.
    pending: Mutex<HashMap<u32, Pending>>,
    /// `ISteamUserStats` (`isteamuserstats.md` §3).
    pub stats: Mutex<crate::userstats::Stats>,
}

static PIPES: Mutex<Vec<Arc<Pipe>>> = Mutex::new(Vec::new());
static NEXT_PIPE: AtomicI32 = AtomicI32::new(1);
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);
/// Requests sent since `GetIPCCallCount` was last called.
static IPC_CALLS: AtomicU32 = AtomicU32::new(0);
static ENDPOINT: Mutex<Option<Address>> = Mutex::new(None);

/// Connects to `addr` instead of this user's endpoint: for tests, which
/// run a daemon of their own.
pub fn override_endpoint(addr: Address) {
    *lock(&ENDPOINT) = Some(addr);
}

/// The open pipe with this handle.
pub fn get(handle: i32) -> Option<Arc<Pipe>> {
    lock(&PIPES).iter().find(|p| p.handle == handle).cloned()
}

pub fn any_open() -> bool {
    !lock(&PIPES).is_empty()
}

/// The number of requests since the previous call.
pub fn take_ipc_calls() -> u32 {
    IPC_CALLS.swap(0, Ordering::Relaxed)
}

/// The claimed app id (`steamworks-abi.md` §3.3).
pub fn claimed_app_id() -> u32 {
    let var = |n| {
        std::env::var(n)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    if let Some(id) = var("SteamAppId").filter(|&id| id != 0) {
        return id as u32;
    }
    var("SteamGameId").map_or(0, |id| (id & 0xff_ffff) as u32)
}

fn arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "aarch64"
    }
}

/// `ISteamClient::CreateSteamPipe`: 0 when no daemon answers or it
/// refuses.
pub fn create() -> i32 {
    let addr = match lock(&ENDPOINT).clone() {
        Some(a) => Ok(a),
        None => Address::locate(),
    };
    let conn = match addr.and_then(|a| endpoint::connect(&a)) {
        Ok(c) => c,
        Err(e) => {
            log!(Warn, "no ossteam daemon to connect to: {e}");
            return 0;
        }
    };
    let mut writer = conn.writer;
    let mut reader = conn.reader;
    let closer = conn.closer;
    let hello = Message::Hello(Hello {
        protocol: PROTOCOL,
        pid: std::process::id(),
        app_id: claimed_app_id(),
        arch: arch().into(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    if let Err(e) = write_message(&mut writer, &hello) {
        log!(Warn, "the daemon closed the connection: {e}");
        closer();
        return 0;
    }

    // The welcome is read on the reader thread, so that a daemon that
    // does not answer cannot hang the game for longer than TIMEOUT.
    let (welcome_tx, welcome_rx) = mpsc::channel();
    let (pipe_tx, pipe_rx) = mpsc::channel::<Arc<Pipe>>();
    let spawned = std::thread::Builder::new()
        .name("ossteam-steamclient".into())
        .spawn(move || {
            let first = read_message(&mut reader);
            let ok = matches!(first, Ok(Message::Welcome(_)));
            let _ = welcome_tx.send(first);
            if !ok {
                return;
            }
            if let Ok(pipe) = pipe_rx.recv() {
                pipe.read_loop(&mut *reader);
            }
        });
    if let Err(e) = spawned {
        log!(Warn, "cannot start a thread for the pipe: {e}");
        closer();
        return 0;
    }
    let welcome = match welcome_rx.recv_timeout(TIMEOUT) {
        Ok(Ok(Message::Welcome(w))) => Ok(w),
        Ok(Ok(Message::Refused(r))) => {
            Err(format!("the daemon refused the connection: {}", r.reason))
        }
        Ok(Ok(other)) => Err(format!("the daemon sent {other:?} instead of a welcome")),
        Ok(Err(e)) => Err(format!("the daemon closed the connection: {e}")),
        Err(_) => Err(format!("the daemon did not answer within {TIMEOUT:?}")),
    };
    let welcome = match welcome {
        Ok(w) => w,
        Err(e) => {
            log!(Warn, "{e}");
            // This also ends the reader thread when it still waits.
            closer();
            return 0;
        }
    };
    let handle = NEXT_PIPE.fetch_add(1, Ordering::Relaxed);
    let pipe = Arc::new(Pipe {
        handle,
        user: AtomicI32::new(0),
        closed: AtomicBool::new(false),
        writer: Mutex::new(writer),
        closer,
        info: RwLock::new(Info {
            session: welcome.session,
            app: welcome.app,
            received: Instant::now(),
        }),
        queue: Mutex::new(Queue::default()),
        next_request: AtomicU32::new(1),
        waiters: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        stats: Mutex::new(crate::userstats::Stats::default()),
    });
    lock(&PIPES).push(Arc::clone(&pipe));
    let _ = pipe_tx.send(Arc::clone(&pipe));
    log!(Debug, "pipe {handle} connected");
    send_report(true);
    handle
}

/// `ISteamClient::BReleaseSteamPipe`.
pub fn release(handle: i32) -> bool {
    let Some(pipe) = get(handle) else {
        return false;
    };
    send_report(true);
    lock(&PIPES).retain(|p| p.handle != handle);
    pipe.close(true);
    true
}

/// Sends what the report holds over an open pipe (§4). With `force`, it
/// does not wait for the interval.
pub fn send_report(force: bool) {
    if !crate::report::due(force) {
        return;
    }
    let pipes: Vec<Arc<Pipe>> = lock(&PIPES).clone();
    let Some(pipe) = pipes.iter().find(|p| !p.is_closed()) else {
        return;
    };
    if let Some(r) = crate::report::take() {
        let msg = Message::Report(r);
        if !pipe.send(&msg)
            && let Message::Report(r) = msg
        {
            crate::report::restore(r);
        }
    }
}

impl Pipe {
    /// The connection is gone: the daemon exited or the pipe was
    /// released.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn info(&self) -> std::sync::RwLockReadGuard<'_, Info> {
        self.info.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn user(&self) -> i32 {
        self.user.load(Ordering::Acquire)
    }

    pub fn set_user(&self, user: i32) {
        self.user.store(user, Ordering::Release);
    }

    /// Logged on: open, an account active and connected to Steam.
    pub fn logged_on(&self) -> bool {
        if self.is_closed() {
            return false;
        }
        let i = self.info();
        i.session.active && i.session.logged_on
    }

    /// Sends a message. Returns whether it was written.
    fn send(&self, msg: &Message) -> bool {
        if self.is_closed() {
            return false;
        }
        let mut w = lock(&self.writer);
        match write_message(&mut *w, msg) {
            Ok(()) => true,
            Err(e) => {
                log!(Debug, "pipe {}: writing failed: {e}", self.handle);
                false
            }
        }
    }

    /// Asks the daemon and waits for the answer. `None` when the pipe is
    /// closed or the daemon does not answer in time.
    pub fn request(&self, body: RequestBody) -> Option<ResponseBody> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        lock(&self.waiters).insert(id, tx);
        IPC_CALLS.fetch_add(1, Ordering::Relaxed);
        log!(Debug, "pipe {}: request {id}: {body:?}", self.handle);
        if !self.send(&Message::Request(Request { id, body })) {
            lock(&self.waiters).remove(&id);
            return None;
        }
        let answer = rx.recv_timeout(TIMEOUT).ok();
        lock(&self.waiters).remove(&id);
        if answer.is_none() {
            log!(Warn, "pipe {}: request {id} got no answer", self.handle);
        }
        answer
    }

    /// Starts an API call (§2.5) whose result is `callback`, `size` bytes,
    /// made from the daemon's answer by `decode`. Returns its handle, or 0
    /// (`k_uAPICallInvalid`) when the pipe is closed.
    pub fn begin_call(&self, body: RequestBody, callback: i32, size: usize, decode: Decode) -> u64 {
        if self.is_closed() {
            return 0;
        }
        let call = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
        let pending = Pending::Call {
            call,
            callback,
            size,
            decode,
        };
        if let Some(Pending::Call {
            call,
            callback,
            size,
            ..
        }) = self.send_pending(body, pending)
        {
            self.complete(call, callback, vec![0; size], FAILURE_STEAM_GONE);
        }
        call
    }

    /// Sends a request whose answer goes to `then`, on the pipe's reader
    /// thread. Returns false, without calling `then`, when the request
    /// could not be sent.
    pub fn send_then(&self, body: RequestBody, then: Then) -> bool {
        self.send_pending(body, Pending::Then(then)).is_none()
    }

    /// Sends a request answered later. Returns `pending` when it was not
    /// sent.
    fn send_pending(&self, body: RequestBody, pending: Pending) -> Option<Pending> {
        if self.is_closed() {
            return Some(pending);
        }
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        lock(&self.pending).insert(id, pending);
        IPC_CALLS.fetch_add(1, Ordering::Relaxed);
        log!(Debug, "pipe {}: request {id}: {body:?}", self.handle);
        if self.send(&Message::Request(Request { id, body })) {
            None
        } else {
            lock(&self.pending).remove(&id)
        }
    }

    /// Stores a call's result and queues callback 703.
    fn complete(&self, call: u64, callback: i32, data: Vec<u8>, reason: i32) {
        let size = data.len() as u32;
        let failed = reason != FAILURE_NONE;
        lock(&self.queue).results.insert(
            call,
            Done {
                callback,
                data,
                failed,
                reason,
            },
        );
        self.push(
            callbacks::API_CALL_COMPLETED,
            callbacks::api_call_completed(call, callback, size),
        );
    }

    /// Queues a callback.
    pub fn push(&self, id: i32, data: Vec<u8>) {
        lock(&self.queue).items.push_back(Callback {
            id,
            data: data.into_boxed_slice(),
        });
    }

    /// `Steam_BGetCallback`: the oldest callback, as (user, id, pointer,
    /// size).
    pub fn next_callback(&self) -> Option<(i32, i32, *mut u8, i32)> {
        let mut q = lock(&self.queue);
        q.handed = !q.items.is_empty();
        let user = self.user();
        q.items
            .front_mut()
            .map(|c| (user, c.id, c.data.as_mut_ptr(), c.data.len() as i32))
    }

    /// `Steam_FreeLastCallback`.
    pub fn free_callback(&self) -> bool {
        let mut q = lock(&self.queue);
        if !q.handed {
            return false;
        }
        q.handed = false;
        q.items.pop_front().is_some()
    }

    /// `Steam_GetAPICallResult`: copies the result into `buf` when it is
    /// there and matches. Returns `Some(failed)` when it did.
    ///
    /// # Safety
    ///
    /// `buf` is null or holds `size` writable bytes.
    pub unsafe fn take_result(
        &self,
        call: u64,
        buf: *mut u8,
        size: i32,
        expected: i32,
    ) -> Option<bool> {
        let mut q = lock(&self.queue);
        let done = q.results.get(&call)?;
        if done.callback != expected || done.data.len() != size as usize || buf.is_null() {
            return None;
        }
        // SAFETY: the caller promises `size` bytes, which is the result's
        // length.
        unsafe { std::ptr::copy_nonoverlapping(done.data.as_ptr(), buf, done.data.len()) };
        let failed = done.failed;
        q.results.remove(&call);
        Some(failed)
    }

    /// `ISteamUtils::IsAPICallCompleted`: `Some(failed)` once the result
    /// is stored.
    pub fn call_completed(&self, call: u64) -> Option<bool> {
        lock(&self.queue).results.get(&call).map(|d| d.failed)
    }

    /// `ISteamUtils::GetAPICallFailureReason`.
    pub fn call_failure(&self, call: u64) -> i32 {
        if let Some(d) = lock(&self.queue).results.get(&call) {
            return d.reason;
        }
        if lock(&self.pending)
            .values()
            .any(|p| matches!(p, Pending::Call { call: c, .. } if *c == call))
        {
            return FAILURE_NONE;
        }
        FAILURE_INVALID_HANDLE
    }

    /// Closes the connection. A released pipe drops its callbacks and
    /// results. When the connection is lost instead, pending calls fail
    /// with `SteamGone` and everything stays for the game to collect
    /// (§2.5).
    fn close(&self, released: bool) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // The reader thread then ends and drops its half.
        (self.closer)();
        *lock(&self.writer) = Box::new(std::io::sink());
        lock(&self.waiters).clear();
        let pending: Vec<Pending> = lock(&self.pending).drain().map(|(_, p)| p).collect();
        if released {
            let mut q = lock(&self.queue);
            q.items.clear();
            q.handed = false;
            q.results.clear();
            return;
        }
        for p in pending {
            match p {
                Pending::Call {
                    call,
                    callback,
                    size,
                    ..
                } => self.complete(call, callback, vec![0; size], FAILURE_STEAM_GONE),
                Pending::Then(then) => then(self, None),
            }
        }
    }

    fn read_loop(&self, mut reader: &mut dyn std::io::Read) {
        loop {
            match read_message(&mut reader) {
                Ok(Message::Response(r)) => {
                    let pending = lock(&self.pending).remove(&r.id);
                    if let Some(p) = pending {
                        self.answered(p, &r.body);
                    } else if let Some(tx) = lock(&self.waiters).remove(&r.id) {
                        let _ = tx.send(r.body);
                    }
                }
                Ok(Message::SessionUpdate(s)) => self.on_session(s),
                Ok(Message::AppUpdate(a)) => {
                    self.info.write().unwrap_or_else(|e| e.into_inner()).app = a;
                }
                Ok(other) => {
                    log!(Warn, "pipe {}: unexpected {other:?}; closing", self.handle);
                    break;
                }
                Err(FrameError::Closed) => break,
                Err(e) => {
                    if !self.is_closed() {
                        log!(Warn, "pipe {}: reading failed: {e}", self.handle);
                    }
                    break;
                }
            }
        }
        if !self.is_closed() {
            log!(Info, "the daemon closed pipe {}", self.handle);
            let was = self.logged_on();
            self.close(false);
            if was {
                self.push(
                    callbacks::SERVERS_DISCONNECTED,
                    callbacks::servers_disconnected(callbacks::E_RESULT_NO_CONNECTION),
                );
            }
        }
    }

    /// The daemon answered a request that was waiting in `pending`.
    fn answered(&self, p: Pending, body: &ResponseBody) {
        match p {
            Pending::Call {
                call,
                callback,
                size,
                decode,
            } => match decode(self, body) {
                Some(data) if data.len() == size => {
                    self.complete(call, callback, data, FAILURE_NONE)
                }
                _ => self.complete(call, callback, vec![0; size], FAILURE_STEAM_GONE),
            },
            Pending::Then(then) => then(self, Some(body)),
        }
    }

    fn on_session(&self, s: Session) {
        let was = self.logged_on();
        {
            let mut i = self.info.write().unwrap_or_else(|e| e.into_inner());
            i.session = s;
            i.received = Instant::now();
        }
        let now = self.logged_on();
        if now && !was {
            self.push(callbacks::SERVERS_CONNECTED, callbacks::servers_connected());
            crate::userstats::logged_on(self);
        } else if was && !now {
            self.push(
                callbacks::SERVERS_DISCONNECTED,
                callbacks::servers_disconnected(callbacks::E_RESULT_NO_CONNECTION),
            );
        }
    }
}
