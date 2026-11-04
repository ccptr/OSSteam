// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of library folders and content jobs (`ipc.md` §5.5,
//! §5.6; `content.md` §9, §10): the methods, scheduling, the reports of
//! running jobs, and update detection.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ossteam_content::depots::{self, Host, Wanted};
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode, Topic, event, response};
use ossteam_pics::cache::Kind as CacheKind;
use ossteam_pics::{AppInfo, LibraryApp};
use ossteam_types::{AppId, SteamId};
use ossteam_vdf::{Object, Value};

use super::shim::dlc_installed;
use super::{Core, internal, invalid};
use crate::content::folders::{self, Layout};
use crate::content::job::{self, Failure, Spec};
use crate::content::{Kind, Report};
use crate::db::{Folder, InstalledApp, JobRow};

/// What to do with a running job once it reports that it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterStop {
    Pause,
    Cancel,
}

pub struct JobEntry {
    pub row: JobRow,
    pub progress: v1::JobProgressInfo,
    pub after_stop: Option<AfterStop>,
}

/// The content state the core keeps.
#[derive(Default)]
pub struct Content {
    pub folders: Vec<Folder>,
    pub installed: BTreeMap<AppId, InstalledApp>,
    pub jobs: BTreeMap<u64, JobEntry>,
    /// The CM connection of the logged-on account, for jobs.
    pub requester: Option<ossteam_net::Requester>,
    /// `<data>/library`, created when first needed.
    pub default_folder: PathBuf,
}

fn not_found(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::NotFound, m)
}

fn conflict(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::Conflict, m)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unfinished(s: v1::JobState) -> bool {
    matches!(
        s,
        v1::JobState::Queued | v1::JobState::Running | v1::JobState::Paused | v1::JobState::Failed
    )
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Core {
    /// Loads folders, installs and jobs. Jobs that were running when the
    /// daemon stopped are queued again (`content.md` §9).
    pub(super) fn load_content(&mut self) -> anyhow::Result<()> {
        self.db.forget_finished_jobs()?;
        self.content.folders = self.db.folders()?;
        self.content.installed = self
            .db
            .installed_apps()?
            .into_iter()
            .map(|a| (a.app_id, a))
            .collect();
        for mut row in self.db.jobs()? {
            if row.state == v1::JobState::Running {
                row.state = v1::JobState::Queued;
                self.db.update_job(&row)?;
            }
            self.content.jobs.insert(
                row.job_id,
                JobEntry {
                    row,
                    progress: v1::JobProgressInfo::default(),
                    after_stop: None,
                },
            );
        }
        Ok(())
    }

    pub(super) fn is_installed(&self, app: AppId) -> bool {
        self.content.installed.contains_key(&app)
    }

    /// For `library::InstallState`. A DLC is installed with its parent,
    /// when one of its depots is (`shim::dlc_installed`).
    pub(super) fn install_state(&self, app: &LibraryApp) -> Option<bool> {
        if let Some(a) = self.content.installed.get(&app.id) {
            return Some(a.update_available);
        }
        let parent = app.parent?;
        let installed = self.content.installed.get(&parent)?;
        let info = self.app_info(parent)?;
        let depots: Vec<u32> = installed.depots.iter().map(|d| d.depot.0).collect();
        dlc_installed(&AppInfo(&info).depots(), &depots, app.id).then_some(false)
    }

    pub(super) fn installed_proto(&self, app: AppId) -> Option<v1::InstalledApp> {
        let a = self.content.installed.get(&app)?;
        let folder = self.content.folders.iter().find(|f| f.id == a.folder_id)?;
        Some(v1::InstalledApp {
            folder: path_str(&folder.path),
            path: path_str(&Layout::new(&folder.path).app_dir(&a.install_dir)),
            branch: a.branch.clone(),
            build_id: a.build_id,
            depots: a
                .depots
                .iter()
                .map(|d| v1::InstalledDepot {
                    depot_id: d.depot.0,
                    manifest_id: d.manifest.0,
                    shared_from: d.shared_from.map_or(0, |a| a.0),
                })
                .collect(),
            size: a.size,
            update_available: a.update_available,
        })
    }

    pub(super) fn running_jobs(&self) -> u32 {
        self.content
            .jobs
            .values()
            .filter(|j| j.row.state == v1::JobState::Running)
            .count() as u32
    }

    // --- Folders -----------------------------------------------------------

    /// The default folder, created and added if it is not there yet.
    fn default_folder(&mut self) -> Result<Folder, RemoteError> {
        if let Some(f) = self.content.folders.iter().find(|f| f.is_default) {
            return Ok(f.clone());
        }
        let path = self.content.default_folder.clone();
        let id = folders::ensure(&path).map_err(|e| {
            RemoteError::new(ErrorCode::Internal, format!("{}: {e}", path.display()))
        })?;
        let f = Folder {
            id,
            path,
            is_default: true,
        };
        self.db.add_folder(&f).map_err(|e| internal(&e))?;
        self.content.folders.insert(0, f.clone());
        Ok(f)
    }

    fn folder_proto(&self, f: &Folder) -> v1::LibraryFolder {
        let available = folders::read_id(&f.path).ok().flatten().as_deref() == Some(f.id.as_str());
        v1::LibraryFolder {
            path: path_str(&f.path),
            is_default: f.is_default,
            available,
            free_bytes: if available {
                folders::free_space(&f.path).unwrap_or(0)
            } else {
                0
            },
            app_ids: self
                .content
                .installed
                .values()
                .filter(|a| a.folder_id == f.id)
                .map(|a| a.app_id.0)
                .collect(),
        }
    }

    fn find_folder(&self, path: &str) -> Option<&Folder> {
        let p = Path::new(path.trim());
        self.content.folders.iter().find(|f| f.path == p)
    }

    pub(super) fn list_folders(&mut self) -> Result<response::Result, RemoteError> {
        self.default_folder()?;
        Ok(response::Result::ListLibraryFolders(
            v1::ListLibraryFoldersResponse {
                folders: self
                    .content
                    .folders
                    .iter()
                    .map(|f| self.folder_proto(f))
                    .collect(),
            },
        ))
    }

    pub(super) fn add_folder(
        &mut self,
        req: &v1::AddLibraryFolderRequest,
    ) -> Result<response::Result, RemoteError> {
        let path = PathBuf::from(req.path.trim());
        if !path.is_absolute() {
            return Err(invalid(format!("{:?} is not an absolute path", req.path)));
        }
        if path.to_str().is_none() {
            return Err(invalid("library folder paths must be valid Unicode"));
        }
        self.default_folder()?;
        if self.content.folders.iter().any(|f| f.path == path) {
            return Err(conflict(format!(
                "{} is already a library folder",
                path.display()
            )));
        }
        if let Some(f) = self
            .content
            .folders
            .iter()
            .find(|f| folders::overlap(&f.path, &path))
        {
            return Err(invalid(format!(
                "{} overlaps the library folder {}",
                path.display(),
                f.path.display()
            )));
        }
        let id = folders::ensure(&path).map_err(|e| {
            RemoteError::new(ErrorCode::Internal, format!("{}: {e}", path.display()))
        })?;
        if self.content.folders.iter().any(|f| f.id == id) {
            return Err(conflict(format!(
                "{} has the id of a library folder already in the list",
                path.display()
            )));
        }
        let f = Folder {
            id,
            path,
            is_default: false,
        };
        self.db.add_folder(&f).map_err(|e| internal(&e))?;
        self.content.folders.push(f.clone());
        Ok(response::Result::AddLibraryFolder(
            v1::AddLibraryFolderResponse {
                folder: Some(self.folder_proto(&f)),
            },
        ))
    }

    pub(super) fn remove_folder(
        &mut self,
        req: &v1::RemoveLibraryFolderRequest,
    ) -> Result<response::Result, RemoteError> {
        let f = self
            .find_folder(&req.path)
            .cloned()
            .ok_or_else(|| not_found(format!("{} is not a library folder", req.path)))?;
        if f.is_default {
            return Err(conflict("the default library folder cannot be removed"));
        }
        if self.content.installed.values().any(|a| a.folder_id == f.id) {
            return Err(conflict(format!(
                "apps are installed in {}",
                f.path.display()
            )));
        }
        if self
            .content
            .jobs
            .values()
            .any(|j| j.row.folder_id == f.id && unfinished(j.row.state))
        {
            return Err(conflict(format!("a job uses {}", f.path.display())));
        }
        self.db.remove_folder(&f.id).map_err(|e| internal(&e))?;
        self.content.folders.retain(|x| x.id != f.id);
        Ok(response::Result::RemoveLibraryFolder(
            v1::RemoveLibraryFolderResponse {},
        ))
    }

    // --- Jobs --------------------------------------------------------------

    fn active_account(&self) -> Result<SteamId, RemoteError> {
        self.session_state
            .account
            .as_ref()
            .map(|a| SteamId(a.steam_id))
            .ok_or_else(|| RemoteError::new(ErrorCode::NotLoggedIn, "no account is logged in"))
    }

    pub(super) fn job_proto(&self, e: &JobEntry) -> v1::Job {
        let r = &e.row;
        v1::Job {
            job_id: r.job_id,
            app_id: r.app_id.0,
            kind: r.kind as i32,
            state: r.state as i32,
            priority: r.priority,
            progress: Some(e.progress),
            error: r.error.as_ref().map(|(code, message)| v1::Error {
                code: *code as i32,
                message: message.clone(),
                detail: String::new(),
            }),
            folder: self
                .content
                .folders
                .iter()
                .find(|f| f.id == r.folder_id)
                .map(|f| path_str(&f.path))
                .unwrap_or_default(),
            branch: r.branch.clone(),
            created: r.created,
        }
    }

    fn job_changed(&mut self, job_id: u64) {
        let Some(e) = self.content.jobs.get(&job_id) else {
            return;
        };
        if let Err(err) = self.db.update_job(&e.row) {
            tracing::error!("storing job {job_id}: {err:#}");
        }
        let job = self.job_proto(e);
        self.broadcast(
            Topic::Jobs,
            event::Kind::JobStateChanged(v1::JobStateChanged { job: Some(job) }),
        );
        self.resume_launches();
    }

    /// Whether job `job_id` can still finish; a job no longer listed
    /// cannot.
    pub(super) fn job_unfinished(&self, job_id: u64) -> bool {
        self.content
            .jobs
            .get(&job_id)
            .is_some_and(|e| unfinished(e.row.state) && e.row.state != v1::JobState::Failed)
    }

    pub(super) fn unfinished_job(&self, app: AppId) -> Option<u64> {
        self.content
            .jobs
            .values()
            .find(|j| j.row.app_id == app && unfinished(j.row.state))
            .map(|j| j.row.job_id)
    }

    pub(super) fn create_job(
        &mut self,
        steam_id: SteamId,
        app: AppId,
        kind: Kind,
        folder_id: String,
        branch: String,
    ) -> Result<v1::Job, RemoteError> {
        self.queue_job(JobRow {
            job_id: 0,
            steam_id,
            app_id: app,
            kind: kind.proto(),
            state: v1::JobState::Queued,
            priority: 0,
            folder_id,
            branch,
            created: now(),
            error: None,
            remove_compat_data: false,
        })
    }

    /// Stores and schedules a new job.
    fn queue_job(&mut self, mut row: JobRow) -> Result<v1::Job, RemoteError> {
        let (app, kind) = (row.app_id, row.kind);
        row.job_id = self.db.add_job(&row).map_err(|e| internal(&e))?;
        let id = row.job_id;
        tracing::info!(job = id, app = %app, ?kind, "job queued");
        self.content.jobs.insert(
            id,
            JobEntry {
                row,
                progress: v1::JobProgressInfo::default(),
                after_stop: None,
            },
        );
        self.job_changed(id);
        let job = self.job_proto(&self.content.jobs[&id]);
        self.schedule();
        Ok(job)
    }

    pub(super) fn app_info(&self, app: AppId) -> Option<Object> {
        self.pics_cache.load(CacheKind::App, app.0).map(|i| i.data)
    }

    pub(super) fn install(
        &mut self,
        req: &v1::InstallRequest,
    ) -> Result<response::Result, RemoteError> {
        let steam_id = self.active_account()?;
        let snapshot = self.snapshot()?;
        let app = AppId(req.app_id);
        if snapshot.library.find(app).is_none() {
            return Err(not_found(format!("app {app} is not in the library")));
        }
        let info = self
            .app_info(app)
            .ok_or_else(|| not_found(format!("the info of app {app} is not available")))?;
        job::install_dir(&info).map_err(|e| RemoteError::new(ErrorCode::Unsupported, e))?;
        let branch = match req.branch.trim() {
            "" => "public".to_owned(),
            b => b.to_owned(),
        };
        if let Some(d) = info.get_object("depots") {
            depots::check_branch(d, &branch).map_err(|e| {
                let f: Failure = e.into();
                match f {
                    Failure::Error { code, message } => RemoteError::new(code, message),
                    Failure::Cancelled => unreachable!("branch checks do not cancel"),
                }
            })?;
        }
        let folder = if req.folder.trim().is_empty() {
            self.default_folder()?
        } else {
            self.find_folder(&req.folder)
                .cloned()
                .ok_or_else(|| not_found(format!("{} is not a library folder", req.folder)))?
        };
        if self.is_installed(app) {
            return Err(conflict(format!(
                "app {app} is installed; update or verify it instead"
            )));
        }
        if let Some(j) = self.unfinished_job(app) {
            return Err(conflict(format!("app {app} has job {j} already")));
        }
        let job = self.create_job(steam_id, app, Kind::Install, folder.id, branch)?;
        Ok(response::Result::Install(v1::JobResponse {
            job: Some(job),
        }))
    }

    pub(super) fn update(
        &mut self,
        req: &v1::UpdateRequest,
    ) -> Result<response::Result, RemoteError> {
        let steam_id = self.active_account()?;
        let apps: Vec<AppId> = if req.all {
            self.content
                .installed
                .values()
                .filter(|a| a.update_available)
                .map(|a| a.app_id)
                .collect()
        } else {
            let apps: Vec<AppId> = req.app_ids.iter().map(|&a| AppId(a)).collect();
            if let Some(a) = apps.iter().find(|a| !self.is_installed(**a)) {
                return Err(not_found(format!("app {a} is not installed")));
            }
            apps
        };
        let mut jobs = Vec::new();
        for app in apps {
            if self.unfinished_job(app).is_some() {
                continue;
            }
            let a = self.content.installed[&app].clone();
            jobs.push(self.create_job(steam_id, app, Kind::Update, a.folder_id, a.branch)?);
        }
        Ok(response::Result::Update(v1::UpdateResponse { jobs }))
    }

    pub(super) fn verify(
        &mut self,
        req: &v1::VerifyRequest,
    ) -> Result<response::Result, RemoteError> {
        let steam_id = self.active_account()?;
        let app = AppId(req.app_id);
        let a = self
            .content
            .installed
            .get(&app)
            .cloned()
            .ok_or_else(|| not_found(format!("app {app} is not installed")))?;
        if let Some(j) = self.unfinished_job(app) {
            return Err(conflict(format!("app {app} has job {j} already")));
        }
        let job = self.create_job(steam_id, app, Kind::Verify, a.folder_id, a.branch)?;
        Ok(response::Result::Verify(v1::JobResponse { job: Some(job) }))
    }

    pub(super) fn uninstall(
        &mut self,
        req: &v1::UninstallRequest,
    ) -> Result<response::Result, RemoteError> {
        let steam_id = self.active_account()?;
        let app = AppId(req.app_id);
        let mut folder_id = self
            .content
            .installed
            .get(&app)
            .map(|a| a.folder_id.clone());
        let mut branch = self.content.installed.get(&app).map(|a| a.branch.clone());
        if let Some(j) = self.unfinished_job(app) {
            let row = &self.content.jobs[&j].row;
            if row.kind == v1::JobKind::Uninstall {
                return Err(conflict(format!(
                    "app {app} is being uninstalled by job {j}"
                )));
            }
            folder_id.get_or_insert_with(|| row.folder_id.clone());
            branch.get_or_insert_with(|| row.branch.clone());
            self.cancel_job_inner(j, false)?;
        }
        let (Some(folder_id), Some(branch)) = (folder_id, branch) else {
            return Err(not_found(format!("app {app} is not installed")));
        };
        let job = self.queue_job(JobRow {
            job_id: 0,
            steam_id,
            app_id: app,
            kind: v1::JobKind::Uninstall,
            state: v1::JobState::Queued,
            priority: 0,
            folder_id,
            branch,
            created: now(),
            error: None,
            remove_compat_data: req.remove_compat_data && super::compat::ENABLED,
        })?;
        Ok(response::Result::Uninstall(v1::JobResponse {
            job: Some(job),
        }))
    }

    pub(super) fn list_jobs(&self) -> Result<response::Result, RemoteError> {
        let steam_id = self.active_account()?;
        let mut jobs: Vec<&JobEntry> = self
            .content
            .jobs
            .values()
            .filter(|j| j.row.steam_id == steam_id)
            .collect();
        jobs.sort_by_key(|j| (!unfinished(j.row.state), j.row.job_id));
        Ok(response::Result::ListJobs(v1::ListJobsResponse {
            jobs: jobs.into_iter().map(|j| self.job_proto(j)).collect(),
        }))
    }

    /// The active account's job.
    fn own_job(&self, job_id: u64) -> Result<&JobEntry, RemoteError> {
        let steam_id = self.active_account()?;
        self.content
            .jobs
            .get(&job_id)
            .filter(|j| j.row.steam_id == steam_id)
            .ok_or_else(|| not_found(format!("there is no job {job_id}")))
    }

    fn job_response(&self, job_id: u64) -> v1::JobResponse {
        v1::JobResponse {
            job: Some(self.job_proto(&self.content.jobs[&job_id])),
        }
    }

    pub(super) fn pause_job(&mut self, job_id: u64) -> Result<response::Result, RemoteError> {
        let state = self.own_job(job_id)?.row.state;
        match state {
            v1::JobState::Queued => {}
            v1::JobState::Running => {
                self.content_jobs.stop(job_id, false);
                self.content
                    .jobs
                    .get_mut(&job_id)
                    .expect("checked")
                    .after_stop = Some(AfterStop::Pause);
            }
            s => return Err(conflict(format!("job {job_id} is {}", state_name(s)))),
        }
        self.content
            .jobs
            .get_mut(&job_id)
            .expect("checked")
            .row
            .state = v1::JobState::Paused;
        self.job_changed(job_id);
        Ok(response::Result::PauseJob(self.job_response(job_id)))
    }

    pub(super) fn resume_job(&mut self, job_id: u64) -> Result<response::Result, RemoteError> {
        let e = self.own_job(job_id)?;
        if !matches!(e.row.state, v1::JobState::Paused | v1::JobState::Failed) {
            return Err(conflict(format!(
                "job {job_id} is {}",
                state_name(e.row.state)
            )));
        }
        if self.content_jobs.is_running(job_id) {
            return Err(conflict(format!("job {job_id} is still stopping")));
        }
        let e = self.content.jobs.get_mut(&job_id).expect("checked");
        e.row.state = v1::JobState::Queued;
        e.row.error = None;
        e.after_stop = None;
        self.job_changed(job_id);
        self.schedule();
        Ok(response::Result::ResumeJob(self.job_response(job_id)))
    }

    pub(super) fn cancel_job(&mut self, job_id: u64) -> Result<response::Result, RemoteError> {
        self.own_job(job_id)?;
        self.cancel_job_inner(job_id, true)?;
        Ok(response::Result::CancelJob(self.job_response(job_id)))
    }

    /// Cancels an unfinished job and deletes its staging. `schedule` is
    /// false when the caller queues another job for the app right after.
    fn cancel_job_inner(&mut self, job_id: u64, schedule: bool) -> Result<(), RemoteError> {
        let e = &self.content.jobs[&job_id];
        if !unfinished(e.row.state) {
            return Err(conflict(format!(
                "job {job_id} is {}",
                state_name(e.row.state)
            )));
        }
        if self.content_jobs.is_running(job_id) {
            self.content_jobs.stop(job_id, true);
            self.content
                .jobs
                .get_mut(&job_id)
                .expect("checked")
                .after_stop = Some(AfterStop::Cancel);
        } else if let Some(f) = self
            .content
            .folders
            .iter()
            .find(|f| f.id == e.row.folder_id)
        {
            // Staging can be large; delete it off the core thread.
            let staging = Layout::new(&f.path).staging(e.row.app_id);
            let _ = std::thread::Builder::new()
                .name("discard".into())
                .spawn(move || std::fs::remove_dir_all(staging));
        }
        let e = self.content.jobs.get_mut(&job_id).expect("checked");
        e.row.state = v1::JobState::Cancelled;
        let app = e.row.app_id;
        let was_update = e.row.kind == v1::JobKind::Update;
        let _ = self.db.set_job_journal(job_id, None);
        if was_update && self.is_installed(app) {
            self.mark_update(app, true);
        }
        self.job_changed(job_id);
        if schedule {
            self.schedule();
        }
        Ok(())
    }

    pub(super) fn set_job_priority(
        &mut self,
        req: &v1::SetJobPriorityRequest,
    ) -> Result<response::Result, RemoteError> {
        let e = self.own_job(req.job_id)?;
        if !unfinished(e.row.state) {
            return Err(conflict(format!(
                "job {} is {}",
                req.job_id,
                state_name(e.row.state)
            )));
        }
        self.content
            .jobs
            .get_mut(&req.job_id)
            .expect("checked")
            .row
            .priority = req.priority;
        self.job_changed(req.job_id);
        Ok(response::Result::SetJobPriority(
            self.job_response(req.job_id),
        ))
    }

    // --- Scheduling and reports ---------------------------------------------

    /// The logged-on account's requester, for jobs.
    pub(super) fn set_requester(&mut self, r: Option<ossteam_net::Requester>) {
        if r.is_some() {
            self.content_jobs.new_session();
        }
        self.content.requester = r;
        self.schedule();
    }

    /// Starts queued jobs of the logged-on account while fewer than
    /// `content.concurrent_jobs` run.
    pub(super) fn schedule(&mut self) {
        let Some(requester) = self.content.requester.clone() else {
            return;
        };
        let Ok(steam_id) = self.active_account() else {
            return;
        };
        if !self.session_state.logged_on {
            return;
        }
        // Which depots a job takes depends on what the account owns
        // (`content.md` §6), so jobs wait for the whole library.
        if !self
            .library
            .as_ref()
            .is_some_and(|s| s.complete && s.steam_id == steam_id)
        {
            return;
        }
        while (self.running_jobs() as usize) < self.config.concurrent_jobs() {
            // A job whose thread is still stopping, such as one cancelled
            // for an uninstall, keeps the next job of its app waiting.
            let busy: HashSet<AppId> = self
                .content
                .jobs
                .values()
                .filter(|j| self.content_jobs.is_running(j.row.job_id))
                .map(|j| j.row.app_id)
                .collect();
            let next = self
                .content
                .jobs
                .values()
                .filter(|j| j.row.steam_id == steam_id && j.row.state == v1::JobState::Queued)
                .filter(|j| !busy.contains(&j.row.app_id))
                .max_by_key(|j| (j.row.priority, std::cmp::Reverse(j.row.job_id)))
                .map(|j| j.row.job_id);
            let Some(job_id) = next else {
                return;
            };
            let started = self.job_spec(job_id).and_then(|spec| {
                self.content_jobs
                    .start(spec, requester.clone())
                    .map_err(|e| {
                        RemoteError::new(ErrorCode::Internal, format!("starting the job: {e}"))
                    })
            });
            let e = self.content.jobs.get_mut(&job_id).expect("listed above");
            match started {
                Ok(()) => {
                    tracing::info!(job = job_id, app = %e.row.app_id, "job started");
                    e.row.state = v1::JobState::Running;
                    e.progress = v1::JobProgressInfo::default();
                }
                Err(err) => {
                    e.row.state = v1::JobState::Failed;
                    e.row.error = Some((err.code, err.message));
                }
            }
            self.job_changed(job_id);
        }
    }

    fn job_spec(&self, job_id: u64) -> Result<Spec, RemoteError> {
        let row = &self.content.jobs[&job_id].row;
        let kind = Kind::from_proto(row.kind).ok_or_else(|| invalid("the job has no kind"))?;
        let folder = self
            .content
            .folders
            .iter()
            .find(|f| f.id == row.folder_id)
            .ok_or_else(|| not_found("the job's library folder is not in the list any more"))?;
        let app = row.app_id;
        let info = match self.app_info(app) {
            Some(i) => i,
            None if kind == Kind::Uninstall => Object::new(),
            None => return Err(not_found(format!("the info of app {app} is not available"))),
        };
        let installed = self.content.installed.get(&app).cloned();
        let mut wanted_others: HashSet<AppId> = installed
            .iter()
            .flat_map(|a| &a.depots)
            .filter_map(|d| d.shared_from)
            .collect();
        if let Some(d) = info.get_object("depots") {
            for (_, v) in d.iter() {
                if let Some(a) = v
                    .as_object()
                    .and_then(|o| o.get("depotfromapp"))
                    .and_then(Value::as_i64)
                    .and_then(|n| u32::try_from(n).ok())
                {
                    wanted_others.insert(AppId(a));
                }
            }
        }
        let others: HashMap<AppId, Object> = wanted_others
            .into_iter()
            .filter_map(|a| Some((a, self.app_info(a)?)))
            .collect();
        let snapshot = self.library.as_ref().filter(|s| s.steam_id == row.steam_id);
        let owned: HashSet<AppId> = snapshot
            .map(|s| s.library.iter().map(|a| a.id).collect())
            .unwrap_or_default();
        let low_violence = snapshot.is_some_and(|s| s.low_violence.contains(&app));
        let journal = self.db.job_journal(job_id).map_err(|e| internal(&e))?;
        // An install or update takes the depots of the app's tool's
        // platform; verify repairs what is there (`compat-tools.md` §4).
        let host = match (kind, &installed) {
            (Kind::Verify | Kind::Uninstall, Some(a)) => installed_host(a),
            _ => self.install_host(app, &info),
        };
        Ok(Spec {
            job_id,
            kind,
            app,
            branch: row.branch.clone(),
            folder: Layout::new(&folder.path),
            folder_id: folder.id.clone(),
            info,
            others,
            owned,
            optional_dlc: self.config.optional_dlc(app).into_iter().collect(),
            low_violence,
            language: self.config.language().to_owned(),
            host,
            installed,
            other_installs: self
                .content
                .installed
                .values()
                .filter(|a| a.app_id != app)
                .cloned()
                .collect(),
            journal,
            remove_compat_data: row.remove_compat_data,
            threads: self.config.download_threads(),
            cell_id: self.db.cell_id().unwrap_or(0),
        })
    }

    pub(super) fn on_content(&mut self, report: Report) {
        match report {
            Report::Progress { job_id, progress } => {
                if let Some(e) = self.content.jobs.get_mut(&job_id) {
                    e.progress = progress;
                    self.broadcast(
                        Topic::Jobs,
                        event::Kind::JobProgress(v1::JobProgress {
                            job_id,
                            progress: Some(progress),
                        }),
                    );
                }
            }
            Report::Journal { job_id, bytes } => {
                if let Err(e) = self.db.set_job_journal(job_id, Some(&bytes)) {
                    tracing::error!("storing the journal of job {job_id}: {e:#}");
                }
            }
            Report::Finished { job_id, result } => {
                self.content_jobs.finished(job_id);
                self.finished(job_id, result);
                self.schedule();
                self.update_shims();
            }
        }
    }

    fn finished(&mut self, job_id: u64, result: Result<job::Done, Failure>) {
        let Some(e) = self.content.jobs.get_mut(&job_id) else {
            return;
        };
        let app = e.row.app_id;
        let after = e.after_stop.take();
        match result {
            Ok(done) => {
                e.row.state = v1::JobState::Done;
                e.row.error = None;
                let stored = match done {
                    Some(installed) => {
                        let r = self.db.set_installed(&installed);
                        self.content.installed.insert(app, installed);
                        r
                    }
                    None => {
                        self.content.installed.remove(&app);
                        self.db.remove_installed(app)
                    }
                };
                if let Err(err) = stored {
                    tracing::error!("recording app {app}: {err:#}");
                }
                let _ = self.db.set_job_journal(job_id, None);
                self.broadcast(
                    Topic::Library,
                    event::Kind::AppChanged(v1::AppChanged {
                        app_ids: vec![app.0],
                        complete: self.library.as_ref().is_some_and(|s| s.complete),
                    }),
                );
            }
            Err(Failure::Cancelled) => match after {
                // The state was set when the stop was asked for.
                Some(AfterStop::Pause | AfterStop::Cancel) => {}
                // The daemon is stopping, or the session ended.
                None => e.row.state = v1::JobState::Queued,
            },
            Err(Failure::Error { code, message }) => {
                if after == Some(AfterStop::Cancel) {
                    // Failed while stopping; it stays cancelled.
                } else if self.content.requester.is_none() || !self.session_state.logged_on {
                    // The account went offline under the job (`ipc.md` §5.6).
                    e.row.state = v1::JobState::Queued;
                } else {
                    e.row.state = v1::JobState::Failed;
                    e.row.error = Some((code, message));
                }
            }
        }
        self.job_changed(job_id);
    }

    fn mark_update(&mut self, app: AppId, available: bool) {
        if let Some(a) = self.content.installed.get_mut(&app)
            && a.update_available != available
        {
            a.update_available = available;
            if let Err(e) = self.db.set_update_available(app, available) {
                tracing::error!("recording the update state of app {app}: {e:#}");
            }
        }
    }

    /// The depots an update of `a` would install now (`content.md` §6);
    /// `None` when they cannot be worked out.
    fn current_depots(&self, a: &InstalledApp) -> Option<Vec<depots::Chosen>> {
        let info = self.app_info(a.app_id)?;
        let others: HashMap<AppId, Object> = a
            .depots
            .iter()
            .filter_map(|d| d.shared_from)
            .chain(
                info.get_object("depots")
                    .into_iter()
                    .flat_map(|d| d.iter())
                    .filter_map(|(_, v)| v.as_object()?.get("depotfromapp")?.as_i64())
                    .filter_map(|n| u32::try_from(n).ok().map(AppId)),
            )
            .filter_map(|x| Some((x, self.app_info(x)?)))
            .collect();
        let snapshot = self.library.as_ref()?;
        let owned: HashSet<AppId> = snapshot.library.iter().map(|x| x.id).collect();
        let owns = |x: AppId| owned.contains(&x);
        let turned_on = self.config.optional_dlc(a.app_id);
        let optional_on = |x: AppId| turned_on.contains(&x);
        let lookup = |x: AppId| others.get(&x);
        let want = Wanted {
            branch: &a.branch,
            language: self.config.language(),
            low_violence: snapshot.low_violence.contains(&a.app_id),
            host: self.install_host(a.app_id, &info),
            owns: &owns,
            optional_on: &optional_on,
        };
        depots::choose(&info, &lookup, &want).ok()
    }

    /// Whether `a`'s tool now runs programs of another platform than the
    /// one installed (`compat-tools.md` §4); the update records it, even
    /// when the depots are the same.
    fn platform_changed(&self, a: &InstalledApp) -> bool {
        let Some(info) = self.app_info(a.app_id) else {
            return false;
        };
        installed_host(a).os != self.install_host(a.app_id, &info).os
    }

    /// Update detection (`content.md` §10): after the library or product
    /// info changes, compares each installed app's depots with what its
    /// branch names now.
    pub(super) fn check_updates(&mut self) {
        let Ok(steam_id) = self.active_account() else {
            return;
        };
        // Until the license list has arrived and the library is fetched,
        // what the account owns is not known: at the start the library
        // comes from the cache with no licenses, which owns no DLC.
        if !self.library.as_ref().is_some_and(|s| s.complete) {
            return;
        }
        let apps: Vec<InstalledApp> = self.content.installed.values().cloned().collect();
        for a in apps {
            // No depots now, as for a platform the app has none for, is
            // nothing an update could install.
            let Some(current) = self.current_depots(&a).filter(|c| !c.is_empty()) else {
                continue;
            };
            let changed = a.depots.iter().any(|d| {
                !current
                    .iter()
                    .any(|c| c.depot == d.depot && c.manifest == d.manifest)
            }) || self.platform_changed(&a);
            if changed == a.update_available {
                continue;
            }
            self.mark_update(a.app_id, changed);
            tracing::info!(app = %a.app_id, update = changed, "update state changed");
            if changed && self.config.auto_update() && self.unfinished_job(a.app_id).is_none() {
                let _ = self.create_job(
                    steam_id,
                    a.app_id,
                    Kind::Update,
                    a.folder_id.clone(),
                    a.branch.clone(),
                );
            }
            self.broadcast(
                Topic::Library,
                event::Kind::AppChanged(v1::AppChanged {
                    app_ids: vec![a.app_id.0],
                    complete: self.library.as_ref().is_some_and(|s| s.complete),
                }),
            );
        }
    }

    /// `content.optional_dlc.<app>` changed (`content.md` §10). A depot
    /// turned off is no longer chosen, which update detection sees. One
    /// turned on is chosen but not installed, which detection does not
    /// count (a refused key looks the same), so the app is marked here.
    pub(super) fn optional_dlc_changed(&mut self, app: AppId) {
        self.check_updates();
        let Ok(steam_id) = self.active_account() else {
            return;
        };
        if !self.library.as_ref().is_some_and(|s| s.complete) {
            return;
        }
        let Some(a) = self.content.installed.get(&app).cloned() else {
            return;
        };
        let Some(current) = self.current_depots(&a) else {
            return;
        };
        let missing = current
            .iter()
            .any(|c| c.optional.is_some() && !a.depots.iter().any(|d| d.depot == c.depot));
        if !missing || a.update_available {
            return;
        }
        self.mark_update(app, true);
        tracing::info!(app = %app, "optional DLC turned on; update state changed");
        if self.config.auto_update() && self.unfinished_job(app).is_none() {
            let _ = self.create_job(
                steam_id,
                app,
                Kind::Update,
                a.folder_id.clone(),
                a.branch.clone(),
            );
        }
        self.broadcast(
            Topic::Library,
            event::Kind::AppChanged(v1::AppChanged {
                app_ids: vec![app.0],
                complete: true,
            }),
        );
    }

    /// Stores the journals that jobs sent while the core was stopping.
    /// Stores what the jobs reported while the core was stopping: their
    /// last journals, and the results of jobs that ended, so a job that
    /// finished just then is recorded. Nothing new is scheduled.
    pub(super) fn drain_jobs(&mut self, commands: &crossbeam_channel::Receiver<super::Command>) {
        for cmd in commands.try_iter() {
            match cmd {
                super::Command::Content(Report::Journal { job_id, bytes }) => {
                    let _ = self.db.set_job_journal(job_id, Some(&bytes));
                }
                super::Command::Content(Report::Finished { job_id, result }) => {
                    self.finished(job_id, result);
                }
                _ => {}
            }
        }
    }
}

fn state_name(s: v1::JobState) -> &'static str {
    match s {
        v1::JobState::Unspecified => "in no state",
        v1::JobState::Queued => "queued",
        v1::JobState::Running => "running",
        v1::JobState::Paused => "paused",
        v1::JobState::Done => "done",
        v1::JobState::Failed => "failed",
        v1::JobState::Cancelled => "cancelled",
    }
}

/// The platform an install's depots were chosen for; empty is the host's.
fn installed_host(a: &InstalledApp) -> Host {
    let host = Host::this();
    let os = ["windows", "macos", "linux"]
        .into_iter()
        .find(|p| *p == a.platform)
        .unwrap_or(host.os);
    Host { os, ..host }
}
