// SPDX-License-Identifier: GPL-2.0-or-later
//! One content job, run on a thread of its own (`content.md` §6 to §12).
//!
//! The core gives a job everything it needs from the daemon's state up
//! front ([`Spec`]), so the job never touches the database. It reports
//! progress, its journal and its result back to the core.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use ossteam_content::cdn::{self, Fetch, Rotation};
use ossteam_content::depots::{self, ChooseError, Host, Wanted};
use ossteam_content::download::{DownloadError, Downloader, Tokens};
use ossteam_content::engine::{self, DepotWork, EngineError, Hooks, Mode, Phase, Removal};
use ossteam_content::request::{self, RequestError};
use ossteam_content::{DepotKey, Manifest, path};
use ossteam_ipc::v1::{self, ErrorCode};
use ossteam_net::{CallError, Requester};
use ossteam_proto::steam::CContentServerDirectoryServerInfo;
use ossteam_types::{AppId, CancelToken, DepotId, ManifestId};
use ossteam_vdf::{Object, Value};

use super::folders::Layout;
use super::script::{self, System};
use super::{Kind, Report};
use crate::core::Command;
use crate::db::{InstalledApp, InstalledDepot};

/// The server list is fetched again after this long (`content.md` §1).
const SERVER_LIST_TTL: Duration = Duration::from_secs(3600);

/// Everything a job needs, taken from the daemon's state when it starts.
pub struct Spec {
    pub job_id: u64,
    pub kind: Kind,
    pub app: AppId,
    pub branch: String,
    pub folder: Layout,
    pub folder_id: String,
    /// The app's info (the contents of `appinfo`).
    pub info: Object,
    /// The info of other apps the library has, for depots shared from
    /// them.
    pub others: HashMap<AppId, Object>,
    /// The apps the library holds, for DLC depots.
    pub owned: HashSet<AppId>,
    /// The optional DLC turned on for the app (`content.optional_dlc`).
    pub optional_dlc: HashSet<AppId>,
    pub low_violence: bool,
    pub language: String,
    /// The platform whose depots the install takes (`compat-tools.md` §4).
    pub host: Host,
    /// What is installed now.
    pub installed: Option<InstalledApp>,
    /// Every other installed app, for uninstalling shared depots.
    pub other_installs: Vec<InstalledApp>,
    pub journal: Option<Vec<u8>>,
    /// An uninstall deletes `compatdata/<app id>` too.
    pub remove_compat_data: bool,
    pub threads: usize,
    pub cell_id: u32,
}

/// Shared by every job.
pub struct Env {
    pub requester: Requester,
    pub fetch: Arc<dyn Fetch>,
    pub servers: Arc<ServerList>,
    /// What install scripts act on; `None` where they do not apply.
    pub scripts: Option<Arc<dyn System>>,
    pub core: Sender<Command>,
}

/// How the core steers a running job.
#[derive(Default)]
pub struct Control {
    pub cancel: CancelToken,
    /// Set before `cancel` when the job's staging is to be deleted.
    pub discard: AtomicBool,
}

/// Why a job did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Cancelled,
    Error { code: ErrorCode, message: String },
}

fn fail(code: ErrorCode, message: impl Into<String>) -> Failure {
    Failure::Error {
        code,
        message: message.into(),
    }
}

impl From<RequestError> for Failure {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Call(CallError::Cancelled) => Failure::Cancelled,
            e => fail(
                ErrorCode::Network,
                format!("a request to Steam failed: {e}"),
            ),
        }
    }
}

impl From<DownloadError> for Failure {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::Cancelled => Failure::Cancelled,
            e => fail(ErrorCode::Network, e.to_string()),
        }
    }
}

impl From<EngineError> for Failure {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Cancelled => Failure::Cancelled,
            EngineError::Download(d) => d.into(),
            e @ EngineError::Symlink(..) => fail(ErrorCode::Unsupported, e.to_string()),
            e => fail(ErrorCode::Internal, e.to_string()),
        }
    }
}

impl From<ChooseError> for Failure {
    fn from(e: ChooseError) -> Self {
        let code = match e {
            ChooseError::NoBranch(_) => ErrorCode::NotFound,
            ChooseError::PasswordBranch(_)
            | ChooseError::EncryptedManifest(_)
            | ChooseError::MissingApp { .. }
            | ChooseError::SharedTwice(_) => ErrorCode::Unsupported,
            ChooseError::BadManifest { .. } => ErrorCode::Internal,
        };
        fail(code, e.to_string())
    }
}

/// The content server list, fetched once for all jobs and kept for an
/// hour (`content.md` §1).
#[derive(Default)]
pub struct ServerList {
    list: std::sync::Mutex<Option<(Vec<CContentServerDirectoryServerInfo>, Instant)>>,
}

impl ServerList {
    fn get(
        &self,
        conn: &Requester,
        cell_id: u32,
        cancel: &CancelToken,
    ) -> Result<Vec<CContentServerDirectoryServerInfo>, Failure> {
        let mut l = self.list.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((list, at)) = l.as_ref()
            && at.elapsed() < SERVER_LIST_TTL
        {
            return Ok(list.clone());
        }
        let list = request::servers(conn, cell_id, cancel)?;
        *l = Some((list.clone(), Instant::now()));
        Ok(list)
    }

    /// Drops the list, after every server failed or at a new session.
    pub fn forget(&self) {
        *self.list.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// CDN auth tokens over CM for one app (`content.md` §7).
struct CmTokens {
    conn: Requester,
    app: AppId,
}

impl Tokens for CmTokens {
    fn token(
        &self,
        depot: DepotId,
        host: &str,
        cancel: &CancelToken,
    ) -> Result<(String, u64), String> {
        request::cdn_auth_token(&self.conn, self.app, depot, host, cancel)
            .map(|(t, until)| (t, u64::from(until)))
            .map_err(|e| e.to_string())
    }
}

fn phase(p: Phase) -> v1::JobPhase {
    match p {
        Phase::Checking => v1::JobPhase::Checking,
        Phase::Copying => v1::JobPhase::Copying,
        Phase::Downloading => v1::JobPhase::Downloading,
        Phase::Finishing => v1::JobPhase::Finishing,
    }
}

fn progress(p: &engine::Progress) -> v1::JobProgressInfo {
    v1::JobProgressInfo {
        phase: phase(p.phase) as i32,
        bytes_total: p.bytes_total,
        bytes_done: p.bytes_done,
        download_total: p.download_total,
        downloaded: p.downloaded,
        files_total: p.files_total,
        files_done: p.files_done,
    }
}

/// `config/installdir` of an app's info, checked to be one plain path
/// component (`content.md` §8).
pub fn install_dir(info: &Object) -> Result<String, String> {
    let dir = info
        .get_path(&["config", "installdir"])
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if dir.is_empty() {
        return Err("the app's info names no install directory".into());
    }
    path::check_component(dir, cfg!(windows))
        .map_err(|why| format!("the install directory {dir:?} is not usable: {why}"))?;
    Ok(dir.to_owned())
}

/// The build a branch is at; 0 when the info gives none.
fn build_id(info: &Object, branch: &str) -> u32 {
    info.get_path(&["depots", "branches", branch, "buildid"])
        .and_then(Value::as_i64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// What a finished job leaves behind: the app as installed now, or `None`
/// after an uninstall.
pub type Done = Option<InstalledApp>;

struct Runner<'a> {
    spec: &'a Spec,
    env: &'a Env,
    control: &'a Control,
}

impl Runner<'_> {
    fn report(&self, r: Report) {
        let _ = self.env.core.send(Command::Content(r));
    }

    fn preparing(&self) {
        self.report(Report::Progress {
            job_id: self.spec.job_id,
            progress: v1::JobProgressInfo {
                phase: v1::JobPhase::Preparing as i32,
                ..Default::default()
            },
        });
    }

    fn cancel(&self) -> &CancelToken {
        &self.control.cancel
    }

    /// The install directory of a depot: this app's, or the other app's
    /// for a shared install.
    fn dir_for(&self, shared_from: Option<AppId>) -> Result<PathBuf, Failure> {
        match shared_from {
            None => {
                let dir =
                    install_dir(&self.spec.info).map_err(|e| fail(ErrorCode::Unsupported, e))?;
                Ok(self.spec.folder.app_dir(&dir))
            }
            Some(other) => {
                let info = self.spec.others.get(&other).ok_or_else(|| {
                    fail(
                        ErrorCode::Unsupported,
                        format!(
                            "a depot is installed with app {other}, whose info is not available"
                        ),
                    )
                })?;
                let dir = install_dir(info)
                    .map_err(|e| fail(ErrorCode::Unsupported, format!("app {other}: {e}")))?;
                Ok(self.spec.folder.app_dir(&dir))
            }
        }
    }

    /// The app's install scripts, read from its files
    /// (`install-scripts.md` §1).
    fn scripts(
        &self,
        sys: &dyn System,
        installed: &InstalledApp,
    ) -> Vec<ossteam_content::script::Script> {
        let dir_of = |depot: DepotId| {
            let d = installed.depots.iter().find(|d| d.depot == depot)?;
            match d.shared_from {
                None => Some(self.spec.folder.app_dir(&installed.install_dir)),
                Some(_) => self.dir_for(d.shared_from).ok(),
            }
        };
        script::load(&installed.scripts, &dir_of, &self.spec.language, sys)
    }

    /// Asks for each depot's key and keeps the depots it is granted for
    /// (`content.md` §2).
    fn keys(&self, depots: &[DepotId]) -> Result<HashMap<DepotId, DepotKey>, Failure> {
        let mut keys = HashMap::new();
        let mut refused = Vec::new();
        for &d in depots {
            match request::depot_key(&self.env.requester, self.spec.app, d, self.cancel())? {
                Ok(k) => {
                    keys.insert(d, k);
                }
                Err(eresult) => refused.push(format!("{d} ({eresult})")),
            }
        }
        if !refused.is_empty() {
            tracing::info!(app = %self.spec.app, refused = ?refused, "depot keys refused");
        }
        if keys.is_empty() && !depots.is_empty() {
            return Err(fail(
                ErrorCode::NotOwned,
                format!(
                    "Steam granted no depot key for this app; refused: {}",
                    refused.join(", ")
                ),
            ));
        }
        Ok(keys)
    }

    fn downloader(&self) -> Result<Downloader, Failure> {
        let list = self
            .env
            .servers
            .get(&self.env.requester, self.spec.cell_id, self.cancel())?;
        let usable = cdn::select(&list, self.spec.app);
        let rotation = Rotation::new(usable).ok_or_else(|| {
            fail(
                ErrorCode::Network,
                "no content server offers HTTPS for this app",
            )
        })?;
        Ok(Downloader::new(
            self.env.fetch.clone(),
            rotation,
            Some(Arc::new(CmTokens {
                conn: self.env.requester.clone(),
                app: self.spec.app,
            })),
        ))
    }

    /// Fetches, decrypts and checks a manifest (`content.md` §3, §4).
    fn fetch_manifest(
        &self,
        dl: &Downloader,
        depot: DepotId,
        id: ManifestId,
        key: &DepotKey,
    ) -> Result<Manifest, Failure> {
        let branch = Some(self.spec.branch.as_str());
        let code = request::manifest_request_code(
            &self.env.requester,
            self.spec.app,
            depot,
            id,
            branch,
            self.cancel(),
        )?;
        if code == 0 {
            return Err(fail(
                ErrorCode::NotOwned,
                format!("Steam gave no request code for manifest {id} of depot {depot}"),
            ));
        }
        let mut m = dl.manifest(depot, id, code, self.cancel())?;
        m.decrypt_names(key).and_then(|()| m.check()).map_err(|e| {
            fail(
                ErrorCode::Internal,
                format!("manifest {id} of depot {depot}: {e}"),
            )
        })?;
        Ok(m)
    }

    /// A stored manifest (`content.md` §4 "Stored form"); `None` if it is
    /// missing or unreadable, which costs a full download of that depot.
    fn stored(&self, depot: DepotId, id: ManifestId) -> Option<Manifest> {
        let path = self.spec.folder.manifest(depot, id);
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| Manifest::parse(&b).map_err(|e| e.to_string()))
            .and_then(|mut m| m.check().map(|()| m).map_err(|e| e.to_string()));
        match read {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "stored manifest not usable");
                None
            }
        }
    }

    fn store(&self, m: &Manifest) -> Result<(), Failure> {
        let path = self.spec.folder.manifest(m.depot, m.id);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(self.spec.folder.manifests())?;
            let tmp = path.with_extension("manifest.tmp");
            std::fs::write(&tmp, m.encode())?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| fail(ErrorCode::Internal, format!("{}: {e}", path.display())))
    }

    /// Whether any app other than this one has `depot` at `manifest`.
    fn used_elsewhere(&self, depot: DepotId, manifest: Option<ManifestId>) -> bool {
        self.spec
            .other_installs
            .iter()
            .filter(|a| a.app_id != self.spec.app)
            .flat_map(|a| &a.depots)
            .any(|d| d.depot == depot && manifest.is_none_or(|m| d.manifest == m))
    }

    fn engine(&self, job: &engine::Job, dl: &Downloader) -> Result<engine::Outcome, Failure> {
        let job_id = self.spec.job_id;
        let progress_hook = |p: &engine::Progress| {
            self.report(Report::Progress {
                job_id,
                progress: progress(p),
            })
        };
        let journal_hook = |bytes: Vec<u8>| self.report(Report::Journal { job_id, bytes });
        let hooks = Hooks {
            progress: &progress_hook,
            journal: &journal_hook,
        };
        let prepared = engine::prepare(job, self.spec.journal.as_deref(), &hooks, self.cancel())?;
        let need = prepared.bytes_to_write;
        if need > 0 {
            let root = &self.spec.folder.root;
            let free = super::folders::free_space(root)
                .map_err(|e| fail(ErrorCode::Internal, format!("{}: {e}", root.display())))?;
            let want = need.saturating_add(need / 100);
            if free <= want {
                return Err(fail(
                    ErrorCode::Internal,
                    format!(
                        "not enough free space in {}: the job writes {want} bytes and {free} are free",
                        root.display()
                    ),
                ));
            }
        }
        Ok(prepared.run(dl, self.spec.threads, &hooks, self.cancel())?)
    }

    /// Install or update (`content.md` §6, §10).
    fn install(&self) -> Result<Done, Failure> {
        let spec = self.spec;
        self.preparing();
        let owns = |a: AppId| spec.owned.contains(&a);
        let optional_on = |a: AppId| spec.optional_dlc.contains(&a);
        let others = |a: AppId| spec.others.get(&a);
        let want = Wanted {
            branch: &spec.branch,
            language: &spec.language,
            low_violence: spec.low_violence,
            host: spec.host,
            owns: &owns,
            optional_on: &optional_on,
        };
        let chosen = depots::choose(&spec.info, &others, &want)?;
        if chosen.is_empty() {
            return Err(fail(
                ErrorCode::Unsupported,
                format!(
                    "the app has no depots for {} on branch {:?}",
                    spec.host.os, spec.branch
                ),
            ));
        }
        let ids: Vec<DepotId> = chosen.iter().map(|c| c.depot).collect();
        let keys = self.keys(&ids)?;
        let dl = self.downloader()?;
        let installed: HashMap<DepotId, &InstalledDepot> = spec
            .installed
            .iter()
            .flat_map(|a| &a.depots)
            .map(|d| (d.depot, d))
            .collect();

        let mut work = Vec::new();
        let mut recorded = Vec::new();
        for c in chosen.iter().filter(|c| keys.contains_key(&c.depot)) {
            let key = keys[&c.depot].clone();
            let shared_from = c.from_app.filter(|_| c.shared_install);
            let install_dir = self.dir_for(shared_from)?;
            let old = installed
                .get(&c.depot)
                .and_then(|d| self.stored(d.depot, d.manifest));
            let manifest = match &old {
                Some(m) if m.id == c.manifest => m.clone(),
                _ => self.fetch_manifest(&dl, c.depot, c.manifest, &key)?,
            };
            recorded.push(InstalledDepot {
                depot: c.depot,
                manifest: c.manifest,
                shared_from,
            });
            work.push(DepotWork {
                key,
                install_dir,
                manifest,
                installed: old,
            });
        }
        let mut removed = Vec::new();
        for d in installed.values() {
            if work.iter().any(|w| w.manifest.depot == d.depot)
                || self.used_elsewhere(d.depot, None)
            {
                continue;
            }
            if let Some(m) = self.stored(d.depot, d.manifest) {
                removed.push(Removal {
                    install_dir: self.dir_for(d.shared_from)?,
                    manifest: m,
                });
            }
        }
        let job = engine::Job {
            mode: Mode::Install,
            depots: work,
            removed,
            staging: spec.folder.staging(spec.app),
        };
        let out = self.engine(&job, &dl)?;
        tracing::info!(app = %spec.app, ?out, "install finished");

        for w in &job.depots {
            self.store(&w.manifest)?;
        }
        // Stored manifests no app uses any more.
        let old_ids = installed.values().map(|d| (d.depot, d.manifest));
        for (depot, id) in old_ids {
            if !recorded
                .iter()
                .any(|r| r.depot == depot && r.manifest == id)
                && !self.used_elsewhere(depot, Some(id))
            {
                let _ = std::fs::remove_file(spec.folder.manifest(depot, id));
            }
        }
        Ok(Some(InstalledApp {
            app_id: spec.app,
            folder_id: spec.folder_id.clone(),
            install_dir: install_dir(&spec.info).map_err(|e| fail(ErrorCode::Unsupported, e))?,
            branch: spec.branch.clone(),
            build_id: build_id(&spec.info, &spec.branch),
            size: job.depots.iter().map(|w| w.manifest.original_size).sum(),
            update_available: false,
            platform: spec.host.os.to_owned(),
            depots: recorded,
            // Nothing runs yet (`install-scripts.md` §3).
            scripts: job
                .depots
                .iter()
                .flat_map(|w| script::files(&w.manifest))
                .collect(),
            script_values: spec
                .installed
                .as_ref()
                .map(|a| a.script_values.clone())
                .unwrap_or_default(),
        }))
    }

    /// `content.md` §11.
    fn verify(&self) -> Result<Done, Failure> {
        let spec = self.spec;
        let installed = spec
            .installed
            .clone()
            .ok_or_else(|| fail(ErrorCode::NotFound, "the app is not installed"))?;
        self.preparing();
        let ids: Vec<DepotId> = installed.depots.iter().map(|d| d.depot).collect();
        let keys = self.keys(&ids)?;
        let dl = self.downloader()?;
        let mut work = Vec::new();
        for d in &installed.depots {
            let Some(key) = keys.get(&d.depot) else {
                continue;
            };
            let manifest = match self.stored(d.depot, d.manifest) {
                Some(m) => m,
                None => {
                    let m = self.fetch_manifest(&dl, d.depot, d.manifest, key)?;
                    self.store(&m)?;
                    m
                }
            };
            work.push(DepotWork {
                key: key.clone(),
                install_dir: self.dir_for(d.shared_from)?,
                installed: Some(manifest.clone()),
                manifest,
            });
        }
        let job = engine::Job {
            mode: Mode::Verify,
            depots: work,
            removed: Vec::new(),
            staging: spec.folder.staging(spec.app),
        };
        let out = self.engine(&job, &dl)?;
        tracing::info!(app = %spec.app, repaired = out.files_written, "verify finished");
        let mut installed = installed;
        if let Some(sys) = &self.env.scripts {
            let loaded = self.scripts(sys.as_ref(), &installed);
            let plan = script::removal(&loaded, &installed.script_values, sys.as_ref());
            let out = script::execute(sys.as_ref(), &plan, self.cancel())
                .map_err(|script::Stopped| Failure::Cancelled)?;
            installed.script_values = script::remaining(&installed.script_values, &plan, &out);
        }
        Ok(Some(installed))
    }

    /// `content.md` §12.
    fn uninstall(&self) -> Result<Done, Failure> {
        let spec = self.spec;
        if spec.remove_compat_data {
            remove_dir(&spec.folder.compat_data(spec.app))?;
        }
        let Some(installed) = &spec.installed else {
            // Only an unfinished install was there; its staging goes.
            remove_dir(&spec.folder.staging(spec.app))?;
            return Ok(None);
        };
        self.preparing();
        if let Some(sys) = &self.env.scripts {
            let loaded = self.scripts(sys.as_ref(), installed);
            let plan = script::uninstall(&loaded, &installed.script_values, sys.as_ref());
            script::execute(sys.as_ref(), &plan, self.cancel())
                .map_err(|script::Stopped| Failure::Cancelled)?;
        }
        // Shared depots: their files go only if no other app has them.
        for d in installed.depots.iter().filter(|d| d.shared_from.is_some()) {
            if self.used_elsewhere(d.depot, None) {
                continue;
            }
            if let Some(m) = self.stored(d.depot, d.manifest) {
                let job = engine::Job {
                    mode: Mode::Install,
                    depots: Vec::new(),
                    removed: vec![Removal {
                        install_dir: self.dir_for(d.shared_from)?,
                        manifest: m,
                    }],
                    staging: spec.folder.staging(spec.app),
                };
                let cancel = self.cancel();
                engine::prepare(&job, None, &Hooks::none(), cancel)?.run(
                    &Downloader::offline(),
                    1,
                    &Hooks::none(),
                    cancel,
                )?;
            }
        }
        let dir = spec.folder.app_dir(&installed.install_dir);
        path::check_component(&installed.install_dir, cfg!(windows)).map_err(|why| {
            fail(
                ErrorCode::Internal,
                format!("the recorded install directory is not usable: {why}"),
            )
        })?;
        remove_dir(&dir)?;
        remove_dir(&spec.folder.staging(spec.app))?;
        for d in &installed.depots {
            if !self.used_elsewhere(d.depot, Some(d.manifest)) {
                let _ = std::fs::remove_file(spec.folder.manifest(d.depot, d.manifest));
            }
        }
        Ok(None)
    }
}

/// Deletes a directory tree; one that is not there is fine. On Windows,
/// read-only files are made writable first, as deletion needs.
fn remove_dir(dir: &std::path::Path) -> Result<(), Failure> {
    let result = std::fs::remove_dir_all(dir).or_else(|e| {
        if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied {
            clear_read_only_tree(dir);
            std::fs::remove_dir_all(dir)
        } else {
            Err(e)
        }
    });
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fail(ErrorCode::Internal, format!("{}: {e}", dir.display()))),
    }
}

fn clear_read_only_tree(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            clear_read_only_tree(&e.path());
        } else if t.is_file()
            && let Ok(m) = e.metadata()
            && m.permissions().readonly()
        {
            let mut p = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(e.path(), p);
        }
    }
}

/// Runs a job to its end and reports the result.
pub fn run(spec: Spec, env: Arc<Env>, control: Arc<Control>) {
    let r = Runner {
        spec: &spec,
        env: &env,
        control: &control,
    };
    let result = match spec.kind {
        Kind::Install | Kind::Update => r.install(),
        Kind::Verify => r.verify(),
        Kind::Uninstall => r.uninstall(),
    };
    if result == Err(Failure::Cancelled) && control.discard.load(Ordering::Relaxed) {
        let _ = remove_dir(&spec.folder.staging(spec.app));
    }
    if let Err(Failure::Error { code, message }) = &result {
        tracing::warn!(job = spec.job_id, app = %spec.app, ?code, "job failed: {message}");
        if *code == ErrorCode::Network {
            env.servers.forget();
        }
    }
    r.report(Report::Finished {
        job_id: spec.job_id,
        result,
    });
}
