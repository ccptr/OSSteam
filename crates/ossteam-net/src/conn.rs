// SPDX-License-Identifier: GPL-2.0-or-later
//! A live CM connection: one I/O thread, a job table and heartbeats
//! (`docs/spec/cm.md` §3.3, §3.5, §4; ADR 0001).

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender};
use ossteam_proto::ServiceMethod;
use ossteam_proto::steam::{CMsgClientHeartBeat, CMsgClientHello, CMsgMulti, EMsg};
use ossteam_types::{CancelToken, EResult, SteamId};
use polling::{Event, Events, Poller};

use crate::directory::CmServer;
use crate::msg::{Header, JOB_NONE, Packet};
use crate::multi;
use crate::transport::{self, CANCEL_SLICE, ConnectError, Transport};

/// The protocol version sent in `ClientHello` (`cm.md` §4, unverified).
pub const PROTOCOL_VERSION: u32 = 65580;

/// How deeply `Multi` messages may nest before they are dropped.
const MAX_MULTI_DEPTH: u32 = 4;

const POLL_KEY: usize = 0;

/// What the connection delivers that is not a reply to a request.
#[derive(Debug)]
pub enum Incoming {
    Message(Packet),
    /// The connection is gone. This is always the last item.
    Closed(CloseReason),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CloseReason {
    #[error("closed by this client")]
    Local,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the connection is closed")]
    Closed,
    #[error("no reply within the timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("the server answered {eresult}{}", message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Failed {
        eresult: EResult,
        message: Option<String>,
    },
    #[error("unexpected reply message {0}")]
    UnexpectedReply(u32),
    #[error("reply does not decode: {0}")]
    Decode(#[from] prost::DecodeError),
}

/// The ids the server assigned at logon, stamped on outgoing messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionIds {
    pub steam_id: SteamId,
    pub session_id: i32,
}

struct Shared {
    poller: Poller,
    outbox: Mutex<VecDeque<Vec<u8>>>,
    /// `None` once the connection has closed, so no new job can wait on
    /// a reply that will never come.
    jobs: Mutex<Option<HashMap<u64, Sender<Packet>>>>,
    next_job: AtomicU64,
    session: Mutex<Option<SessionIds>>,
    heartbeat: Mutex<Option<Duration>>,
    close_requested: AtomicBool,
    events: Sender<Incoming>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere never leaves these maps half-updated, so a
    // poisoned lock is still usable.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A CM connection. Dropping it closes the connection.
pub struct Connection {
    shared: Arc<Shared>,
    server: CmServer,
    thread: Option<JoinHandle<()>>,
}

impl Connection {
    /// Connects to `server` and starts the I/O thread. Unsolicited
    /// messages, and finally the reason the connection closed, go to
    /// `events`.
    pub fn connect(
        server: &CmServer,
        timeout: Duration,
        events: Sender<Incoming>,
        cancel: &CancelToken,
    ) -> Result<Self, ConnectError> {
        let transport = transport::connect(server, timeout, cancel)?;
        transport.socket().set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            poller: Poller::new()?,
            outbox: Mutex::new(VecDeque::new()),
            jobs: Mutex::new(Some(HashMap::new())),
            next_job: AtomicU64::new(1),
            session: Mutex::new(None),
            heartbeat: Mutex::new(None),
            close_requested: AtomicBool::new(false),
            events,
        });
        let thread_shared = shared.clone();
        let cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("cm-io".into())
            .spawn(move || run(&thread_shared, transport, &cancel))?;
        tracing::info!(endpoint = %server.endpoint, "CM connected");
        Ok(Self {
            shared,
            server: server.clone(),
            thread: Some(thread),
        })
    }

    pub fn server(&self) -> &CmServer {
        &self.server
    }

    pub fn is_closed(&self) -> bool {
        lock(&self.shared.jobs).is_none()
    }

    /// Sends `ClientHello` (`cm.md` §4 step 2).
    pub fn hello(&self) -> Result<(), CallError> {
        self.send(Packet::proto(
            EMsg::KEMsgClientHello,
            &CMsgClientHello {
                protocol_version: Some(PROTOCOL_VERSION),
            },
        ))
    }

    /// Records the logged-on session. From now on outgoing protobuf
    /// headers carry these ids, and service calls are authenticated.
    pub fn set_session(&self, ids: SessionIds) {
        *lock(&self.shared.session) = Some(ids);
    }

    pub fn session(&self) -> Option<SessionIds> {
        *lock(&self.shared.session)
    }

    /// Starts (or changes) heartbeats at the interval the server gave.
    pub fn set_heartbeat(&self, interval: Duration) {
        *lock(&self.shared.heartbeat) = Some(interval.max(Duration::from_secs(1)));
        let _ = self.shared.poller.notify();
    }

    /// Queues one message.
    pub fn send(&self, mut packet: Packet) -> Result<(), CallError> {
        if self.is_closed() {
            return Err(CallError::Closed);
        }
        stamp(&self.shared, &mut packet);
        lock(&self.shared.outbox).push_back(packet.encode());
        let _ = self.shared.poller.notify();
        Ok(())
    }

    /// Sends `packet` as a new job and waits for the reply that targets it.
    pub fn request(
        &self,
        mut packet: Packet,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Packet, CallError> {
        let job = self.shared.next_job.fetch_add(1, Ordering::Relaxed);
        packet.set_source_job(job);
        let (tx, rx) = crossbeam_channel::bounded(1);
        match lock(&self.shared.jobs).as_mut() {
            Some(jobs) => jobs.insert(job, tx),
            None => return Err(CallError::Closed),
        };
        let forget = || {
            if let Some(jobs) = lock(&self.shared.jobs).as_mut() {
                jobs.remove(&job);
            }
        };
        if let Err(e) = self.send(packet) {
            forget();
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                forget();
                return Err(CallError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                forget();
                return Err(CallError::Timeout);
            }
            match rx.recv_timeout(left.min(CANCEL_SLICE)) {
                Ok(reply) => return Ok(reply),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(CallError::Closed),
            }
        }
    }

    /// Calls a unified service method (`cm.md` §3.5). The call is
    /// authenticated once a session is set, and non-authed before that.
    pub fn call<M: ServiceMethod>(
        &self,
        request: &M::Request,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<M::Response, CallError> {
        let emsg = if self.session().is_some() {
            EMsg::KEMsgServiceMethodCallFromClient
        } else {
            EMsg::KEMsgServiceMethodCallFromClientNonAuthed
        };
        let mut packet = Packet::proto(emsg, request);
        if let Some(h) = packet.proto_header_mut() {
            h.target_job_name = Some(method_name::<M>());
        }
        let reply = self.request(packet, timeout, cancel)?;
        if !reply.is(EMsg::KEMsgServiceMethodResponse) {
            return Err(CallError::UnexpectedReply(reply.emsg));
        }
        let header = reply
            .proto_header()
            .expect("service responses have protobuf headers");
        let eresult = EResult(header.eresult());
        if !eresult.is_ok() {
            return Err(CallError::Failed {
                eresult,
                message: header.error_message.clone().filter(|m| !m.is_empty()),
            });
        }
        Ok(reply.decode_body()?)
    }

    /// Closes the connection and waits for the I/O thread to finish.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.close_requested.store(true, Ordering::Release);
        let _ = self.shared.poller.notify();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `<Service>.<Method>#<version>` (`cm.md` §3.5).
pub fn method_name<M: ServiceMethod>() -> String {
    format!("{}.{}#1", M::SERVICE, M::METHOD)
}

/// Fills in the session ids on a protobuf header that does not set them.
fn stamp(shared: &Shared, packet: &mut Packet) {
    let Some(ids) = *lock(&shared.session) else {
        return;
    };
    if let Header::Proto(h) = &mut packet.header {
        h.steamid.get_or_insert(ids.steam_id.0);
        h.client_sessionid.get_or_insert(ids.session_id);
    }
}

fn run(shared: &Shared, mut transport: Box<dyn Transport>, cancel: &CancelToken) {
    // SAFETY: the socket stays registered only while `transport` is alive:
    // it is deleted below before `transport` is dropped, panic or not.
    if let Err(e) = unsafe {
        shared
            .poller
            .add(transport.socket(), Event::readable(POLL_KEY))
    } {
        finish(
            shared,
            CloseReason::Io(format!("poller registration failed: {e}")),
        );
        return;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        io_loop(shared, transport.as_mut(), cancel)
    }));
    let _ = shared.poller.delete(transport.socket());
    drop(transport);
    match result {
        Ok(reason) => finish(shared, reason),
        Err(panic) => {
            finish(shared, CloseReason::Io("I/O thread panicked".into()));
            resume_unwind(panic);
        }
    }
}

fn finish(shared: &Shared, reason: CloseReason) {
    // Dropping the senders wakes every waiting request with `Closed`.
    lock(&shared.jobs).take();
    match &reason {
        CloseReason::Io(e) => tracing::warn!(reason = %e, "CM connection closed"),
        r => tracing::info!(reason = %r, "CM connection closed"),
    }
    let _ = shared.events.send(Incoming::Closed(reason));
}

fn io_loop(shared: &Shared, t: &mut dyn Transport, cancel: &CancelToken) -> CloseReason {
    let mut events = Events::new();
    let mut next_heartbeat: Option<Instant> = None;
    loop {
        if shared.close_requested.load(Ordering::Acquire) {
            t.close();
            return CloseReason::Local;
        }
        if cancel.is_cancelled() {
            t.close();
            return CloseReason::Cancelled;
        }

        let now = Instant::now();
        match (*lock(&shared.heartbeat), next_heartbeat) {
            (Some(interval), None) => next_heartbeat = Some(now + interval),
            (Some(interval), Some(due)) if now >= due => {
                let mut beat =
                    Packet::proto(EMsg::KEMsgClientHeartBeat, &CMsgClientHeartBeat::default());
                stamp(shared, &mut beat);
                lock(&shared.outbox).push_back(beat.encode());
                next_heartbeat = Some(now + interval);
            }
            _ => {}
        }

        let outgoing: Vec<_> = lock(&shared.outbox).drain(..).collect();
        for msg in outgoing {
            if let Err(e) = t.send(msg) {
                return CloseReason::Io(e.to_string());
            }
        }
        if let Err(e) = t.flush() {
            return CloseReason::Io(e.to_string());
        }

        let interest = Event::new(POLL_KEY, true, t.wants_write());
        if let Err(e) = shared.poller.modify(t.socket(), interest) {
            return CloseReason::Io(format!("poller: {e}"));
        }
        let mut wait = CANCEL_SLICE;
        if let Some(due) = next_heartbeat {
            wait = wait.min(due.saturating_duration_since(Instant::now()));
        }
        events.clear();
        match shared.poller.wait(&mut events, Some(wait)) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return CloseReason::Io(format!("poller: {e}")),
        }

        // Read whatever is buffered, whatever woke us: TLS may hold
        // decrypted data that the socket no longer signals.
        loop {
            match t.recv() {
                Ok(Some(buf)) => dispatch(shared, &buf, 0),
                Ok(None) => break,
                Err(e) => return CloseReason::Io(e.to_string()),
            }
        }
    }
}

fn dispatch(shared: &Shared, buf: &[u8], depth: u32) {
    let packet = match Packet::parse(buf) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, len = buf.len(), "dropping unparsable CM message");
            return;
        }
    };
    if packet.is(EMsg::KEMsgMulti) {
        if depth >= MAX_MULTI_DEPTH {
            tracing::warn!("dropping Multi nested deeper than {MAX_MULTI_DEPTH}");
            return;
        }
        let inner = packet
            .decode_body::<CMsgMulti>()
            .map_err(|e| e.to_string())
            .and_then(|m| multi::unpack(&m).map_err(|e| e.to_string()));
        match inner {
            Ok(messages) => {
                for m in messages {
                    dispatch(shared, &m, depth + 1);
                }
            }
            Err(e) => tracing::warn!(error = %e, "dropping malformed Multi"),
        }
        return;
    }

    let target = packet.target_job();
    if target != JOB_NONE {
        let waiter = lock(&shared.jobs).as_mut().and_then(|j| j.remove(&target));
        match waiter {
            Some(tx) => {
                let _ = tx.send(packet);
                return;
            }
            None => {
                tracing::debug!(
                    job = target,
                    emsg = packet.emsg,
                    "reply for a job nobody waits on"
                );
            }
        }
    }
    tracing::trace!(emsg = packet.emsg, kind = ?packet.kind(), "CM message");
    let _ = shared.events.send(Incoming::Message(packet));
}
