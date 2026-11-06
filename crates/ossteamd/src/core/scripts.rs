// SPDX-License-Identifier: GPL-2.0-or-later
//! Install scripts at the start of a game (`install-scripts.md` §3.1):
//! a thread per launch plans and takes the steps, natively on Windows or
//! in the app's Proton prefix (`compat-tools.md` §7), and reports each
//! step and the values written back to the core, which then goes on with
//! the launch.

use std::collections::HashMap;
use std::path::PathBuf;

use crossbeam_channel::Sender;
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::{self, ErrorCode, Topic, event};
use ossteam_types::{AppId, CancelToken, DepotId};

use super::{Command, Core};
use crate::content::script::prefix::{Prefix, Setup};
use crate::content::script::{self, ScriptFile, ValueRef};

/// Where the steps act.
pub enum Target {
    /// This Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    Windows,
    /// The prefix in this compatibility data directory.
    Prefix { compat_data: PathBuf, setup: Setup },
}

/// What a launch's scripts need, gathered on the core thread.
pub struct Work {
    pub app: AppId,
    pub files: Vec<ScriptFile>,
    /// The directory of each depot that has scripts.
    pub dirs: HashMap<DepotId, PathBuf>,
    pub language: String,
    pub target: Target,
}

/// From a launch's script thread.
pub enum Report {
    /// A step started (`LaunchStep`, `launch.md` §8).
    Step {
        app: AppId,
        text: String,
    },
    Done(Done),
}

pub struct Done {
    pub app: AppId,
    /// What the steps wrote, to record with the install (§5).
    pub written: Vec<ValueRef>,
    /// `Err` stops the launch: a script that does not match its manifest.
    pub result: Result<(), RemoteError>,
}

/// Starts the thread that takes `work`'s steps.
pub fn spawn(work: Work, core: Sender<Command>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(format!("scripts-{}", work.app))
        .spawn(move || {
            let done = run(work, &core);
            let _ = core.send(Command::Scripts(Report::Done(done)));
        })
        .map(drop)
}

fn run(work: Work, core: &Sender<Command>) -> Done {
    let app = work.app;
    let mut report = |text: String| {
        tracing::info!(app = %app, "install scripts: {text}");
        let _ = core.send(Command::Scripts(Report::Step { app, text }));
    };
    let done = |result| Done {
        app,
        written: Vec::new(),
        result,
    };
    let system: std::sync::Arc<dyn script::System> = match work.target {
        Target::Windows => match script::system() {
            Some(s) => s,
            None => return done(Ok(())),
        },
        Target::Prefix { compat_data, setup } => {
            match Prefix::open(&compat_data, setup, &mut report) {
                Ok(p) => std::sync::Arc::new(p),
                Err(e) => {
                    // Nothing in the scripts stops a launch but a damaged
                    // file (§3.1); the game may do without them.
                    tracing::warn!(app = %app, "install scripts not applied: {e}");
                    return done(Ok(()));
                }
            }
        }
    };
    let dir_of = |d: DepotId| work.dirs.get(&d).cloned();
    let loaded = script::load(&work.files, &dir_of, &work.language, system.as_ref());
    if !loaded.unusable.is_empty() {
        let names: Vec<&str> = loaded.unusable.iter().map(|f| f.path.as_str()).collect();
        return done(Err(RemoteError::new(
            ErrorCode::NotFound,
            format!(
                "the install script {} does not match the installed files; verify app {app}",
                names.join(", ")
            ),
        )));
    }
    let plan = script::start(&loaded.scripts, system.as_ref());
    // Nothing cancels it: a shutdown leaves it running, unwatched (§3.1).
    let out = script::execute(system.as_ref(), &plan, &CancelToken::new(), &mut report)
        .expect("never cancelled");
    Done {
        app,
        written: script::written(&plan, &out),
        result: Ok(()),
    }
}

impl Core {
    pub(super) fn on_scripts(&mut self, report: Report) {
        match report {
            Report::Step { app, text } => self.broadcast(
                Topic::Games,
                event::Kind::LaunchStep(v1::LaunchStep {
                    app_id: app.0,
                    step: v1::LaunchStepKind::InstallScript as i32,
                    text,
                }),
            ),
            Report::Done(done) => {
                let app = done.app;
                if !done.written.is_empty()
                    && let Some(installed) = self.content.installed.get_mut(&app)
                {
                    script::record(&mut installed.script_values, done.written);
                    if let Err(e) = self.db.set_installed(installed) {
                        tracing::error!("recording app {app}'s install script values: {e:#}");
                    }
                }
                self.scripts_done(app, done.result);
            }
        }
    }
}
