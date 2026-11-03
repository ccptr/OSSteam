// SPDX-License-Identifier: GPL-2.0-or-later
//! The model: the store, the daemon handle and the reads in flight
//! (`gui.md` §3). A plain struct on the thread that owns it: call results
//! come back as [`Reply`]s through the `post` it was made with, and its
//! timers are a [`deadline`](Model::deadline) that the owner keeps by
//! calling [`tick`](Model::tick). The GUI does both through its event
//! loop; the daemon's tests through a channel.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ossteam_ipc::v1::{AssetKind, ErrorCode, NoticeLevel, event};
use ossteam_ipc::{Client, ClientError};

use crate::daemon::{CallError, Daemon, Initial, unless_logged_out};
use crate::rate::Rates;
use crate::store::{INFO_NOTICE_TIME, LibraryQuery, Read, Store};

/// `ListApps` is read again at most this often after `AppChanged` (§3).
const LIBRARY_INTERVAL: Duration = Duration::from_millis(250);

/// `GetAsset` calls at once (§4.2).
const ASSET_CALLS: usize = 8;

/// An app image, asked for once per app and kind, and once more at
/// double size if the scale factor rises above 1 (§4.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Asset {
    Waiting,
    Ready(Arc<std::path::Path>),
    /// `GetAsset` failed; the name is shown instead.
    Missing,
}

/// Which images were asked for, and the `GetAsset` calls waiting.
#[derive(Default)]
struct Assets {
    /// Each image and whether it was last asked for at double size.
    known: HashMap<(u32, AssetKind), (Asset, bool)>,
    queue: VecDeque<(u32, AssetKind, bool)>,
    calls: usize,
}

impl Assets {
    fn get(&self, app_id: u32, kind: AssetKind) -> Option<&Asset> {
        self.known.get(&(app_id, kind)).map(|(asset, _)| asset)
    }

    /// Queues a call unless the image was asked for at this size or
    /// larger. An image asked for again at double size, after the window
    /// moved to a denser display or opened before the compositor told it
    /// its scale, keeps showing until the larger one arrives.
    fn want(&mut self, app_id: u32, kind: AssetKind, high_dpi: bool) {
        match self.known.get_mut(&(app_id, kind)) {
            Some((_, asked_high)) if *asked_high || !high_dpi => return,
            Some((_, asked_high)) => *asked_high = true,
            None => {
                self.known
                    .insert((app_id, kind), (Asset::Waiting, high_dpi));
            }
        }
        self.queue.push_back((app_id, kind, high_dpi));
    }

    /// The next call to make, newest first: what was asked for last is
    /// what is on screen.
    fn next(&mut self) -> Option<(u32, AssetKind, bool)> {
        if self.calls >= ASSET_CALLS {
            return None;
        }
        let call = self.queue.pop_back()?;
        self.calls += 1;
        Some(call)
    }

    /// Records a call's answer. A failed call at double size keeps the
    /// image already shown; an answer at single size that came after the
    /// call at double size was made is shown only until that one answers.
    fn answered(&mut self, app_id: u32, kind: AssetKind, high_dpi: bool, path: Option<PathBuf>) {
        self.calls -= 1;
        let Some((asset, asked_high)) = self.known.get_mut(&(app_id, kind)) else {
            return;
        };
        let latest = high_dpi == *asked_high;
        match path {
            Some(path) if latest || *asset == Asset::Waiting => {
                *asset = Asset::Ready(Arc::from(path));
            }
            None if latest && !matches!(asset, Asset::Ready(_)) => *asset = Asset::Missing,
            _ => {}
        }
    }
}

/// A result for the model from another thread, applied with
/// [`Model::handle`] on the thread that owns it.
pub struct Reply(Box<dyn FnOnce(&mut Model) + Send>);

/// Sends a [`Reply`] to the thread that owns the model.
pub type Post = Arc<dyn Fn(Reply) + Send + Sync>;

pub struct Model {
    pub store: Store,
    pub daemon: Daemon,
    pub rates: Rates,
    /// View state shared by the pages that show a job: why the last
    /// Pause, Resume, Cancel or move of each job failed.
    pub job_errors: HashMap<u64, String>,
    /// When `store.status` was read, to keep the daemon's uptime current.
    pub status_read: Instant,
    post: Post,
    in_flight: HashSet<Read>,
    /// Reads asked for while the same read was in flight.
    again: HashSet<Read>,
    library_started: Option<Instant>,
    /// When a library read held back by [`LIBRARY_INTERVAL`] is due.
    library_due: Option<Instant>,
    /// Asked for by views as they build what is visible, so behind a
    /// cell: building takes the model by reference.
    assets: RefCell<Assets>,
}

impl Model {
    pub fn new(daemon: Daemon, initial: Initial, post: Post) -> Self {
        Self {
            store: Store::new(initial),
            daemon,
            rates: Rates::default(),
            job_errors: HashMap::new(),
            status_read: Instant::now(),
            post,
            in_flight: HashSet::new(),
            again: HashSet::new(),
            library_started: None,
            library_due: None,
            assets: RefCell::default(),
        }
    }

    /// Applies a reply that came through `post`.
    pub fn handle(&mut self, reply: Reply) {
        (reply.0)(self);
    }

    /// When [`tick`](Self::tick) has something to do: a held-back library
    /// read, or an info notice to expire.
    pub fn deadline(&self) -> Option<Instant> {
        let notices = self
            .store
            .notices
            .iter()
            .filter(|n| n.level == NoticeLevel::Info)
            .map(|n| n.at + INFO_NOTICE_TIME)
            .min();
        match (self.library_due, notices) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Does what is due at `now`.
    pub fn tick(&mut self, now: Instant) {
        if self.library_due.is_some_and(|t| t <= now) {
            self.library_due = None;
            self.read(Read::Library);
        }
        self.store.expire_notices(now);
    }

    pub fn apply(&mut self, kind: event::Kind) {
        if let event::Kind::JobProgress(p) = &kind
            && let Some(progress) = &p.progress
        {
            self.rates.sample(p.job_id, Instant::now(), progress);
        }
        for read in self.store.apply(kind) {
            self.read(read);
        }
        let jobs = &self.store.jobs;
        self.rates.retain(|id| jobs.contains_key(&id));
    }

    /// Makes `read`, or makes it again once the one in flight is done.
    pub fn read(&mut self, read: Read) {
        if self.in_flight.contains(&read) {
            self.again.insert(read);
            return;
        }
        if read == Read::Library {
            let now = Instant::now();
            if let Some(started) = self.library_started {
                let due = started + LIBRARY_INTERVAL;
                if due > now {
                    self.library_due.get_or_insert(due);
                    return;
                }
            }
            self.library_started = Some(now);
        }
        self.in_flight.insert(read);
        match read {
            Read::Accounts => {
                self.finish(read, |c| c.list_accounts(), |m, r| m.store.set_accounts(r))
            }
            Read::Library => {
                let q = self.store.library_query.clone();
                let asked = q.clone();
                self.finish(
                    read,
                    move |c| unless_logged_out(c.list_apps(&q.filter, q.installed_only)),
                    move |m, r| m.store.set_library(&asked, r),
                );
            }
            Read::Jobs => self.finish(
                read,
                |c| unless_logged_out(c.list_jobs()),
                |m, r| m.store.set_jobs(r),
            ),
            Read::Folders => self.finish(
                read,
                |c| c.list_library_folders(),
                |m, r| m.store.set_folders(r),
            ),
            Read::App(id) => self.finish(
                read,
                move |c| Ok((c.get_app(id), c.get_launch_options(id))),
                move |m, (info, options)| {
                    m.store.set_app_info(id, info.map_err(CallError::from));
                    m.store
                        .set_launch_options(id, options.map_err(CallError::from));
                },
            ),
        }
    }

    /// Calls `f` on a call thread and hands its result back to the model.
    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
        then: impl FnOnce(&mut Self, Result<T, CallError>) + Send + 'static,
    ) {
        let post = self.post.clone();
        self.daemon
            .call(f, move |r| post(Reply(Box::new(move |m| then(m, r)))));
    }

    fn finish<T: Send + 'static>(
        &mut self,
        read: Read,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
        apply: impl FnOnce(&mut Self, T) + Send + 'static,
    ) {
        self.call(f, move |m, r| {
            m.in_flight.remove(&read);
            match r {
                Ok(v) => apply(m, v),
                Err(e) => m.background_error(&e),
            }
            if m.again.remove(&read) {
                m.read(read);
            }
        });
    }

    /// A read nobody waits on failed: an error notice, unless the
    /// connection is gone (the window says so) or the account just went.
    fn background_error(&mut self, e: &CallError) {
        if e.code.is_none() || e.is(ErrorCode::NotLoggedIn) {
            return;
        }
        self.error(e.message.clone());
    }

    /// An error with no control to sit next to (§2.3).
    pub fn error(&mut self, text: String) {
        self.store.push_notice(NoticeLevel::Error, text);
    }

    pub fn dismiss_notice(&mut self, id: u64) {
        self.store.dismiss_notice(id);
    }

    /// The image, asking for it unless it was asked for before at this
    /// size or larger. Views call this for what they show, as they build
    /// it.
    pub fn asset(&self, app_id: u32, kind: AssetKind, high_dpi: bool) -> Asset {
        let asset = {
            let mut assets = self.assets.borrow_mut();
            assets.want(app_id, kind, high_dpi);
            assets.get(app_id, kind).cloned().unwrap_or(Asset::Waiting)
        };
        self.pump_assets();
        asset
    }

    fn pump_assets(&self) {
        loop {
            let Some((app_id, kind, high_dpi)) = self.assets.borrow_mut().next() else {
                return;
            };
            self.call(
                move |c| c.get_asset(app_id, kind, high_dpi),
                move |m, r| {
                    let path = r.ok().map(PathBuf::from);
                    m.assets.get_mut().answered(app_id, kind, high_dpi, path);
                    m.pump_assets();
                },
            );
        }
    }

    pub fn set_library_query(&mut self, q: LibraryQuery) {
        if q != self.store.library_query {
            self.store.library_query = q;
            self.read(Read::Library);
        }
    }

    pub fn open_app(&mut self, app_id: u32) {
        if let Some(read) = self.store.open_app(app_id) {
            self.read(read);
        }
    }

    pub fn close_app(&mut self, app_id: u32) {
        self.store.close_app(app_id);
    }

    /// A call about `job_id`: its error is shown next to the job, and
    /// cleared by the next call about it that succeeds.
    pub fn job_call<F>(&mut self, job_id: u64, f: F)
    where
        F: FnOnce(&Client) -> Result<(), ClientError> + Send + 'static,
    {
        self.call(f, move |m, r| {
            match r {
                Ok(()) => m.job_errors.remove(&job_id),
                Err(e) => m.job_errors.insert(job_id, e.message),
            };
        });
    }

    /// Makes a call whose failure has no control to show it next to: it
    /// becomes an error notice. `done` gets the result on success.
    pub fn run<T, F>(&mut self, f: F, done: impl FnOnce(&mut Self, T) + Send + 'static)
    where
        T: Send + 'static,
        F: FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    {
        self.call(f, move |m, r| match r {
            Ok(v) => done(m, v),
            Err(e) => m.error(e.message),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: u32 = 10;

    fn ready(path: &str) -> Asset {
        Asset::Ready(Arc::from(PathBuf::from(path)))
    }

    fn calls(assets: &mut Assets) -> Vec<(u32, AssetKind, bool)> {
        std::iter::from_fn(|| assets.next()).collect()
    }

    #[test]
    fn an_image_is_asked_for_once_per_size() {
        let mut a = Assets::default();
        a.want(APP, AssetKind::Capsule, false);
        a.want(APP, AssetKind::Capsule, false);
        assert_eq!(calls(&mut a), [(APP, AssetKind::Capsule, false)]);
        assert_eq!(a.get(APP, AssetKind::Capsule), Some(&Asset::Waiting));
        a.answered(APP, AssetKind::Capsule, false, Some("1x".into()));
        assert_eq!(a.get(APP, AssetKind::Capsule), Some(&ready("1x")));

        // The scale factor rose above 1: once more, at double size, and
        // the image at single size shows until that one arrives.
        a.want(APP, AssetKind::Capsule, true);
        a.want(APP, AssetKind::Capsule, true);
        assert_eq!(calls(&mut a), [(APP, AssetKind::Capsule, true)]);
        assert_eq!(a.get(APP, AssetKind::Capsule), Some(&ready("1x")));
        a.answered(APP, AssetKind::Capsule, true, Some("2x".into()));
        assert_eq!(a.get(APP, AssetKind::Capsule), Some(&ready("2x")));

        // Back at scale 1, the larger image does.
        a.want(APP, AssetKind::Capsule, false);
        assert_eq!(calls(&mut a), []);
        assert_eq!(a.calls, 0);
    }

    #[test]
    fn a_failed_call_at_double_size_keeps_the_image_shown() {
        let mut a = Assets::default();
        a.want(APP, AssetKind::Hero, false);
        calls(&mut a);
        a.answered(APP, AssetKind::Hero, false, Some("1x".into()));
        a.want(APP, AssetKind::Hero, true);
        calls(&mut a);
        a.answered(APP, AssetKind::Hero, true, None);
        assert_eq!(a.get(APP, AssetKind::Hero), Some(&ready("1x")));

        a.want(APP, AssetKind::Logo, true);
        calls(&mut a);
        a.answered(APP, AssetKind::Logo, true, None);
        assert_eq!(a.get(APP, AssetKind::Logo), Some(&Asset::Missing));
    }

    #[test]
    fn an_answer_at_single_size_never_replaces_one_at_double_size() {
        // Both calls in flight; the one at double size answers first.
        let mut a = Assets::default();
        a.want(APP, AssetKind::Capsule, false);
        a.want(APP, AssetKind::Capsule, true);
        assert_eq!(calls(&mut a).len(), 2);
        a.answered(APP, AssetKind::Capsule, true, Some("2x".into()));
        a.answered(APP, AssetKind::Capsule, false, Some("1x".into()));
        assert_eq!(a.get(APP, AssetKind::Capsule), Some(&ready("2x")));

        // The one at single size answers first: shown until the other
        // does.
        let mut a = Assets::default();
        a.want(APP, AssetKind::Hero, false);
        a.want(APP, AssetKind::Hero, true);
        calls(&mut a);
        a.answered(APP, AssetKind::Hero, false, Some("1x".into()));
        assert_eq!(a.get(APP, AssetKind::Hero), Some(&ready("1x")));
        a.answered(APP, AssetKind::Hero, true, Some("2x".into()));
        assert_eq!(a.get(APP, AssetKind::Hero), Some(&ready("2x")));

        // Its failure leaves the image to the call at double size.
        let mut a = Assets::default();
        a.want(APP, AssetKind::Logo, false);
        a.want(APP, AssetKind::Logo, true);
        calls(&mut a);
        a.answered(APP, AssetKind::Logo, false, None);
        assert_eq!(a.get(APP, AssetKind::Logo), Some(&Asset::Waiting));
        a.answered(APP, AssetKind::Logo, true, Some("2x".into()));
        assert_eq!(a.get(APP, AssetKind::Logo), Some(&ready("2x")));
    }

    #[test]
    fn at_most_eight_calls_at_once() {
        let mut a = Assets::default();
        for app in 0..10 {
            a.want(app, AssetKind::Capsule, true);
        }
        let first = calls(&mut a);
        assert_eq!(first.len(), ASSET_CALLS);
        // Newest first.
        assert_eq!(first[0], (9, AssetKind::Capsule, true));
        a.answered(9, AssetKind::Capsule, true, None);
        assert_eq!(calls(&mut a), [(1, AssetKind::Capsule, true)]);
    }
}
