// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of `ISteamHTTP` (`docs/spec/steamworks/isteamhttp.md`
//! §6 to §8): each connection's requests, queued and run on worker
//! threads, and each process's cookie containers.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use ossteam_gameipc::{
    Done, HttpDone, HttpSendRequest, Message, NoticeBody, RequestBody, Response, ResponseBody,
};
use ossteam_types::CancelToken;

use super::shim::error;
use super::{Command, Core};
use crate::gameipc::ConnId;
use crate::http::{self, Event, Jar, Job};

/// The limits of §6.
const RUNNING: usize = 8;
const WAITING: usize = 256;
/// Cookie containers per process.
const CONTAINERS: usize = 1024;

struct Running {
    id: u32,
    seq: u64,
    stop: CancelToken,
    /// Cancelled: it keeps its place until its thread ends, and nothing
    /// more is sent for it.
    cancelled: bool,
}

struct Waiting {
    id: u32,
    request: HttpSendRequest,
}

/// One connection's requests.
#[derive(Default)]
struct Lane {
    running: HashMap<u32, Running>,
    waiting: VecDeque<Waiting>,
}

#[derive(Default)]
pub struct Http {
    agent: Option<ureq::Agent>,
    lanes: HashMap<ConnId, Lane>,
    /// Cookie containers by process and handle.
    jars: HashMap<(u32, u32), Arc<Mutex<Jar>>>,
    next_seq: u64,
    pub stop: CancelToken,
}

/// What a request's worker reports, back on the core's thread.
pub struct HttpEvent {
    conn: ConnId,
    handle: u32,
    seq: u64,
    kind: Kind,
}

enum Kind {
    Event(Event),
    Done(HttpDone),
}

pub fn is_http_request(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::HttpSend(_)
            | RequestBody::HttpCancel(_)
            | RequestBody::HttpReorder(_)
            | RequestBody::HttpCookieContainer(_)
            | RequestBody::HttpReleaseCookies(_)
            | RequestBody::HttpSetCookie(_)
    )
}

fn unsuccessful() -> ResponseBody {
    ResponseBody::HttpDone(HttpDone::default())
}

impl Core {
    /// A request of §8. `None` when the answer comes later.
    pub(super) fn http_request(
        &mut self,
        conn: ConnId,
        id: u32,
        body: RequestBody,
    ) -> Option<ResponseBody> {
        let Some(pid) = self.shim_pid_of(conn) else {
            return Some(error("the connection is closing"));
        };
        let done = Some(ResponseBody::Done(Done {}));
        match body {
            RequestBody::HttpSend(r) => self.http_send(conn, id, r),
            RequestBody::HttpCancel(r) => {
                if let Some(lane) = self.http.lanes.get_mut(&conn) {
                    lane.waiting.retain(|w| w.request.handle != r.handle);
                    if let Some(run) = lane.running.get_mut(&r.handle) {
                        run.cancelled = true;
                        run.stop.cancel();
                    }
                }
                done
            }
            RequestBody::HttpReorder(r) => {
                if let Some(lane) = self.http.lanes.get_mut(&conn)
                    && let Some(i) = lane
                        .waiting
                        .iter()
                        .position(|w| w.request.handle == r.handle)
                {
                    let w = lane.waiting.remove(i).expect("found");
                    if r.first {
                        lane.waiting.push_front(w);
                    } else {
                        lane.waiting.push_back(w);
                    }
                }
                done
            }
            RequestBody::HttpCookieContainer(r) => {
                let count = self.http.jars.keys().filter(|(p, _)| *p == pid).count();
                if count >= CONTAINERS {
                    return Some(error("too many cookie containers"));
                }
                self.http.jars.insert(
                    (pid, r.handle),
                    Arc::new(Mutex::new(Jar::new(r.allow_modify))),
                );
                done
            }
            RequestBody::HttpReleaseCookies(r) => {
                self.http.jars.remove(&(pid, r.handle));
                done
            }
            RequestBody::HttpSetCookie(r) => {
                let Some(jar) = self.http.jars.get(&(pid, r.handle)) else {
                    return Some(error("no such cookie container"));
                };
                let uri = r
                    .url
                    .parse::<ureq::http::Uri>()
                    .ok()
                    .filter(|u| u.host().is_some_and(|h| !h.is_empty()))
                    .or_else(|| format!("http://{}/", r.host).parse().ok());
                let Some(uri) = uri else {
                    return Some(error("the cookie has no host"));
                };
                let stored = jar.lock().unwrap_or_else(|e| e.into_inner()).store(
                    &uri,
                    &r.cookie,
                    http::cookies_now(),
                );
                if stored {
                    done
                } else {
                    Some(error("the cookie was refused"))
                }
            }
            _ => Some(error("not an HTTP request")),
        }
    }

    fn http_send(&mut self, conn: ConnId, id: u32, r: HttpSendRequest) -> Option<ResponseBody> {
        let lane = self.http.lanes.entry(conn).or_default();
        if lane.running.contains_key(&r.handle)
            || lane.waiting.iter().any(|w| w.request.handle == r.handle)
        {
            return Some(error("that HTTP request was sent already"));
        }
        if lane.running.len() < RUNNING {
            self.http_start(conn, id, r);
        } else if lane.waiting.len() < WAITING {
            lane.waiting.push_back(Waiting { id, request: r });
        } else {
            tracing::info!(conn, "a game has more than {WAITING} HTTP requests waiting");
            return Some(unsuccessful());
        }
        None
    }

    /// Starts a request on a worker thread.
    fn http_start(&mut self, conn: ConnId, id: u32, request: HttpSendRequest) {
        let app = self.shim_app_of(conn).map_or(0, |a| a.0);
        let pid = self.shim_pid_of(conn).unwrap_or(0);
        let jar = (request.cookies != 0)
            .then(|| self.http.jars.get(&(pid, request.cookies)).cloned())
            .flatten();
        let agent = self.http.agent.get_or_insert_with(http::agent).clone();
        let handle = request.handle;
        self.http.next_seq += 1;
        let seq = self.http.next_seq;
        let stop = CancelToken::new();
        let all = self.http.stop.clone();
        let commands = self.games.commands.clone();
        let job = Job { app, request, jar };
        let worker_stop = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("http-request".into())
            .spawn(move || {
                // Shutdown stops every request.
                let _hook = all.on_cancel({
                    let s = worker_stop.clone();
                    move || s.cancel()
                });
                if all.is_cancelled() {
                    worker_stop.cancel();
                }
                let send = |kind| {
                    commands
                        .send(Command::Http(HttpEvent {
                            conn,
                            handle,
                            seq,
                            kind,
                        }))
                        .is_ok()
                };
                let done = http::run(&agent, job, &worker_stop, |e| send(Kind::Event(e)));
                send(Kind::Done(done));
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start a thread for an HTTP request: {e}");
            self.shim_send(
                conn,
                Message::Response(Response {
                    id,
                    body: unsuccessful(),
                }),
            );
            return;
        }
        self.http.lanes.entry(conn).or_default().running.insert(
            handle,
            Running {
                id,
                seq,
                stop,
                cancelled: false,
            },
        );
    }

    /// A worker's report (§8).
    pub(super) fn on_http(&mut self, e: HttpEvent) {
        let Some(run) = self
            .http
            .lanes
            .get(&e.conn)
            .and_then(|l| l.running.get(&e.handle))
            .filter(|r| r.seq == e.seq)
        else {
            return;
        };
        let (id, cancelled) = (run.id, run.cancelled);
        match e.kind {
            Kind::Event(_) if cancelled => {}
            Kind::Event(Event::Headers(h)) => {
                self.shim_send(e.conn, Message::Notice(NoticeBody::HttpHeaders(h)));
            }
            Kind::Event(Event::Data(data)) => {
                self.shim_send(
                    e.conn,
                    Message::Notice(NoticeBody::HttpData(ossteam_gameipc::HttpData {
                        handle: e.handle,
                        data,
                    })),
                );
            }
            Kind::Done(d) => {
                if !cancelled {
                    self.shim_send(
                        e.conn,
                        Message::Response(Response {
                            id,
                            body: ResponseBody::HttpDone(d),
                        }),
                    );
                }
                self.http_next(e.conn, e.handle);
            }
        }
    }

    /// Frees a request's place and starts the next one waiting.
    fn http_next(&mut self, conn: ConnId, handle: u32) {
        let Some(lane) = self.http.lanes.get_mut(&conn) else {
            return;
        };
        lane.running.remove(&handle);
        if lane.running.len() < RUNNING
            && let Some(w) = lane.waiting.pop_front()
        {
            self.http_start(conn, w.id, w.request);
        }
    }

    /// A connection closed: its requests stop.
    pub(super) fn http_disconnected(&mut self, conn: ConnId) {
        if let Some(lane) = self.http.lanes.remove(&conn) {
            for r in lane.running.values() {
                r.stop.cancel();
            }
        }
    }

    /// A process's last connection closed: its cookie containers go.
    pub(super) fn http_process_gone(&mut self, pid: u32) {
        self.http.jars.retain(|(p, _), _| *p != pid);
    }
}
