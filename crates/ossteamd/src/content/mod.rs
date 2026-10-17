// SPDX-License-Identifier: GPL-2.0-or-later
//! Content jobs: install, update, verify and uninstall (`content.md`,
//! `ipc.md` §5.5, §5.6; SPEC §10).
//!
//! The core schedules jobs and keeps their state in the database. Each
//! running job has a thread of its own ([`job::run`]), which runs the
//! engine with `content.download_threads` download threads and reports
//! back to the core. Every job shares one HTTPS agent, one bandwidth
//! throttle and one content server list.

pub mod folders;
pub mod job;
pub mod script;

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use ossteam_content::cdn::{Fetch, Https};
use ossteam_content::throttle::Throttle;
use ossteam_ipc::v1;
use ossteam_net::Requester;

use crate::core::Command;
use job::{Control, Env, ServerList, Spec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update,
    Verify,
    Uninstall,
}

impl Kind {
    pub fn proto(self) -> v1::JobKind {
        match self {
            Self::Install => v1::JobKind::Install,
            Self::Update => v1::JobKind::Update,
            Self::Verify => v1::JobKind::Verify,
            Self::Uninstall => v1::JobKind::Uninstall,
        }
    }

    pub fn from_proto(k: v1::JobKind) -> Option<Self> {
        match k {
            v1::JobKind::Install => Some(Self::Install),
            v1::JobKind::Update => Some(Self::Update),
            v1::JobKind::Verify => Some(Self::Verify),
            v1::JobKind::Uninstall => Some(Self::Uninstall),
            v1::JobKind::Unspecified => None,
        }
    }
}

/// What a job tells the core.
pub enum Report {
    Progress {
        job_id: u64,
        progress: v1::JobProgressInfo,
    },
    /// The job's journal, to be stored with it (`content.md` §9).
    Journal { job_id: u64, bytes: Vec<u8> },
    Finished {
        job_id: u64,
        result: Result<job::Done, job::Failure>,
    },
}

/// Simultaneous connections the HTTPS agent keeps for reuse.
const CONNECTIONS: usize = 64;

/// The core's side of the running jobs.
pub struct Handle {
    fetch: Arc<dyn Fetch>,
    throttle: Arc<Throttle>,
    servers: Arc<ServerList>,
    scripts: Option<Arc<dyn script::System>>,
    core: Sender<Command>,
    running: HashMap<u64, (Arc<Control>, JoinHandle<()>)>,
}

impl Handle {
    /// Downloads go over HTTPS, limited to `bytes_per_sec` (0: no limit).
    /// Install scripts act on `scripts`, `None` where they do not apply.
    pub fn new(
        core: Sender<Command>,
        bytes_per_sec: u64,
        scripts: Option<Arc<dyn script::System>>,
    ) -> Self {
        let throttle = Arc::new(Throttle::new(bytes_per_sec));
        let fetch = Arc::new(Https::new(throttle.clone(), CONNECTIONS));
        Self::with_fetch(core, fetch, throttle, scripts)
    }

    /// With another transport: the fake CDN in tests (FAITH-4).
    pub fn with_fetch(
        core: Sender<Command>,
        fetch: Arc<dyn Fetch>,
        throttle: Arc<Throttle>,
        scripts: Option<Arc<dyn script::System>>,
    ) -> Self {
        Self {
            fetch,
            throttle,
            servers: Arc::new(ServerList::default()),
            scripts,
            core,
            running: HashMap::new(),
        }
    }

    pub fn set_bandwidth(&self, bytes_per_sec: u64) {
        self.throttle.set_rate(bytes_per_sec);
    }

    /// A new session: the server list is fetched again (`content.md` §1).
    pub fn new_session(&self) {
        self.servers.forget();
    }

    pub fn is_running(&self, job_id: u64) -> bool {
        self.running.contains_key(&job_id)
    }

    /// Starts a job on its own thread.
    pub fn start(&mut self, spec: Spec, requester: Requester) -> std::io::Result<()> {
        let job_id = spec.job_id;
        let control = Arc::new(Control::default());
        let env = Arc::new(Env {
            requester,
            fetch: self.fetch.clone(),
            servers: self.servers.clone(),
            scripts: self.scripts.clone(),
            core: self.core.clone(),
        });
        let c = control.clone();
        let thread = std::thread::Builder::new()
            .name(format!("job-{job_id}"))
            .spawn(move || job::run(spec, env, c))?;
        self.running.insert(job_id, (control, thread));
        Ok(())
    }

    /// Asks a running job to stop; it reports `Finished` with
    /// `Failure::Cancelled`. `discard` deletes its staging.
    pub fn stop(&self, job_id: u64, discard: bool) {
        if let Some((c, _)) = self.running.get(&job_id) {
            if discard {
                c.discard.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            c.cancel.cancel();
        }
    }

    /// Joins a job's thread once it reported `Finished`.
    pub fn finished(&mut self, job_id: u64) {
        if let Some((_, t)) = self.running.remove(&job_id) {
            let _ = t.join();
        }
    }

    /// Stops every job and waits for them; their journals stay, so they
    /// resume at the next start (`content.md` §9).
    pub fn shutdown(&mut self) {
        for (c, _) in self.running.values() {
            c.cancel.cancel();
        }
        for (_, (_, t)) in self.running.drain() {
            let _ = t.join();
        }
    }
}
