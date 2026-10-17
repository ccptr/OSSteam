// SPDX-License-Identifier: GPL-2.0-or-later
//! The PICS worker: licenses, product info, change polling and the library
//! (`pics.md` §1 to §7; SPEC PICS-1 to PICS-3).
//!
//! PICS requests take seconds and bringing a large library up to date
//! takes many of them, so this runs on its own thread instead of the
//! session worker's. The session worker tells it which account is active,
//! hands it a [`Requester`] after each logon, and forwards the license
//! list. It reports [`Update`]s to the core, which answers library queries
//! from the last snapshot without waiting on the network (CONC-1).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};
use ossteam_net::{CallError, Requester};
use ossteam_pics::cache::Kind;
use ossteam_pics::info::{AppSummary, PackageInfo};
use ossteam_pics::library::{Library, granted_packages};
use ossteam_pics::request::{self, Changes, Fetched, RequestError, Want};
use ossteam_pics::{AppInfo, Cache, Info, License};
use ossteam_types::{AppId, CancelToken, PackageId, SteamId};

use crate::core::Command;

/// How often to ask what changed while logged on (`pics.md` §4).
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// The worker's wait when nothing is scheduled.
const IDLE: Duration = Duration::from_secs(3600);

pub enum Cmd {
    /// The active account changed, or there is none any more. The previous
    /// account's licenses and library are dropped.
    Account(Option<SteamId>),
    /// `steam_id` logged on; requests go through `requester` until it
    /// closes.
    LoggedOn {
        steam_id: SteamId,
        requester: Requester,
    },
    /// The license list, which replaces any earlier one (`pics.md` §1).
    Licenses(Vec<License>),
}

/// The library as the core serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub steam_id: SteamId,
    pub library: Library,
    /// The license list has arrived and everything the library needs has
    /// been fetched, or refused by Steam. False while fetching, and after
    /// a failed fetch until the retry succeeds.
    pub complete: bool,
    /// Apps granted by a license with the low-violence flag (`pics.md` §1),
    /// which take low-violence depots (`content.md` §6).
    pub low_violence: BTreeSet<AppId>,
}

/// What the worker tells the core.
pub struct Update {
    /// `None` when there is no active account.
    pub snapshot: Option<Arc<Snapshot>>,
    /// Apps whose info was fetched anew since the last update.
    pub refreshed: Vec<AppId>,
}

/// The core's side of the worker.
pub struct Handle {
    tx: Option<Sender<Cmd>>,
    stop: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// `poll_interval` is [`POLL_INTERVAL`] in the daemon.
    pub fn spawn(
        cache: Cache,
        poll_interval: Duration,
        core: Sender<Command>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let stop = CancelToken::new();
        let worker = Worker::new(cache, poll_interval, core, stop.clone());
        let thread = std::thread::Builder::new()
            .name("pics".into())
            .spawn(move || worker.run(&rx))?;
        Ok(Self {
            tx: Some(tx),
            stop,
            thread: Some(thread),
        })
    }

    /// A sender for the session worker.
    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone().expect("not shut down")
    }

    /// Interrupts any request in flight and waits for the thread to end.
    /// Later calls do nothing.
    pub fn shutdown(&mut self) {
        self.stop.cancel();
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct PackageEntry {
    change_number: u32,
    missing_token: bool,
    apps: Vec<AppId>,
}

struct AppEntry {
    change_number: u32,
    missing_token: bool,
    summary: AppSummary,
    /// The apps its depots are shared from (`depotfromapp`).
    depot_sources: Vec<AppId>,
}

/// What the cache holds for an id, as far as the worker has looked.
/// `None` in the maps below means the cache has no usable entry.
type Known<K, V> = HashMap<K, Option<V>>;

struct Worker {
    core: Sender<Command>,
    cache: Cache,
    stop: CancelToken,
    account: Option<SteamId>,
    requester: Option<Requester>,
    licenses: Option<Vec<License>>,
    /// Access tokens for this session, never written to disk (`pics.md`
    /// §2). Packages' come with the licenses; apps' are asked for.
    package_tokens: HashMap<PackageId, u64>,
    app_tokens: HashMap<AppId, u64>,
    /// Apps whose token was asked for, whether or not one was granted.
    tokens_asked: HashSet<AppId>,
    packages: Known<PackageId, PackageEntry>,
    apps: Known<AppId, AppEntry>,
    /// Ids Steam does not know or sent no usable info for. They are not
    /// asked for again until the next full check.
    unavailable_packages: HashSet<PackageId>,
    unavailable_apps: HashSet<AppId>,
    change_number: u32,
    /// The next update is a full check (`pics.md` §5): the licenses
    /// changed, or the last attempt failed.
    check_due: bool,
    /// When to run the next update.
    next_poll: Option<Instant>,
    poll_interval: Duration,
    refreshed: Vec<AppId>,
    published: Option<Arc<Snapshot>>,
}

impl Worker {
    fn new(
        cache: Cache,
        poll_interval: Duration,
        core: Sender<Command>,
        stop: CancelToken,
    ) -> Self {
        let change_number = cache.change_number();
        Self {
            core,
            poll_interval,
            cache,
            stop,
            account: None,
            requester: None,
            licenses: None,
            package_tokens: HashMap::new(),
            app_tokens: HashMap::new(),
            tokens_asked: HashSet::new(),
            packages: HashMap::new(),
            apps: HashMap::new(),
            unavailable_packages: HashSet::new(),
            unavailable_apps: HashSet::new(),
            change_number,
            check_due: false,
            next_poll: None,
            refreshed: Vec::new(),
            published: None,
        }
    }

    fn run(mut self, cmds: &Receiver<Cmd>) {
        loop {
            let timeout = self
                .next_poll
                .map_or(IDLE, |t| t.saturating_duration_since(Instant::now()));
            select! {
                recv(cmds) -> cmd => match cmd {
                    Ok(cmd) => self.on_cmd(cmd),
                    Err(_) => return,
                },
                default(timeout) => {}
            }
            // Act on the latest state only: a burst of commands, such as
            // a logon followed by its license list, needs one pass.
            while let Ok(cmd) = cmds.try_recv() {
                self.on_cmd(cmd);
            }
            if self.stop.is_cancelled() {
                return;
            }
            // Clients see what the cache has while the fetches run.
            self.publish();
            self.work();
            self.publish();
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Account(steam_id) => self.set_account(steam_id),
            Cmd::LoggedOn {
                steam_id,
                requester,
            } => {
                self.set_account(Some(steam_id));
                self.requester = Some(requester);
                // Each logon brings a license list, which starts the full
                // check; until then the earlier one is still in force.
                self.next_poll = Some(Instant::now() + self.poll_interval);
            }
            Cmd::Licenses(licenses) => {
                let Some(account) = self.account else {
                    return;
                };
                let own = account.account_id();
                let borrowed = licenses.iter().filter(|l| l.is_borrowed(own)).count();
                let void = licenses.iter().filter(|l| l.is_void()).count();
                tracing::info!(
                    licenses = licenses.len(),
                    borrowed,
                    void,
                    "license list received"
                );
                self.package_tokens = licenses
                    .iter()
                    .filter(|l| l.access_token != 0)
                    .map(|l| (l.package_id, l.access_token))
                    .collect();
                self.licenses = Some(licenses);
                self.check_due = true;
                self.next_poll = Some(Instant::now());
            }
        }
    }

    fn set_account(&mut self, steam_id: Option<SteamId>) {
        if self.account == steam_id {
            return;
        }
        self.account = steam_id;
        self.requester = None;
        self.licenses = None;
        self.package_tokens.clear();
        self.app_tokens.clear();
        self.tokens_asked.clear();
        self.unavailable_packages.clear();
        self.unavailable_apps.clear();
        self.check_due = false;
        self.next_poll = None;
    }

    fn work(&mut self) {
        let Some(requester) = self.requester.clone() else {
            return;
        };
        if requester.is_closed() {
            // The session reconnects and sends a new requester.
            self.requester = None;
            self.next_poll = None;
            return;
        }
        if self.licenses.is_none() || self.next_poll.is_none_or(|t| Instant::now() < t) {
            return;
        }
        let result = if self.check_due {
            self.bring_up_to_date(&requester, None)
        } else {
            self.poll(&requester)
        };
        match result {
            Ok(()) => self.check_due = false,
            Err(RequestError::Call(CallError::Cancelled)) => return,
            Err(RequestError::Call(CallError::Closed)) => {
                // Finished after the next logon.
                tracing::info!("the connection closed during a PICS update");
                self.check_due = true;
                self.requester = None;
                self.next_poll = None;
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "PICS update failed: {e}; retrying in {:?}",
                    self.poll_interval
                );
                self.check_due = true;
            }
        }
        self.next_poll = Some(Instant::now() + self.poll_interval);
    }

    /// Asks what changed and fetches what the library uses (`pics.md` §4).
    fn poll(&mut self, requester: &Requester) -> Result<(), RequestError> {
        let changes = request::changes_since(requester, self.change_number, &self.stop)?;
        if changes.current == self.change_number {
            return Ok(());
        }
        tracing::debug!(
            since = self.change_number,
            current = changes.current,
            apps = changes.apps.len(),
            packages = changes.packages.len(),
            "PICS changes"
        );
        self.bring_up_to_date(requester, Some(&changes))
    }

    /// Fetches the info of every granted package and every app they grant
    /// that is missing from the cache or stale. With `changes`, only the
    /// objects listed there are checked for staleness, unless they say the
    /// list is incomplete; without, every one is (`pics.md` §5).
    fn bring_up_to_date(
        &mut self,
        requester: &Requester,
        changes: Option<&Changes>,
    ) -> Result<(), RequestError> {
        let current = match changes {
            Some(c) => c.current,
            None => {
                // Only to learn the current change number for later polls.
                self.unavailable_packages.clear();
                self.unavailable_apps.clear();
                request::changes_since(requester, self.change_number, &self.stop)?.current
            }
        };

        let packages = self.granted();
        for &p in &packages {
            self.load_package(p);
        }
        let check_all = changes.is_none_or(|c| c.full_packages);
        let listed: HashMap<u32, u32> = changes
            .map(|c| c.packages.iter().map(|&(p, n)| (p.0, n)).collect())
            .unwrap_or_default();
        let wants: Vec<Want> = packages
            .iter()
            .filter(|p| !self.unavailable_packages.contains(p))
            .map(|&p| Want {
                id: p.0,
                token: self.package_tokens.get(&p).copied().unwrap_or(0),
            })
            .collect();
        let known = |id: u32| {
            self.packages
                .get(&PackageId(id))
                .and_then(Option::as_ref)
                .map(|e| (e.change_number, e.missing_token))
        };
        let stale = self.stale(requester, &wants, check_all, &listed, known, Kind::Package)?;
        if !stale.is_empty() {
            tracing::info!(count = stale.len(), "fetching package info");
            let fetched = request::product_info(requester, &[], &stale, false, &self.stop)?;
            self.take_packages(fetched);
        }

        let apps: BTreeSet<AppId> = packages
            .iter()
            .filter_map(|p| self.packages.get(p).and_then(Option::as_ref))
            .flat_map(|e| e.apps.iter().copied())
            .collect();
        self.update_apps(requester, &apps, changes)?;
        // Apps whose depots the library's apps share (`content.md` §6,
        // rule 7). They are not in the library, but installs read their
        // manifests, and update detection their changes.
        let sources: BTreeSet<AppId> = apps
            .iter()
            .filter_map(|a| self.apps.get(a).and_then(Option::as_ref))
            .flat_map(|e| e.depot_sources.iter().copied())
            .filter(|a| !apps.contains(a))
            .collect();
        if !sources.is_empty() {
            self.update_apps(requester, &sources, changes)?;
        }

        self.change_number = current;
        if let Err(e) = self.cache.set_change_number(current) {
            tracing::warn!("cannot save the PICS change number: {e}");
        }
        Ok(())
    }

    /// Requests access tokens for `apps` not asked about yet, then fetches
    /// the info of those that are missing from the cache or stale, as
    /// [`Self::bring_up_to_date`] does for packages.
    fn update_apps(
        &mut self,
        requester: &Requester,
        apps: &BTreeSet<AppId>,
        changes: Option<&Changes>,
    ) -> Result<(), RequestError> {
        let new: Vec<AppId> = apps
            .iter()
            .filter(|a| !self.tokens_asked.contains(a))
            .copied()
            .collect();
        if !new.is_empty() {
            let t = request::app_tokens(requester, &new, &self.stop)?;
            if !t.denied.is_empty() {
                tracing::info!(count = t.denied.len(), "access tokens denied");
            }
            self.app_tokens
                .extend(t.tokens.into_iter().filter(|&(_, token)| token != 0));
            self.tokens_asked.extend(new);
        }
        for &a in apps {
            self.load_app(a);
        }
        let check_all = changes.is_none_or(|c| c.full_apps);
        let listed: HashMap<u32, u32> = changes
            .map(|c| c.apps.iter().map(|&(a, n)| (a.0, n)).collect())
            .unwrap_or_default();
        let wants: Vec<Want> = apps
            .iter()
            .filter(|a| !self.unavailable_apps.contains(a))
            .map(|&a| Want {
                id: a.0,
                token: self.app_tokens.get(&a).copied().unwrap_or(0),
            })
            .collect();
        let known = |id: u32| {
            self.apps
                .get(&AppId(id))
                .and_then(Option::as_ref)
                .map(|e| (e.change_number, e.missing_token))
        };
        let stale = self.stale(requester, &wants, check_all, &listed, known, Kind::App)?;
        if !stale.is_empty() {
            tracing::info!(count = stale.len(), "fetching app info");
            let fetched = request::product_info(requester, &stale, &[], false, &self.stop)?;
            self.take_apps(fetched);
        }
        Ok(())
    }

    /// Which of `wants` need fetching. An entry is stale when it is not
    /// cached, when its change number differs from Steam's, or when it was
    /// fetched without a token that is now available. With `check_all`,
    /// Steam's change numbers come from a metadata request; otherwise from
    /// `listed`, and entries not listed there are current.
    fn stale(
        &self,
        requester: &Requester,
        wants: &[Want],
        check_all: bool,
        listed: &HashMap<u32, u32>,
        known: impl Fn(u32) -> Option<(u32, bool)>,
        kind: Kind,
    ) -> Result<Vec<Want>, RequestError> {
        let latest: HashMap<u32, u32> = if check_all {
            let cached: Vec<Want> = wants
                .iter()
                .filter(|w| known(w.id).is_some())
                .copied()
                .collect();
            let meta = match kind {
                Kind::App => request::product_info(requester, &cached, &[], true, &self.stop)?,
                Kind::Package => request::product_info(requester, &[], &cached, true, &self.stop)?,
            };
            let infos = match kind {
                Kind::App => meta.apps,
                Kind::Package => meta.packages,
            };
            infos.iter().map(|i| (i.id, i.change_number)).collect()
        } else {
            listed.clone()
        };
        Ok(wants
            .iter()
            .filter(|w| match known(w.id) {
                None => true,
                Some((number, missing_token)) => {
                    latest.get(&w.id).is_some_and(|&n| n != number)
                        || (missing_token && w.token != 0)
                }
            })
            .copied()
            .collect())
    }

    fn take_packages(&mut self, fetched: Fetched) {
        for info in fetched.packages {
            if let Err(e) = self.cache.store(Kind::Package, &info) {
                tracing::warn!(package = info.id, "cannot cache package info: {e}");
            }
            self.packages
                .insert(PackageId(info.id), Some(package_entry(&info)));
        }
        for id in fetched
            .unknown_packages
            .into_iter()
            .chain(fetched.unavailable_packages)
        {
            self.unavailable_packages.insert(PackageId(id));
        }
    }

    fn take_apps(&mut self, fetched: Fetched) {
        for info in fetched.apps {
            if let Err(e) = self.cache.store(Kind::App, &info) {
                tracing::warn!(app = info.id, "cannot cache app info: {e}");
            }
            self.refreshed.push(AppId(info.id));
            self.apps.insert(AppId(info.id), Some(app_entry(&info)));
        }
        for id in fetched
            .unknown_apps
            .into_iter()
            .chain(fetched.unavailable_apps)
        {
            self.unavailable_apps.insert(AppId(id));
        }
    }

    fn load_package(&mut self, id: PackageId) {
        if !self.packages.contains_key(&id) {
            let entry = self
                .cache
                .load(Kind::Package, id.0)
                .map(|i| package_entry(&i));
            self.packages.insert(id, entry);
        }
    }

    fn load_app(&mut self, id: AppId) {
        if !self.apps.contains_key(&id) {
            let entry = self.cache.load(Kind::App, id.0).map(|i| app_entry(&i));
            self.apps.insert(id, entry);
        }
    }

    fn granted(&self) -> Vec<PackageId> {
        match (&self.licenses, self.account) {
            (Some(l), Some(a)) => granted_packages(l, a.account_id()),
            _ => Vec::new(),
        }
    }

    /// Reports the library if it changed, or if apps were refreshed.
    fn publish(&mut self) {
        let snapshot = self.account.map(|steam_id| {
            let packages = self.granted();
            for &p in &packages {
                self.load_package(p);
            }
            let apps: Vec<AppId> = packages
                .iter()
                .filter_map(|p| self.packages.get(p).and_then(Option::as_ref))
                .flat_map(|e| e.apps.clone())
                .collect();
            for a in apps {
                self.load_app(a);
            }
            let library = Library::build(
                &packages,
                |p| {
                    self.packages
                        .get(&p)
                        .and_then(Option::as_ref)
                        .map(|e| e.apps.clone())
                },
                |a| {
                    self.apps
                        .get(&a)
                        .and_then(Option::as_ref)
                        .map(|e| e.summary.clone())
                },
            );
            let low_violence = self
                .licenses
                .iter()
                .flatten()
                .filter(|l| {
                    l.grants(steam_id.account_id())
                        && l.flags & ossteam_pics::license::flags::LOW_VIOLENCE != 0
                })
                .filter_map(|l| self.packages.get(&l.package_id).and_then(Option::as_ref))
                .flat_map(|e| e.apps.iter().copied())
                .collect();
            Arc::new(Snapshot {
                steam_id,
                library,
                complete: self.licenses.is_some() && !self.check_due,
                low_violence,
            })
        });
        let same = match (&snapshot, &self.published) {
            (Some(a), Some(b)) => a == b,
            (None, None) => true,
            _ => false,
        };
        if same && self.refreshed.is_empty() {
            return;
        }
        if let Some(s) = &snapshot
            && s.complete
            && !same
        {
            tracing::info!(apps = s.library.len(), "library ready");
        }
        self.published.clone_from(&snapshot);
        let update = Update {
            snapshot,
            refreshed: std::mem::take(&mut self.refreshed),
        };
        let _ = self.core.send(Command::Pics(update));
    }
}

fn package_entry(info: &Info) -> PackageEntry {
    PackageEntry {
        change_number: info.change_number,
        missing_token: info.missing_token,
        apps: PackageInfo(&info.data).app_ids(),
    }
}

fn app_entry(info: &Info) -> AppEntry {
    AppEntry {
        change_number: info.change_number,
        missing_token: info.missing_token,
        summary: AppInfo(&info.data).summary(),
        depot_sources: AppInfo(&info.data)
            .depots()
            .into_iter()
            .filter_map(|d| d.from_app)
            .filter(|a| a.0 != 0)
            .collect(),
    }
}
