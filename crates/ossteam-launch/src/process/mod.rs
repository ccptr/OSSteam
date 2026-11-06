// SPDX-License-Identifier: GPL-2.0-or-later
//! A game's process tree: starting it, waiting until the last process is
//! gone, and stopping it (`launch.md` §4 and §5, LAUNCH-4, LAUNCH-5).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ossteam_types::AppId;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

#[cfg(target_os = "linux")]
pub use unix::reaper_main;

/// How long a stopped game has to close before it is killed.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// What to start.
#[derive(Debug, Clone)]
pub struct Spec {
    pub app: AppId,
    /// The executable's absolute path.
    pub executable: PathBuf,
    /// `command::shell_command_line` on Unix, `command::windows_command_line`
    /// on Windows.
    pub command_line: String,
    pub working_dir: PathBuf,
    /// Set on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// Standard output and error go here; the file is replaced, unless
    /// `append_log`.
    pub log: PathBuf,
    /// Adds to `log` instead: for a launch's setup commands and the game
    /// after them (`launch.md` §3).
    pub append_log: bool,
    /// Linux: the program that runs `--reaper` (the daemon's own
    /// executable). Unused elsewhere.
    pub reaper: PathBuf,
    /// Linux: stopping sends `SIGINT` to the reaper's children first, for
    /// a compatibility tool with `use_tool_subprocess_reaper`
    /// (`compat-tools.md` §10). Unused elsewhere.
    pub interrupt_first: bool,
    /// Linux: the private `~/.steam` the reaper, and so every process of
    /// the launch, runs in (`steamworks-abi.md` §1.1.2). Always `None`
    /// elsewhere.
    pub isolation: Option<Arc<crate::isolation::Plan>>,
}

impl Spec {
    /// The log file, replaced or added to.
    fn open_log(&self) -> std::io::Result<std::fs::File> {
        if self.append_log {
            std::fs::File::options()
                .create(true)
                .append(true)
                .open(&self.log)
        } else {
            std::fs::File::create(&self.log)
        }
    }
}

/// Why a game was not started.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// A step of the private `~/.steam` failed (`steamworks-abi.md` §1.1.2).
    #[error("games get no private ~/.steam: {0}")]
    Isolation(#[from] crate::isolation::Failure),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How the process the daemon started ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i64),
    Signal(i32),
}

/// A started game. [`Game::wait`] blocks until its tree is gone; a
/// [`Stopper`] taken before that can stop it from another thread.
pub struct Game {
    pid: u32,
    inner: os::Game,
}

impl Game {
    pub fn spawn(spec: &Spec) -> Result<Self, SpawnError> {
        let inner = os::Game::spawn(spec)?;
        Ok(Self {
            pid: inner.pid(),
            inner,
        })
    }

    /// The process the daemon started: the reaper on Linux.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stopper(&self) -> Stopper {
        Stopper(self.inner.stopper())
    }

    /// Waits until the last process of the game is gone.
    pub fn wait(self) -> std::io::Result<Exit> {
        self.inner.wait()
    }
}

/// Stops a game: asks it to close, then kills what is left after
/// `timeout`.
#[derive(Clone)]
pub struct Stopper(os::Stopper);

impl Stopper {
    /// Returns once the request is made; the kill, if needed, happens on
    /// a thread of its own.
    pub fn stop(&self, timeout: Duration) {
        self.0.stop(timeout);
    }

    /// Whether `pid` is a process of the game's tree (`launch.md` §4): a
    /// descendant of the reaper on Linux, a member of the game's process
    /// group on macOS, a process in its job object on Windows. False once
    /// the tree is gone.
    pub fn contains(&self, pid: u32) -> bool {
        self.0.contains(pid)
    }
}
