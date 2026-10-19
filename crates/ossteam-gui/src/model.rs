// SPDX-License-Identifier: GPL-2.0-or-later
//! The model: the store, the daemon handle and the reads in flight, in a
//! gpui entity that the views observe (`gui.md` §3).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use ossteam_ipc::v1::{AssetKind, ErrorCode, NoticeLevel, event};
use ossteam_ipc::{Client, ClientError};

use crate::daemon::{CallError, Daemon, Initial, unless_logged_out};
use crate::rate::Rates;
use crate::store::{INFO_NOTICE_TIME, LibraryQuery, Read, Store};

/// `ListApps` is read again at most this often after `AppChanged` (§3).
const LIBRARY_INTERVAL: Duration = Duration::from_millis(250);

/// `GetAsset` calls at once (§4.2).
const ASSET_CALLS: usize = 8;

/// An app image, asked for once per app and kind (§4.2).
#[derive(Debug, Clone)]
pub enum Asset {
    Waiting,
    Ready(Arc<std::path::Path>),
    /// `GetAsset` failed; the name is shown instead.
    Missing,
}

#[derive(Default)]
struct Assets {
    known: HashMap<(u32, AssetKind), Asset>,
    queue: VecDeque<(u32, AssetKind, bool)>,
    calls: usize,
}

pub struct Model {
    pub store: Store,
    pub daemon: Daemon,
    pub rates: Rates,
    /// View state shared by the pages that show a job: why the last
    /// Pause, Resume, Cancel or move of each job failed.
    pub job_errors: HashMap<u64, String>,
    /// When `store.status` was read, to keep the daemon's uptime current.
    pub status_read: Instant,
    in_flight: HashSet<Read>,
    /// Reads asked for while the same read was in flight.
    again: HashSet<Read>,
    library_started: Option<Instant>,
    library_timer: Option<Task<()>>,
    assets: Assets,
}

impl Model {
    pub fn new(daemon: Daemon, initial: Initial) -> Self {
        Self {
            store: Store::new(initial),
            daemon,
            rates: Rates::default(),
            job_errors: HashMap::new(),
            status_read: Instant::now(),
            in_flight: HashSet::new(),
            again: HashSet::new(),
            library_started: None,
            library_timer: None,
            assets: Assets::default(),
        }
    }

    pub fn apply(&mut self, kind: event::Kind, cx: &mut Context<Self>) {
        if let event::Kind::JobProgress(p) = &kind
            && let Some(progress) = &p.progress
        {
            self.rates.sample(p.job_id, Instant::now(), progress);
        }
        let notice = matches!(kind, event::Kind::Notice(_));
        for read in self.store.apply(kind) {
            self.read(read, cx);
        }
        if notice {
            self.expire_notices_later(cx);
        }
        let jobs = &self.store.jobs;
        self.rates.retain(|id| jobs.contains_key(&id));
        cx.notify();
    }

    /// Makes `read`, or makes it again once the one in flight is done.
    pub fn read(&mut self, read: Read, cx: &mut Context<Self>) {
        if self.in_flight.contains(&read) {
            self.again.insert(read);
            return;
        }
        if read == Read::Library {
            let wait = self
                .library_started
                .map(|t| LIBRARY_INTERVAL.saturating_sub(t.elapsed()))
                .unwrap_or_default();
            if !wait.is_zero() {
                if self.library_timer.is_none() {
                    self.library_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(wait).await;
                        let _ = this.update(cx, |m, cx| {
                            m.library_timer = None;
                            m.read(Read::Library, cx);
                        });
                    }));
                }
                return;
            }
            self.library_started = Some(Instant::now());
        }
        self.in_flight.insert(read);
        let d = self.daemon.clone();
        match read {
            Read::Accounts => {
                let call = d.call(|c| c.list_accounts());
                self.finish(read, call, cx, |m, r| m.store.set_accounts(r));
            }
            Read::Library => {
                let q = self.store.library_query.clone();
                let filter = q.filter.clone();
                let call =
                    d.call(move |c| unless_logged_out(c.list_apps(&filter, q.installed_only)));
                let q = self.store.library_query.clone();
                self.finish(read, call, cx, move |m, r| m.store.set_library(&q, r));
            }
            Read::Jobs => {
                let call = d.call(|c| unless_logged_out(c.list_jobs()));
                self.finish(read, call, cx, |m, r| m.store.set_jobs(r));
            }
            Read::Folders => {
                let call = d.call(|c| c.list_library_folders());
                self.finish(read, call, cx, |m, r| m.store.set_folders(r));
            }
            Read::App(id) => {
                let call = d.call(move |c| Ok((c.get_app(id), c.get_launch_options(id))));
                self.finish(read, call, cx, move |m, (info, options)| {
                    m.store.set_app_info(id, info.map_err(CallError::from));
                    m.store
                        .set_launch_options(id, options.map_err(CallError::from));
                });
            }
        }
    }

    fn finish<T: Send + 'static>(
        &mut self,
        read: Read,
        call: impl std::future::Future<Output = Result<T, CallError>> + 'static,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut Self, T) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |m, cx| {
                m.in_flight.remove(&read);
                match r {
                    Ok(v) => apply(m, v),
                    Err(e) => m.background_error(&e, cx),
                }
                if m.again.remove(&read) {
                    m.read(read, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A read nobody waits on failed: an error notice, unless the
    /// connection is gone (the window says so) or the account just went.
    fn background_error(&mut self, e: &CallError, cx: &mut Context<Self>) {
        if e.code.is_none() || e.is(ErrorCode::NotLoggedIn) {
            return;
        }
        self.error(e.message.clone(), cx);
    }

    /// An error with no control to sit next to (§2.3).
    pub fn error(&mut self, text: String, cx: &mut Context<Self>) {
        self.store.push_notice(NoticeLevel::Error, text);
        cx.notify();
    }

    fn expire_notices_later(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INFO_NOTICE_TIME).await;
            let _ = this.update(cx, |m, cx| {
                if m.store.expire_notices(Instant::now()) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn dismiss_notice(&mut self, id: u64, cx: &mut Context<Self>) {
        self.store.dismiss_notice(id);
        cx.notify();
    }

    pub fn asset(&self, app_id: u32, kind: AssetKind) -> Option<&Asset> {
        self.assets.known.get(&(app_id, kind))
    }

    /// Asks for an image unless it was asked for before. Views call this
    /// while drawing what is visible; it does not notify until the image
    /// arrives.
    pub fn want_asset(
        &mut self,
        app_id: u32,
        kind: AssetKind,
        high_dpi: bool,
        cx: &mut Context<Self>,
    ) {
        if self.assets.known.contains_key(&(app_id, kind)) {
            return;
        }
        self.assets.known.insert((app_id, kind), Asset::Waiting);
        self.assets.queue.push_back((app_id, kind, high_dpi));
        self.pump_assets(cx);
    }

    fn pump_assets(&mut self, cx: &mut Context<Self>) {
        while self.assets.calls < ASSET_CALLS
            // Newest first: what was asked for last is what is on screen.
            && let Some((app_id, kind, high_dpi)) = self.assets.queue.pop_back()
        {
            self.assets.calls += 1;
            let call = self
                .daemon
                .call(move |c| c.get_asset(app_id, kind, high_dpi));
            cx.spawn(async move |this, cx| {
                let r = call.await;
                let _ = this.update(cx, |m, cx| {
                    m.assets.calls -= 1;
                    let asset = match r {
                        Ok(path) => Asset::Ready(Arc::from(PathBuf::from(path))),
                        Err(_) => Asset::Missing,
                    };
                    m.assets.known.insert((app_id, kind), asset);
                    m.pump_assets(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn set_library_query(&mut self, q: LibraryQuery, cx: &mut Context<Self>) {
        if q != self.store.library_query {
            self.store.library_query = q;
            self.read(Read::Library, cx);
        }
    }

    pub fn open_app(&mut self, app_id: u32, cx: &mut Context<Self>) {
        if let Some(read) = self.store.open_app(app_id) {
            self.read(read, cx);
        }
    }

    /// A call about `job_id`: its error is shown next to the job, and
    /// cleared by the next call about it that succeeds.
    pub fn job_call<F>(&mut self, job_id: u64, cx: &mut Context<Self>, f: F)
    where
        F: FnOnce(&Client) -> Result<(), ClientError> + Send + 'static,
    {
        let call = self.daemon.call(f);
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(()) => m.job_errors.remove(&job_id),
                    Err(e) => m.job_errors.insert(job_id, e.message),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn close_app(&mut self, app_id: u32) {
        self.store.close_app(app_id);
    }

    /// Makes a call whose failure has no control to show it next to: it
    /// becomes an error notice. `done` gets the result on success.
    pub fn run<T, F>(
        &mut self,
        cx: &mut Context<Self>,
        f: F,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    {
        let call = self.daemon.call(f);
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(v) => done(m, v, cx),
                    Err(e) => m.error(e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
