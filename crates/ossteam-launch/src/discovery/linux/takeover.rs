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
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
        let Some(pid) = std::fs::read_to_string(self.pid_file())
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok())
        else {
            return Ok(false);
        };
        // The links in /proc/<pid>/fd are canonical paths.
        let dir = std::fs::canonicalize(&self.steam).unwrap_or_else(|_| self.steam.clone());
        let pipe = dir.join("steam.pipe");
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return Ok(false);
        };
        Ok(fds
            .flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == pipe)))
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

/// A process of this user, other than this one, that has a
/// `steamclient.so` mapped from one of `dirs` (canonical paths), if any:
/// a game the official client started, which may outlive it (§1.1.5).
pub fn steamclient_mapped(dirs: &[PathBuf]) -> Option<u32> {
    if dirs.is_empty() {
        return None;
    }
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
            return Some(pid);
        }
    }
    None
}

/// What the [`Watcher`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    /// Something in `~/.steam` was created, renamed, removed or written.
    Changed,
    /// The interval passed with no change.
    Tick,
    /// `~/.steam` itself went away or moved; the watch has ended.
    Gone,
}

/// Watches `~/.steam` on a thread of its own (inotify), keeping when
/// someone other than ossteam last changed it (§1.1.5).
pub struct Watcher {
    /// Dropped to stop the thread: its other end then reads as closed.
    stop: Option<OwnedFd>,
    thread: Option<JoinHandle<()>>,
    last_foreign: Arc<Mutex<Instant>>,
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
        let last_foreign = Arc::new(Mutex::new(Instant::now()));
        let last = last_foreign.clone();
        let thread = std::thread::Builder::new()
            .name("steam-dir-watch".into())
            .spawn(move || watch(&fd, &stop_r, tick, &last, on))?;
        Ok(Self {
            stop: Some(stop_w),
            thread: Some(thread),
            last_foreign,
        })
    }

    /// How long since someone other than ossteam last changed `~/.steam`,
    /// or since the watch started.
    pub fn quiet_for(&self) -> Duration {
        self.last_foreign
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
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

fn watch(
    fd: &OwnedFd,
    stop: &OwnedFd,
    tick: Duration,
    last_foreign: &Mutex<Instant>,
    mut on: impl FnMut(Watched) -> bool,
) {
    let timeout = Timespec::try_from(tick).unwrap_or(Timespec {
        tv_sec: 10,
        tv_nsec: 0,
    });
    // MOVED_FROM cookies of ossteam's own temporary names.
    let mut own_moves: HashSet<u32> = HashSet::new();
    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    loop {
        let mut fds = [
            PollFd::new(fd, PollFlags::IN),
            PollFd::new(stop, PollFlags::IN),
        ];
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
        if fds[0].revents().is_empty() {
            if !on(Watched::Tick) {
                return;
            }
            continue;
        }
        let mut changed = false;
        let mut foreign = false;
        let mut gone = false;
        let mut reader = inotify::Reader::new(fd.as_fd(), &mut buf);
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
            if !is_own(&e.file_name(), flags, e.cookie(), &mut own_moves) {
                foreign = true;
            }
            if reader.is_buffer_empty() {
                break;
            }
        }
        if foreign {
            *last_foreign
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        }
        if gone {
            on(Watched::Gone);
            return;
        }
        if changed && !on(Watched::Changed) {
            return;
        }
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
