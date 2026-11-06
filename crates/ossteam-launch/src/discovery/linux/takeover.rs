// SPDX-License-Identifier: GPL-2.0-or-later
//! Taking over an official client's entries in the real `~/.steam`, handing
//! them back, and watching `~/.steam` meanwhile (`steamworks-abi.md`
//! §1.1.5, DISC-5). Whether to take over is the daemon's to decide; this
//! module checks the conditions it can see from the files and processes.

use std::collections::HashSet;
use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

use super::super::Error;
use super::super::files::remove;
use super::{Entries, Found, SDKS};

/// What the daemon recorded of the official entries it replaced:
/// `<config>/official-entries.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// The targets of the official `sdk64` and `sdk32` links.
    pub sdk64: Option<PathBuf>,
    pub sdk32: Option<PathBuf>,
    /// The text of the official `steam.pid`.
    pub pid: Option<String>,
}

impl Record {
    fn sdk(&self, sdk: &str) -> Option<&Path> {
        match sdk {
            "sdk64" => self.sdk64.as_deref(),
            _ => self.sdk32.as_deref(),
        }
    }

    fn sdk_mut(&mut self, sdk: &str) -> &mut Option<PathBuf> {
        match sdk {
            "sdk64" => &mut self.sdk64,
            _ => &mut self.sdk32,
        }
    }

    /// The record at `path`, if there is one.
    pub fn read(path: &Path) -> Result<Option<Self>, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io(format!("reading {}", path.display()), e)),
        };
        let bad = |m: String| {
            Error::io(
                format!("reading {}", path.display()),
                io::Error::new(io::ErrorKind::InvalidData, m),
            )
        };
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| bad(e.to_string()))?;
        let mut r = Self::default();
        for (key, value) in &table {
            let toml::Value::String(s) = value else {
                return Err(bad(format!("{key} is not a string")));
            };
            match key.as_str() {
                "sdk64" | "sdk32" => *r.sdk_mut(key) = Some(PathBuf::from(s)),
                "pid" => r.pid = Some(s.clone()),
                _ => return Err(bad(format!("unknown key {key}"))),
            }
        }
        Ok(Some(r))
    }

    /// Writes the record by renaming a new file over `path`.
    fn write(&self, path: &Path) -> Result<(), Error> {
        let mut table = toml::Table::new();
        for sdk in SDKS {
            if let Some(target) = self.sdk(sdk) {
                let text = target.to_str().ok_or_else(|| {
                    Error::io(
                        format!("recording ~/.steam/{sdk}"),
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("its target {} is not UTF-8", target.display()),
                        ),
                    )
                })?;
                table.insert(sdk.into(), text.into());
            }
        }
        if let Some(pid) = &self.pid {
            table.insert("pid".into(), pid.clone().into());
        }
        let text = format!(
            "# The official Steam client's ~/.steam entries, which ossteamd took over\n\
             # (discovery.take_over_official) and hands back from this record.\n{table}"
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::io(format!("writing {}", path.display()), e)
            })
    }
}

/// Why a take-over did not happen, or stopped part way.
#[derive(Debug, thiserror::Error)]
pub enum TakeOverError {
    /// A condition did not hold; what was taken is handed back.
    #[error("{0}")]
    Stopped(String),
    #[error(transparent)]
    Io(#[from] Error),
}

/// `dir/.<name>.ossteam`: where ossteam makes an entry before renaming
/// it into place. The [`Watcher`] knows ossteam's own changes by it.
fn temp_name(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.ossteam"))
}

fn is_temp_name(name: &[u8]) -> bool {
    name.starts_with(b".") && name.ends_with(b".ossteam")
}

/// Makes `link` a symlink to `target` by renaming a new link over it.
fn replace_link(target: &Path, link: &Path) -> Result<(), Error> {
    let dir = link.parent().unwrap_or(Path::new("."));
    let name = link.file_name().unwrap_or_default().to_string_lossy();
    let tmp = temp_name(dir, &name);
    let _ = std::fs::remove_file(&tmp);
    symlink(target, &tmp)
        .and_then(|()| std::fs::rename(&tmp, link))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::io(format!("replacing {}", link.display()), e)
        })
}

/// Writes `text` to `file` by renaming a new file over it.
fn replace_text(file: &Path, text: &str) -> Result<(), Error> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let tmp = temp_name(dir, &name);
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, file))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::io(format!("writing {}", file.display()), e)
        })
}

impl Entries {
    /// `~/.steam`.
    pub fn steam_dir(&self) -> &Path {
        &self.steam
    }

    /// Whether the real entries are ossteam's, with `pid` in `steam.pid`:
    /// what the daemon checks to see that it still holds them.
    pub fn held_by(&self, pid: u32) -> Result<bool, Error> {
        if !matches!(self.find("sdk64")?, Found::Ours)
            || matches!(self.find("sdk32")?, Found::Foreign(_))
        {
            return Ok(false);
        }
        Ok(std::fs::read_to_string(self.pid_file()).is_ok_and(|p| p.trim() == pid.to_string()))
    }

    /// Whether the official client runs (§1.1.3): `steam.pid` is not
    /// ossteam's and names a live process with `~/.steam/steam.pipe` open.
    pub fn official_runs(&self) -> Result<bool, Error> {
        if self.ours()? {
            return Ok(false);
        }
        Ok(self.pid_in_file().is_some_and(|pid| self.pipe_open(pid)))
    }

    /// The pid `steam.pid` names, if it holds one.
    fn pid_in_file(&self) -> Option<u32> {
        std::fs::read_to_string(self.pid_file())
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok())
    }

    /// Whether process `pid` has `~/.steam/steam.pipe` open.
    fn pipe_open(&self, pid: u32) -> bool {
        // The links in /proc/<pid>/fd are canonical paths.
        let dir = std::fs::canonicalize(&self.steam).unwrap_or_else(|_| self.steam.clone());
        let pipe = dir.join("steam.pipe");
        open_on(pid, &[pipe])
    }

    /// A process that shows the official client has not fully closed
    /// (§1.1.5), if there is one. `dirs` are [`Entries::official_dirs`],
    /// taken before the take-over began, since the links stop being the
    /// official client's as it goes on; each one's parent is an official
    /// installation.
    pub fn official_process(&self, dirs: &[PathBuf]) -> Option<Official> {
        let mut installs: Vec<&Path> = Vec::new();
        for d in dirs {
            if let Some(i) = d.parent()
                && !installs.contains(&i)
            {
                installs.push(i);
            }
        }
        if let Some(pid) = self.pid_in_file()
            && pid != std::process::id()
        {
            if self.pipe_open(pid) {
                return Some(Official {
                    pid,
                    what: OfficialKind::Runs,
                });
            }
            if std::fs::read_link(format!("/proc/{pid}/exe"))
                .is_ok_and(|exe| installs.iter().any(|i| exe.starts_with(i)))
            {
                return Some(Official {
                    pid,
                    what: OfficialKind::Exiting,
                });
            }
        }
        scan_processes(dirs, &installs)
    }

    /// The directories the official `sdk64` and `sdk32` point to,
    /// canonical, each once.
    pub fn official_dirs(&self) -> Result<Vec<PathBuf>, Error> {
        let mut dirs = Vec::new();
        for sdk in SDKS {
            if let Found::Foreign(_) = self.find(sdk)?
                && let Ok(dir) = std::fs::canonicalize(self.link(sdk))
                && !dirs.contains(&dir)
            {
                dirs.push(dir);
            }
        }
        Ok(dirs)
    }

    /// Replaces the official client's entries with ossteam's for the
    /// daemon `pid`, having recorded them in `record` (§1.1.5): each link
    /// by a rename, `steam.pid` last. `check` is asked before each rename
    /// whether the conditions still hold; at the first that does not, what
    /// was taken is handed back. The private `~/.steam` must be made
    /// already ([`Entries::create`]); an entry ossteam has no library for
    /// is removed rather than replaced.
    pub fn take_over(
        &self,
        record: &Path,
        pid: u32,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), TakeOverError> {
        if record.symlink_metadata().is_ok() {
            return Err(TakeOverError::Stopped(format!(
                "{} exists already",
                record.display()
            )));
        }
        check().map_err(TakeOverError::Stopped)?;
        let mut r = Record::default();
        for sdk in SDKS {
            if let Found::Foreign(what) = self.find(sdk)? {
                let target = std::fs::read_link(self.link(sdk)).map_err(|_| {
                    TakeOverError::Stopped(format!("{what} is not a link, so it is left alone"))
                })?;
                *r.sdk_mut(sdk) = Some(target);
            }
        }
        if !self.ours()? {
            r.pid = std::fs::read_to_string(self.pid_file()).ok();
        }
        if r == Record::default() {
            return Err(TakeOverError::Stopped(
                "there are no official entries to take over".into(),
            ));
        }
        r.write(record)?;
        let taken = (|| {
            for sdk in SDKS {
                if r.sdk(sdk).is_none() {
                    continue;
                }
                check().map_err(TakeOverError::Stopped)?;
                let ours = self.sdk.join(sdk);
                if ours.is_dir() {
                    replace_link(&ours, &self.link(sdk))?;
                } else {
                    remove(&self.link(sdk))?;
                }
            }
            check().map_err(TakeOverError::Stopped)?;
            replace_text(&self.pid_file(), &pid.to_string())?;
            Ok(())
        })();
        if taken.is_err()
            && let Err(e) = self.hand_back(record)
        {
            tracing::warn!("handing back the official entries failed: {e}");
        }
        taken
    }

    /// Puts back the official entries `record` holds, wherever ossteam's
    /// are in their place, by the same renames, `steam.pid` first; then
    /// deletes `record`. Returns whether there was a record.
    pub fn hand_back(&self, record: &Path) -> Result<bool, Error> {
        let Some(r) = Record::read(record)? else {
            return Ok(false);
        };
        if self.ours()? {
            match &r.pid {
                Some(text) => replace_text(&self.pid_file(), text)?,
                None => remove(&self.pid_file())?,
            }
        }
        for sdk in SDKS {
            let link = self.link(sdk);
            match (self.find(sdk)?, r.sdk(sdk)) {
                (Found::Ours | Found::Missing, Some(target)) => replace_link(target, &link)?,
                (Found::Ours, None) => remove(&link)?,
                _ => {}
            }
        }
        remove(record)?;
        Ok(true)
    }
}

/// A process of the official client that keeps the daemon from taking
/// over (§1.1.5). The daemon waits for it to exit ([`Watcher::watch_exit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Official {
    pub pid: u32,
    pub what: OfficialKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficialKind {
    /// `steam.pid`'s process, with `steam.pipe` open: the client runs
    /// (§1.1.3).
    Runs,
    /// `steam.pid`'s process, run from the official installation, with
    /// the pipe closed: the client is exiting, and still writes to
    /// `~/.steam`.
    Exiting,
    /// A process running the installation's `steam.sh`, which starts the
    /// client again when it exits to update.
    Script,
    /// The installation's `reaper`, which the official client starts each
    /// game under (`reaper SteamLaunch AppId=<id> -- <command>`). It lives
    /// as long as the game's processes do, so it covers those that have
    /// no `steamclient.so` loaded: a launcher before the game, a script,
    /// a crash handler.
    Reaper,
    /// A process with the official `steamclient.so` mapped: a game the
    /// official client started, which may outlive it.
    Game,
}

impl std::fmt::Display for Official {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.pid;
        match self.what {
            OfficialKind::Runs => write!(f, "the official Steam client runs (process {p})"),
            OfficialKind::Exiting => {
                write!(f, "the official Steam client (process {p}) is exiting")
            }
            OfficialKind::Script => write!(f, "process {p} runs the official client's steam.sh"),
            OfficialKind::Reaper => write!(
                f,
                "process {p} is the official client's reaper of a game it started"
            ),
            OfficialKind::Game => {
                write!(
                    f,
                    "process {p} has the official client's steamclient.so loaded"
                )
            }
        }
    }
}

/// Whether process `pid` has one of `files` (canonical paths) open.
fn open_on(pid: u32, files: &[PathBuf]) -> bool {
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    fds.flatten()
        .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| files.contains(&t)))
}

/// A process of this user, other than this one, that runs `steam.sh` or
/// `reaper` from one of `installs`, or has a `steamclient.so` mapped from
/// one of `dirs` (all canonical paths), if any.
fn scan_processes(dirs: &[PathBuf], installs: &[&Path]) -> Option<Official> {
    if dirs.is_empty() {
        return None;
    }
    let scripts: Vec<PathBuf> = installs.iter().map(|i| i.join("steam.sh")).collect();
    let uid = rustix::process::getuid().as_raw();
    let own = std::process::id();
    let procs = std::fs::read_dir("/proc").ok()?;
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if pid == own || !p.metadata().is_ok_and(|m| m.uid() == uid) {
            continue;
        }
        if runs_script(pid, &p.path(), &scripts) {
            return Some(Official {
                pid,
                what: OfficialKind::Script,
            });
        }
        if std::fs::read_link(p.path().join("exe")).is_ok_and(|exe| {
            exe.file_name().is_some_and(|n| n == "reaper")
                && installs.iter().any(|i| exe.starts_with(i))
        }) {
            return Some(Official {
                pid,
                what: OfficialKind::Reaper,
            });
        }
        let Ok(maps) = std::fs::read_to_string(p.path().join("maps")) else {
            continue;
        };
        let mapped = maps.lines().any(|line| {
            let Some(i) = line.find('/') else {
                return false;
            };
            let path = Path::new(line[i..].trim_end_matches(" (deleted)"));
            path.file_name().is_some_and(|n| n == "steamclient.so")
                && path.parent().is_some_and(|d| dirs.iter().any(|o| o == d))
        });
        if mapped {
            return Some(Official {
                pid,
                what: OfficialKind::Game,
            });
        }
    }
    None
}

/// Whether process `pid` (`/proc/<pid>` at `proc`) runs one of `scripts`:
/// an argument names a `steam.sh`, and the process has one of them open
/// (a shell keeps its script open) or the argument leads to one.
fn runs_script(pid: u32, proc: &Path, scripts: &[PathBuf]) -> bool {
    let Ok(cmdline) = std::fs::read(proc.join("cmdline")) else {
        return false;
    };
    let named: Vec<&Path> = cmdline
        .split(|&b| b == 0)
        .map(|a| Path::new(std::ffi::OsStr::from_bytes(a)))
        .filter(|a| a.file_name().is_some_and(|n| n == "steam.sh"))
        .collect();
    if named.is_empty() {
        return false;
    }
    if open_on(pid, scripts) {
        return true;
    }
    named.iter().any(|a| {
        // A relative argument is relative to the process's directory.
        let full = proc.join("cwd").join(a);
        std::fs::canonicalize(full).is_ok_and(|c| scripts.contains(&c))
    })
}

/// What the [`Watcher`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    /// Something in `~/.steam` was created, renamed, removed or written.
    Changed,
    /// A process given to [`Watcher::watch_exit`] exited.
    Exited,
    /// The interval passed with no change.
    Tick,
    /// `~/.steam` itself went away or moved; the watch has ended.
    Gone,
}

/// Watches `~/.steam` on a thread of its own (inotify), keeping when
/// someone other than ossteam last changed it, and waits for processes of
/// the official client to exit (§1.1.5).
pub struct Watcher {
    /// Dropped to stop the thread: its other end then reads as closed.
    stop: Option<OwnedFd>,
    /// Written to when `exits` has a new pid.
    wake: OwnedFd,
    exits: mpsc::Sender<u32>,
    thread: Option<JoinHandle<()>>,
    /// When someone other than ossteam last changed `~/.steam`; `None`
    /// once the official client has closed with no change since.
    last_foreign: Arc<Mutex<Option<Instant>>>,
}

impl Watcher {
    /// Starts watching `dir`, calling `on` with each report and every
    /// `tick` without one, until `on` returns false or the watcher is
    /// dropped. Counts as changed by someone else when it starts, since
    /// what happened before is unknown.
    pub fn start(
        dir: &Path,
        tick: Duration,
        on: impl FnMut(Watched) -> bool + Send + 'static,
    ) -> io::Result<Self> {
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        inotify::add_watch(
            &fd,
            dir,
            WatchFlags::CREATE
                | WatchFlags::DELETE
                | WatchFlags::MOVED_FROM
                | WatchFlags::MOVED_TO
                | WatchFlags::CLOSE_WRITE
                | WatchFlags::DELETE_SELF
                | WatchFlags::MOVE_SELF,
        )?;
        let (stop_r, stop_w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        let (wake_r, wake_w) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )?;
        let (exits, exits_r) = mpsc::channel();
        let last_foreign = Arc::new(Mutex::new(Some(Instant::now())));
        let last = last_foreign.clone();
        let thread = std::thread::Builder::new()
            .name("steam-dir-watch".into())
            .spawn(move || {
                let w = Watch {
                    fd,
                    stop: stop_r,
                    wake: wake_r,
                    exits: exits_r,
                    last_foreign: last,
                };
                w.run(tick, on);
            })?;
        Ok(Self {
            stop: Some(stop_w),
            wake: wake_w,
            exits,
            thread: Some(thread),
            last_foreign,
        })
    }

    /// How long since someone other than ossteam last changed `~/.steam`,
    /// or since the watch started; [`Duration::MAX`] when the official
    /// client has closed since, with no change after it.
    pub fn quiet_for(&self) -> Duration {
        self.last_foreign
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(Duration::MAX, |t| t.elapsed())
    }

    /// Reports [`Watched::Exited`] when process `pid` exits, at once if
    /// it has already. When the last process it waits for has exited,
    /// the changes to `~/.steam` until then count as the official
    /// client's: [`Watcher::quiet_for`] counts from then on.
    pub fn watch_exit(&self, pid: u32) {
        if self.exits.send(pid).is_ok() {
            let _ = rustix::io::write(&self.wake, &[0]);
        }
    }

    /// Whether the watch has ended ([`Watched::Gone`]).
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The watcher's thread.
struct Watch {
    /// The inotify instance.
    fd: OwnedFd,
    stop: OwnedFd,
    wake: OwnedFd,
    exits: mpsc::Receiver<u32>,
    last_foreign: Arc<Mutex<Option<Instant>>>,
}

impl Watch {
    fn run(&self, tick: Duration, mut on: impl FnMut(Watched) -> bool) {
        let timeout = Timespec::try_from(tick).unwrap_or(Timespec {
            tv_sec: 10,
            tv_nsec: 0,
        });
        // MOVED_FROM cookies of ossteam's own temporary names.
        let mut own_moves: HashSet<u32> = HashSet::new();
        // The processes waited for, by pid, with their pidfds.
        let mut waiting: Vec<(u32, OwnedFd)> = Vec::new();
        let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
        loop {
            let mut fds = vec![
                PollFd::new(&self.fd, PollFlags::IN),
                PollFd::new(&self.stop, PollFlags::IN),
                PollFd::new(&self.wake, PollFlags::IN),
            ];
            fds.extend(waiting.iter().map(|(_, p)| PollFd::new(p, PollFlags::IN)));
            match poll(&mut fds, Some(&timeout)) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => {
                    tracing::warn!("watching ~/.steam failed: {e}");
                    on(Watched::Gone);
                    return;
                }
            }
            if !fds[1].revents().is_empty() {
                return;
            }
            let ready: Vec<bool> = fds.iter().map(|f| !f.revents().is_empty()).collect();
            drop(fds);
            if !ready.contains(&true) {
                if !on(Watched::Tick) {
                    return;
                }
                continue;
            }

            // The changes first: what an exiting client wrote is queued
            // before its exit is seen.
            let (mut changed, mut gone) = (false, false);
            if ready[0] {
                (changed, gone) = self.read_changes(&mut buf, &mut own_moves);
            }
            let mut exited = false;
            if ready[2] {
                exited |= self.add_waits(&mut waiting);
            }
            let before = waiting.len();
            let mut i = 0;
            waiting.retain(|_| {
                i += 1;
                !ready.get(2 + i).copied().unwrap_or(false)
            });
            exited |= waiting.len() < before;
            if exited && waiting.is_empty() {
                *self
                    .last_foreign
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            if gone {
                on(Watched::Gone);
                return;
            }
            if changed && !on(Watched::Changed) {
                return;
            }
            if exited && !on(Watched::Exited) {
                return;
            }
        }
    }

    /// Reads the queued changes, keeping when someone else last made one.
    /// Returns whether there were any, and whether the watch has ended.
    fn read_changes(
        &self,
        buf: &mut [MaybeUninit<u8>],
        own_moves: &mut HashSet<u32>,
    ) -> (bool, bool) {
        let (mut changed, mut foreign, mut gone) = (false, false, false);
        let mut reader = inotify::Reader::new(self.fd.as_fd(), buf);
        loop {
            let e = match reader.next() {
                Ok(e) => e,
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => break,
                Err(e) => {
                    tracing::warn!("reading ~/.steam's changes failed: {e}");
                    gone = true;
                    break;
                }
            };
            let flags = e.events();
            if flags.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::IGNORED)
            {
                gone = true;
                continue;
            }
            changed = true;
            if !is_own(&e.file_name(), flags, e.cookie(), own_moves) {
                foreign = true;
            }
            if reader.is_buffer_empty() {
                break;
            }
        }
        if foreign {
            *self
                .last_foreign
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
        }
        (changed, gone)
    }

    /// Takes the pids [`Watcher::watch_exit`] sent. Returns whether one of
    /// them had already exited.
    fn add_waits(&self, waiting: &mut Vec<(u32, OwnedFd)>) -> bool {
        let mut drain = [0u8; 64];
        while matches!(rustix::io::read(&self.wake, &mut drain), Ok(n) if n > 0) {}
        let mut exited = false;
        for pid in self.exits.try_iter() {
            if waiting.iter().any(|(p, _)| *p == pid) {
                continue;
            }
            let Some(raw) = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            else {
                continue;
            };
            match rustix::process::pidfd_open(raw, rustix::process::PidfdFlags::empty()) {
                Ok(fd) => waiting.push((pid, fd)),
                Err(rustix::io::Errno::SRCH) => exited = true,
                Err(e) => tracing::warn!("cannot wait for process {pid} to exit: {e}"),
            }
        }
        exited
    }
}

/// Whether an event is ossteam's own: on one of its temporary names, or
/// the rename of one into place.
fn is_own(
    name: &Option<&CStr>,
    flags: ReadFlags,
    cookie: u32,
    own_moves: &mut HashSet<u32>,
) -> bool {
    let temp = name.is_some_and(|n| is_temp_name(n.to_bytes()));
    if flags.contains(ReadFlags::MOVED_FROM) && temp {
        own_moves.insert(cookie);
        return true;
    }
    if flags.contains(ReadFlags::MOVED_TO) {
        return own_moves.remove(&cookie);
    }
    temp
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::super::super::Libraries;
    use super::*;

    struct Dirs {
        root: PathBuf,
        home: PathBuf,
        data: PathBuf,
        libs: Libraries,
        valve: PathBuf,
        record: PathBuf,
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// An official client that has run: its links and its pid.
    fn dirs(name: &str, lib32: bool) -> Dirs {
        let root = std::env::temp_dir().join(format!("ossteam-take-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(bin.join("lib32")).unwrap();
        std::fs::write(bin.join("libossteam_steamclient.so"), b"64").unwrap();
        if lib32 {
            std::fs::write(bin.join("lib32/libossteam_steamclient.so"), b"32").unwrap();
        }
        let valve = root.join("Steam");
        for d in ["linux64", "linux32"] {
            std::fs::create_dir_all(valve.join(d)).unwrap();
        }
        let steam = home.join(".steam");
        std::fs::create_dir_all(&steam).unwrap();
        symlink(valve.join("linux64"), steam.join("sdk64")).unwrap();
        symlink(valve.join("linux32"), steam.join("sdk32")).unwrap();
        std::fs::write(steam.join("steam.pid"), "99").unwrap();
        Dirs {
            libs: super::super::super::find_libraries(&bin).unwrap(),
            data: root.join("data"),
            record: root.join("config/official-entries.toml"),
            home,
            valve,
            root,
        }
    }

    fn official_as_left(d: &Dirs) {
        let steam = d.home.join(".steam");
        assert_eq!(
            std::fs::read_link(steam.join("sdk64")).unwrap(),
            d.valve.join("linux64")
        );
        assert_eq!(
            std::fs::read_link(steam.join("sdk32")).unwrap(),
            d.valve.join("linux32")
        );
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "99"
        );
        assert!(!d.record.exists());
    }

    #[test]
    fn take_over_and_hand_back() {
        let d = dirs("both", true);
        let e = Entries::new(&d.home, &d.data);
        assert!(matches!(e.create(&d.libs, 7, 0), Err(Error::Conflict(_))));
        assert!(!e.official_runs().unwrap());
        assert_eq!(
            e.official_dirs().unwrap(),
            [
                std::fs::canonicalize(d.valve.join("linux64")).unwrap(),
                std::fs::canonicalize(d.valve.join("linux32")).unwrap()
            ]
        );
        assert!(!e.held_by(7).unwrap());

        e.take_over(&d.record, 7, &mut || Ok(())).unwrap();
        assert!(e.held_by(7).unwrap());
        e.check().unwrap();
        e.create(&d.libs, 7, 0).unwrap();
        let r = Record::read(&d.record).unwrap().unwrap();
        assert_eq!(r.sdk64, Some(d.valve.join("linux64")));
        assert_eq!(r.sdk32, Some(d.valve.join("linux32")));
        assert_eq!(r.pid.as_deref(), Some("99"));
        // No temporary names are left behind.
        let names: Vec<_> = std::fs::read_dir(e.steam_dir())
            .unwrap()
            .map(|n| n.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");

        assert!(e.hand_back(&d.record).unwrap());
        official_as_left(&d);
        assert!(!e.hand_back(&d.record).unwrap());
    }

    #[test]
    fn a_condition_failing_part_way_hands_back() {
        let d = dirs("stop", true);
        let e = Entries::new(&d.home, &d.data);
        let _ = e.create(&d.libs, 7, 0);
        let mut asked = 0;
        let r = e.take_over(&d.record, 7, &mut || {
            asked += 1;
            if asked < 3 {
                Ok(())
            } else {
                Err("the official client runs".into())
            }
        });
        assert!(matches!(r, Err(TakeOverError::Stopped(m)) if m.contains("runs")));
        official_as_left(&d);
    }

    #[test]
    fn without_a_32_bit_library_sdk32_is_removed_and_put_back() {
        let d = dirs("no32", false);
        let e = Entries::new(&d.home, &d.data);
        let _ = e.create(&d.libs, 7, 0);
        e.take_over(&d.record, 7, &mut || Ok(())).unwrap();
        assert!(e.held_by(7).unwrap());
        assert!(e.steam_dir().join("sdk32").symlink_metadata().is_err());
        e.hand_back(&d.record).unwrap();
        official_as_left(&d);
    }

    #[test]
    fn an_entry_the_official_client_made_again_is_left() {
        let d = dirs("yield", true);
        let e = Entries::new(&d.home, &d.data);
        let _ = e.create(&d.libs, 7, 0);
        e.take_over(&d.record, 7, &mut || Ok(())).unwrap();
        // steam.sh replaces sdk64 as the official client starts.
        let steam = e.steam_dir().to_owned();
        std::fs::remove_file(steam.join("sdk64")).unwrap();
        symlink(d.valve.join("linux64"), steam.join("sdk64")).unwrap();
        assert!(!e.held_by(7).unwrap());
        // Handing back now leaves steam.pid (sdk64 is not ossteam's) and
        // puts sdk32 back.
        e.hand_back(&d.record).unwrap();
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "7"
        );
        assert_eq!(
            std::fs::read_link(steam.join("sdk32")).unwrap(),
            d.valve.join("linux32")
        );
    }

    #[test]
    fn the_watcher_tells_its_own_changes_from_others() {
        let d = dirs("watch", true);
        let e = Entries::new(&d.home, &d.data);
        let _ = e.create(&d.libs, 7, 0);
        let (tx, rx) = mpsc::channel();
        let w = Watcher::start(e.steam_dir(), Duration::from_secs(60), move |x| {
            tx.send(x).is_ok()
        })
        .unwrap();
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let before = w.quiet_for();
        e.take_over(&d.record, 7, &mut || Ok(())).unwrap();
        assert_eq!(wait(), Watched::Changed);
        // Let every event of the take-over be read.
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
        assert!(w.quiet_for() >= before, "ossteam's renames are its own");

        std::fs::write(e.steam_dir().join("registry.vdf"), b"x").unwrap();
        assert_eq!(wait(), Watched::Changed);
        std::thread::sleep(Duration::from_millis(50));
        assert!(w.quiet_for() < Duration::from_millis(500));
        drop(w);
    }

    #[test]
    fn the_official_client_runs_while_its_pid_has_the_pipe_open() {
        let d = dirs("runs", true);
        let e = Entries::new(&d.home, &d.data);
        let steam = e.steam_dir().to_owned();
        std::fs::write(steam.join("steam.pid"), std::process::id().to_string()).unwrap();
        assert!(!e.official_runs().unwrap(), "no pipe open");
        let pipe = std::fs::File::create(steam.join("steam.pipe")).unwrap();
        assert!(e.official_runs().unwrap());
        drop(pipe);
        assert!(!e.official_runs().unwrap());
    }

    /// Kills and reaps a child when dropped.
    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn sleeper(program: &Path, stdin: std::process::Stdio) -> Child {
        Child(
            std::process::Command::new(program)
                .arg("30")
                .stdin(stdin)
                .spawn()
                .unwrap(),
        )
    }

    fn sleep_program() -> PathBuf {
        ["/usr/bin/sleep", "/bin/sleep"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .unwrap()
    }

    #[test]
    fn the_official_client_has_closed_once_its_processes_have_exited() {
        let d = dirs("closed", true);
        let e = Entries::new(&d.home, &d.data);
        let steam = e.steam_dir().to_owned();
        let dirs = e.official_dirs().unwrap();
        assert_eq!(e.official_process(&dirs), None, "pid 99 is not the client");

        // The client runs: steam.pid's process has the pipe open.
        std::fs::write(steam.join("steam.pipe"), b"").unwrap();
        let pipe = std::fs::File::open(steam.join("steam.pipe")).unwrap();
        let client = sleeper(&sleep_program(), pipe.into());
        let pid = client.0.id();
        std::fs::write(steam.join("steam.pid"), pid.to_string()).unwrap();
        let found = |what| Some(Official { pid, what });
        assert_eq!(e.official_process(&dirs), found(OfficialKind::Runs));
        drop(client);

        // The client has closed the pipe but runs on from the installation.
        let exe = d.valve.join("ubuntu12_32/steam");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::copy(sleep_program(), &exe).unwrap();
        let client = sleeper(&exe, std::process::Stdio::null());
        let pid = client.0.id();
        std::fs::write(steam.join("steam.pid"), pid.to_string()).unwrap();
        let found = |what| Some(Official { pid, what });
        assert_eq!(e.official_process(&dirs), found(OfficialKind::Exiting));
        drop(client);
        assert_eq!(e.official_process(&dirs), None);

        // steam.sh runs on, to start the client again after an update.
        let script = d.valve.join("steam.sh");
        std::fs::write(&script, "sleep 30\n").unwrap();
        let sh = Child(
            std::process::Command::new("sh")
                .arg(&script)
                .current_dir(&d.root)
                .spawn()
                .unwrap(),
        );
        let pid = sh.0.id();
        let found = |what| Some(Official { pid, what });
        assert_eq!(e.official_process(&dirs), found(OfficialKind::Script));
        drop(sh);
        assert_eq!(e.official_process(&dirs), None);

        // The reaper of a game the official client started runs on.
        let exe = d.valve.join("ubuntu12_32/reaper");
        std::fs::copy(sleep_program(), &exe).unwrap();
        let reaper = sleeper(&exe, std::process::Stdio::null());
        let pid = reaper.0.id();
        let found = |what| Some(Official { pid, what });
        assert_eq!(e.official_process(&dirs), found(OfficialKind::Reaper));
        drop(reaper);
        assert_eq!(e.official_process(&dirs), None);
    }

    #[test]
    fn the_watcher_wakes_when_the_official_client_exits() {
        let d = dirs("exit", true);
        let e = Entries::new(&d.home, &d.data);
        let (tx, rx) = mpsc::channel();
        let w = Watcher::start(e.steam_dir(), Duration::from_secs(60), move |x| {
            tx.send(x).is_ok()
        })
        .unwrap();
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(w.quiet_for() < Duration::from_secs(5), "the start counts");

        let client = sleeper(&sleep_program(), std::process::Stdio::null());
        let script = sleeper(&sleep_program(), std::process::Stdio::null());
        w.watch_exit(client.0.id());
        w.watch_exit(script.0.id());
        // The client writes as it closes, then exits.
        std::fs::write(e.steam_dir().join("registry.vdf"), b"x").unwrap();
        assert_eq!(wait(), Watched::Changed);
        drop(client);
        assert_eq!(wait(), Watched::Exited);
        assert!(w.quiet_for() < Duration::from_secs(5), "steam.sh runs on");
        drop(script);
        assert_eq!(wait(), Watched::Exited);
        assert_eq!(
            w.quiet_for(),
            Duration::MAX,
            "the client's changes are forgiven"
        );

        // A change after it closed counts again.
        std::fs::write(e.steam_dir().join("registry.vdf"), b"y").unwrap();
        assert_eq!(wait(), Watched::Changed);
        assert!(w.quiet_for() < Duration::from_secs(5));

        // A process already gone is reported at once.
        let gone = sleeper(&sleep_program(), std::process::Stdio::null());
        let pid = gone.0.id();
        drop(gone);
        w.watch_exit(pid);
        assert_eq!(wait(), Watched::Exited);
        assert_eq!(w.quiet_for(), Duration::MAX);
    }

    #[test]
    fn a_record_round_trips() {
        let d = dirs("record", true);
        let r = Record {
            sdk64: Some("/a b/linux64".into()),
            sdk32: None,
            pid: Some("12".into()),
        };
        r.write(&d.record).unwrap();
        assert_eq!(Record::read(&d.record).unwrap(), Some(r));
    }
}
