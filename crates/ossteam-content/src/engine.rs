// SPDX-License-Identifier: GPL-2.0-or-later
//! The install engine: install, delta update and verify of one app's
//! depots into its install directories (`content.md` §8 to §11, CONT-4 to
//! CONT-6).
//!
//! A job is prepared, which decides what to write and, for verify, hashes
//! the installed files; the daemon then checks disk space and runs it. A
//! run has three steps:
//!
//! 1. **Local.** Every file to write is created in staging. Each of its
//!    chunks is taken from staging if the journal says it is there, or
//!    copied from an installed file that holds a chunk with the same id,
//!    and checked either way. Nothing is renamed during this step, so the
//!    installed files read are still the old ones.
//! 2. **Download.** The remaining chunks are fetched by a pool of threads.
//!    A file is renamed into place as soon as its last chunk is written.
//! 3. **Finish.** Symbolic links are made, and files and directories the
//!    new manifests no longer list are deleted.
//!
//! Every path comes from a checked manifest (`Manifest::check`). Writes
//! and deletes never go through a symbolic link inside the install
//! directory, so a depot cannot use one to reach outside it.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ossteam_types::{CancelToken, DepotId, ManifestId};
use sha1::{Digest, Sha1};

use crate::chunk;
use crate::download::{DownloadError, Downloader};
use crate::key::DepotKey;
use crate::manifest::{ChunkId, File, Manifest, flags};

/// The journal is handed to the daemon at most this often (`content.md` §9).
pub const JOURNAL_INTERVAL: Duration = Duration::from_secs(1);
/// Progress is reported at most this often (SPEC §8.3: 10 Hz per job).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// How many installed copies of a chunk are tried before downloading it.
const CANDIDATES: usize = 4;

/// One depot of a job.
pub struct DepotWork {
    pub key: DepotKey,
    /// `<library>/common/<installdir>`, of this app or, for a shared
    /// install, of the app the depot is shared from.
    pub install_dir: PathBuf,
    /// What to install, checked and with names decrypted.
    pub manifest: Manifest,
    /// What is installed now, for an update; `None` for a new install.
    pub installed: Option<Manifest>,
}

/// A depot no longer chosen, whose files are deleted (`content.md` §10).
pub struct Removal {
    pub install_dir: PathBuf,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A new install or an update; which files to write is decided by
    /// comparing manifests.
    Install,
    /// Re-hash the installed files and repair those that differ.
    Verify,
}

pub struct Job {
    pub mode: Mode,
    pub depots: Vec<DepotWork>,
    pub removed: Vec<Removal>,
    /// `<library>/.ossteam/staging/<appid>`.
    pub staging: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Verify is hashing installed files.
    #[default]
    Checking,
    /// Chunks are being taken from staging and installed files.
    Copying,
    Downloading,
    /// Links and deletions.
    Finishing,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub phase: Phase,
    /// File bytes the job writes, and how many are written. During
    /// `Checking`, bytes to hash and hashed instead.
    pub bytes_total: u64,
    pub bytes_done: u64,
    /// Bytes to download as served, once known (after `Copying`), and how
    /// many are downloaded, counting earlier runs of a resumed job.
    pub download_total: u64,
    pub downloaded: u64,
    pub files_total: u64,
    pub files_done: u64,
}

/// What a finished job did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Files written; for verify, the files repaired.
    pub files_written: u64,
    pub bytes_downloaded: u64,
    /// Chunks copied from installed files instead of downloaded.
    pub chunks_copied: u64,
    pub files_deleted: u64,
    /// Symbolic links made or remade.
    pub links: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("{}: a symbolic link or a file is where the depot has a directory", .0.display())]
    Unsafe(PathBuf),
    #[error("{}: a directory is where the depot has a file", .0.display())]
    InTheWay(PathBuf),
    #[error(
        "{}: the symbolic link cannot be made: {}{}",
        .0.display(),
        .1,
        if cfg!(windows) { " (Windows allows this only with Developer Mode or the privilege to create symbolic links)" } else { "" }
    )]
    Symlink(PathBuf, io::Error),
    #[error("cancelled")]
    Cancelled,
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> EngineError + '_ {
    move |source| EngineError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Callbacks a running job makes, from any of its threads.
pub struct Hooks<'a> {
    pub progress: &'a (dyn Fn(&Progress) + Sync),
    /// The journal, encoded, to be stored with the job (`content.md` §9).
    pub journal: &'a (dyn Fn(Vec<u8>) + Sync),
}

impl Hooks<'_> {
    /// Hooks that do nothing.
    pub fn none() -> Hooks<'static> {
        Hooks {
            progress: &|_| {},
            journal: &|_| {},
        }
    }
}

// --- The journal -----------------------------------------------------------

const JOURNAL_MAGIC: &[u8; 4] = b"OSJ2";
/// The journal before it counted downloaded bytes.
const JOURNAL_MAGIC_1: &[u8; 4] = b"OSJ1";

/// The chunks a job has written to staging, by depot, file and offset,
/// and the bytes it has downloaded (`content.md` §9). It is only trusted
/// for the manifests it was made with; each chunk is checked again before
/// it is used.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Journal {
    manifests: Vec<(DepotId, ManifestId)>,
    staged: HashSet<(DepotId, [u8; 20], u64)>,
    downloaded: u64,
}

impl Journal {
    fn new(job: &Job) -> Self {
        Self {
            manifests: manifests_of(job),
            staged: HashSet::new(),
            downloaded: 0,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = JOURNAL_MAGIC.to_vec();
        out.extend(self.downloaded.to_le_bytes());
        out.extend((self.manifests.len() as u32).to_le_bytes());
        for (d, m) in &self.manifests {
            out.extend(d.0.to_le_bytes());
            out.extend(m.0.to_le_bytes());
        }
        let mut staged: Vec<_> = self.staged.iter().collect();
        staged.sort();
        out.extend((staged.len() as u32).to_le_bytes());
        for (d, name, offset) in staged {
            out.extend(d.0.to_le_bytes());
            out.extend(name);
            out.extend(offset.to_le_bytes());
        }
        out
    }

    /// `None` for anything that is not a journal this build wrote, or one
    /// of the format before it (which counts nothing downloaded).
    pub fn decode(data: &[u8]) -> Option<Self> {
        let (mut r, counted) = match data.strip_prefix(JOURNAL_MAGIC) {
            Some(r) => (r, true),
            None => (data.strip_prefix(JOURNAL_MAGIC_1)?, false),
        };
        fn take<'a>(r: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
            let (head, rest) = (r.get(..n)?, r.get(n..)?);
            *r = rest;
            Some(head)
        }
        let u32_ = |r: &mut &[u8]| take(r, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let u64_ = |r: &mut &[u8]| take(r, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let mut j = Journal::default();
        if counted {
            j.downloaded = u64_(&mut r)?;
        }
        for _ in 0..u32_(&mut r)? {
            j.manifests
                .push((DepotId(u32_(&mut r)?), ManifestId(u64_(&mut r)?)));
        }
        for _ in 0..u32_(&mut r)? {
            let d = DepotId(u32_(&mut r)?);
            let name: [u8; 20] = take(&mut r, 20)?.try_into().ok()?;
            j.staged.insert((d, name, u64_(&mut r)?));
        }
        r.is_empty().then_some(j)
    }
}

fn manifests_of(job: &Job) -> Vec<(DepotId, ManifestId)> {
    let mut v: Vec<_> = job
        .depots
        .iter()
        .map(|d| (d.manifest.depot, d.manifest.id))
        .collect();
    v.sort();
    v
}

// --- Paths -----------------------------------------------------------------

fn components(f: &File) -> Vec<&str> {
    // Manifests are checked before they reach the engine.
    f.components().expect("manifest names are checked")
}

fn join(root: &Path, parts: &[&str]) -> PathBuf {
    let mut p = root.to_owned();
    p.extend(parts);
    p
}

/// Whether every ancestor of `root/parts` below `root` that exists is a
/// real directory, not a symbolic link.
fn ancestors_safe(root: &Path, parts: &[&str]) -> bool {
    let mut p = root.to_owned();
    for part in &parts[..parts.len().saturating_sub(1)] {
        p.push(part);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return false,
            Err(_) => return true,
        }
    }
    true
}

/// Creates the directories `root/parts[..n]`, refusing to go through
/// anything that is not a real directory. `with_last` includes the last
/// component.
fn make_dirs(root: &Path, parts: &[&str], with_last: bool) -> Result<(), EngineError> {
    fs::create_dir_all(root).map_err(io_err(root))?;
    let n = if with_last {
        parts.len()
    } else {
        parts.len().saturating_sub(1)
    };
    let mut p = root.to_owned();
    for part in &parts[..n] {
        p.push(part);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(EngineError::Unsafe(p)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
                        return Err(EngineError::Unsafe(p));
                    }
                }
                Err(e) => return Err(io_err(&p)(e)),
            },
            Err(e) => return Err(io_err(&p)(e)),
        }
    }
    Ok(())
}

fn name_hash(f: &File) -> [u8; 20] {
    Sha1::digest(f.key().as_bytes()).into()
}

fn read_at(path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_at(path: &Path, offset: u64, data: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(data)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// Sets the permission bits the manifest's flags ask for (`content.md`
/// §8).
fn apply_flags(path: &Path, file_flags: u32) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = perms.mode();
        if file_flags & flags::EXECUTABLE != 0 {
            mode |= 0o111;
        } else {
            mode &= !0o111;
        }
        if file_flags & flags::READ_ONLY != 0 {
            mode &= !0o222;
        } else {
            mode |= 0o200;
        }
        perms.set_mode(mode);
    }
    #[cfg(not(unix))]
    perms.set_readonly(file_flags & flags::READ_ONLY != 0);
    fs::set_permissions(path, perms)
}

/// Windows refuses to replace or delete a read-only file.
fn clear_read_only(path: &Path) {
    if cfg!(windows)
        && let Ok(m) = fs::symlink_metadata(path)
        && m.is_file()
        && m.permissions().readonly()
    {
        let mut p = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = fs::set_permissions(path, p);
    }
}

fn remove_file(path: &Path) -> io::Result<bool> {
    clear_read_only(path);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn is_user_config(f: &File) -> bool {
    f.flags & (flags::USER_CONFIG | flags::VERSIONED_USER_CONFIG) != 0
}

/// SHA-1 of a file, `None` if it cannot be read. Counts bytes into `done`.
fn hash_file(
    path: &Path,
    done: &AtomicU64,
    cancel: &CancelToken,
) -> Result<Option<[u8; 20]>, EngineError> {
    let Ok(mut f) = fs::File::open(path) else {
        return Ok(None);
    };
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        match f.read(&mut buf) {
            Ok(0) => return Ok(Some(h.finalize().into())),
            Ok(n) => {
                h.update(&buf[..n]);
                done.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Ok(None),
        }
    }
}

// --- Preparing -------------------------------------------------------------

/// A file to assemble in staging and rename into place.
struct Rebuild {
    depot: usize,
    file: usize,
    dest: PathBuf,
    staged: PathBuf,
    name: [u8; 20],
}

/// A job ready to run.
pub struct Prepared<'j> {
    job: &'j Job,
    journal: Journal,
    rebuilds: Vec<Rebuild>,
    /// Directories to create: depot index, file index.
    dirs: Vec<(usize, usize)>,
    links: Vec<(usize, usize)>,
    /// Files whose only change is their flags.
    reflag: Vec<(PathBuf, u32)>,
    delete_files: Vec<PathBuf>,
    delete_dirs: Vec<PathBuf>,
    /// For each chunk id, where installed files may hold it.
    local: HashMap<ChunkId, Vec<(PathBuf, u64)>>,
    /// File bytes the run writes that are not staged yet.
    pub bytes_to_write: u64,
}

struct Reporter<'a> {
    hooks: &'a Hooks<'a>,
    last: Mutex<Instant>,
    phase: Mutex<Phase>,
    bytes_total: AtomicU64,
    bytes_done: AtomicU64,
    download_total: AtomicU64,
    downloaded: AtomicU64,
    files_total: AtomicU64,
    files_done: AtomicU64,
    /// A report was held back by the interval; [`Reporter::flush`] sends
    /// it, so the last state before a stall is not lost.
    pending: AtomicBool,
}

impl<'a> Reporter<'a> {
    fn new(hooks: &'a Hooks<'a>) -> Self {
        Self {
            hooks,
            last: Mutex::new(Instant::now() - PROGRESS_INTERVAL),
            phase: Mutex::new(Phase::Checking),
            bytes_total: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            download_total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            pending: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> Progress {
        Progress {
            phase: *self.phase.lock().unwrap_or_else(|e| e.into_inner()),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            download_total: self.download_total.load(Ordering::Relaxed),
            downloaded: self.downloaded.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            files_done: self.files_done.load(Ordering::Relaxed),
        }
    }

    fn phase(&self, p: Phase) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = p;
        self.report(true);
    }

    /// Reports unless one went out less than [`PROGRESS_INTERVAL`] ago.
    fn report(&self, force: bool) {
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.elapsed() < PROGRESS_INTERVAL {
                self.pending.store(true, Ordering::Relaxed);
                return;
            }
            *last = Instant::now();
            self.pending.store(false, Ordering::Relaxed);
        }
        (self.hooks.progress)(&self.snapshot());
    }

    /// Sends a held-back report once the interval has passed.
    fn flush(&self) {
        if self.pending.load(Ordering::Relaxed) {
            self.report(false);
        }
    }
}

/// Decides what a job writes and deletes. For verify this hashes every
/// installed file, which is the slow part. `journal` is what an earlier
/// run of the same job stored, if any.
pub fn prepare<'j>(
    job: &'j Job,
    journal: Option<&[u8]>,
    hooks: &Hooks<'_>,
    cancel: &CancelToken,
) -> Result<Prepared<'j>, EngineError> {
    let journal = journal
        .and_then(Journal::decode)
        .filter(|j| j.manifests == manifests_of(job))
        .unwrap_or_else(|| Journal::new(job));
    let rep = Reporter::new(hooks);
    let hashed = AtomicU64::new(0);
    if job.mode == Mode::Verify {
        let total = job
            .depots
            .iter()
            .flat_map(|d| &d.manifest.files)
            .map(|f| f.size)
            .sum();
        rep.bytes_total.store(total, Ordering::Relaxed);
        rep.phase(Phase::Checking);
    }

    let mut p = Prepared {
        job,
        journal,
        rebuilds: Vec::new(),
        dirs: Vec::new(),
        links: Vec::new(),
        reflag: Vec::new(),
        delete_files: Vec::new(),
        delete_dirs: Vec::new(),
        local: HashMap::new(),
        bytes_to_write: 0,
    };

    for (di, d) in job.depots.iter().enumerate() {
        let installed: HashMap<String, &File> = d
            .installed
            .iter()
            .flat_map(|m| &m.files)
            .map(|f| (f.key(), f))
            .collect();
        for (fi, f) in d.manifest.files.iter().enumerate() {
            let parts = components(f);
            let dest = join(&d.install_dir, &parts);
            if f.is_dir() {
                p.dirs.push((di, fi));
                continue;
            }
            if f.is_symlink() {
                p.links.push((di, fi));
                continue;
            }
            let exists = fs::symlink_metadata(&dest).is_ok();
            let write = match job.mode {
                Mode::Install => {
                    if f.flags & flags::USER_CONFIG != 0 && exists {
                        false
                    } else if let Some(old) = installed.get(&f.key())
                        && old.size == f.size
                        && old.sha == f.sha
                    {
                        if old.flags != f.flags && exists {
                            p.reflag.push((dest.clone(), f.flags));
                        }
                        false
                    } else {
                        true
                    }
                }
                Mode::Verify => {
                    if is_user_config(f) {
                        !exists
                    } else {
                        let good = is_regular_file(&dest)
                            && ancestors_safe(&d.install_dir, &parts)
                            && fs::metadata(&dest).is_ok_and(|m| m.len() == f.size)
                            && (f.size == 0
                                || hash_file(&dest, &hashed, cancel)?
                                    .is_some_and(|h| h[..] == f.sha[..]));
                        rep.bytes_done
                            .store(hashed.load(Ordering::Relaxed), Ordering::Relaxed);
                        rep.report(false);
                        !good
                    }
                }
            };
            if write {
                let name = name_hash(f);
                let staged = job
                    .staging
                    .join(d.manifest.depot.0.to_string())
                    .join(ChunkId(name).to_string());
                p.rebuilds.push(Rebuild {
                    depot: di,
                    file: fi,
                    dest,
                    staged,
                    name,
                });
            }
        }
    }

    // Where chunks may be found on disk: the installed manifests, the new
    // ones (a resumed update may have put some new files in place
    // already), and depots being removed.
    let mut add_local = |root: &Path, m: &Manifest| {
        for f in m.files.iter().filter(|f| !f.is_dir() && !f.is_symlink()) {
            let parts = components(f);
            if !ancestors_safe(root, &parts) {
                continue;
            }
            let path = join(root, &parts);
            for c in &f.chunks {
                let v = p.local.entry(c.id).or_default();
                if v.len() < CANDIDATES && !v.iter().any(|(q, o)| *q == path && *o == c.offset) {
                    v.push((path.clone(), c.offset));
                }
            }
        }
    };
    for d in &job.depots {
        if let Some(m) = &d.installed {
            add_local(&d.install_dir, m);
        }
    }
    for r in &job.removed {
        add_local(&r.install_dir, &r.manifest);
    }
    for d in &job.depots {
        add_local(&d.install_dir, &d.manifest);
    }

    // Deletions (`content.md` §10): what the old manifests list and the new
    // ones do not, except user configuration files.
    if job.mode == Mode::Install {
        let listed: HashSet<PathBuf> = job
            .depots
            .iter()
            .flat_map(|d| {
                d.manifest
                    .files
                    .iter()
                    .map(|f| join(&d.install_dir, &components(f)))
            })
            .collect();
        let old = job
            .depots
            .iter()
            .filter_map(|d| Some((&d.install_dir, d.installed.as_ref()?)))
            .chain(job.removed.iter().map(|r| (&r.install_dir, &r.manifest)));
        for (root, m) in old {
            for f in &m.files {
                let parts = components(f);
                let path = join(root, &parts);
                if listed.contains(&path) || is_user_config(f) || !ancestors_safe(root, &parts) {
                    continue;
                }
                if f.is_dir() {
                    p.delete_dirs.push(path);
                } else {
                    p.delete_files.push(path);
                }
            }
        }
        // Deepest first, so a directory is emptied before it is removed.
        p.delete_dirs
            .sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        p.delete_dirs.dedup();
    }

    for r in &p.rebuilds {
        let f = &job.depots[r.depot].manifest.files[r.file];
        let staged: u64 = f
            .chunks
            .iter()
            .filter(|c| {
                p.journal
                    .staged
                    .contains(&(job.depots[r.depot].manifest.depot, r.name, c.offset))
            })
            .map(|c| u64::from(c.original))
            .sum();
        p.bytes_to_write += f.size.saturating_sub(staged);
    }
    Ok(p)
}

// --- Running ---------------------------------------------------------------

struct Run<'a> {
    p: &'a Prepared<'a>,
    rep: Reporter<'a>,
    journal: Mutex<(Journal, Instant)>,
    /// Chunks each rebuild still waits for.
    remaining: Vec<AtomicUsize>,
    outcome: Mutex<Outcome>,
}

impl Run<'_> {
    fn file(&self, r: &Rebuild) -> &File {
        &self.p.job.depots[r.depot].manifest.files[r.file]
    }

    fn depot_id(&self, r: &Rebuild) -> DepotId {
        self.p.job.depots[r.depot].manifest.depot
    }

    fn record(&self, r: &Rebuild, offset: u64) {
        let due = {
            let mut j = self.journal.lock().unwrap_or_else(|e| e.into_inner());
            j.0.staged.insert((self.depot_id(r), r.name, offset));
            j.0.downloaded = self.rep.downloaded.load(Ordering::Relaxed);
            if j.1.elapsed() >= JOURNAL_INTERVAL {
                j.1 = Instant::now();
                Some(j.0.encode())
            } else {
                None
            }
        };
        if let Some(bytes) = due {
            (self.rep.hooks.journal)(bytes);
        }
    }

    fn flush_journal(&self) {
        let bytes = {
            let mut j = self.journal.lock().unwrap_or_else(|e| e.into_inner());
            j.0.downloaded = self.rep.downloaded.load(Ordering::Relaxed);
            j.0.encode()
        };
        (self.rep.hooks.journal)(bytes);
    }

    /// Moves a complete file into place.
    fn finish_file(&self, r: &Rebuild) -> Result<(), EngineError> {
        let f = self.file(r);
        let d = &self.p.job.depots[r.depot];
        // Windows flushes only through a handle that may write.
        OpenOptions::new()
            .write(true)
            .open(&r.staged)
            .and_then(|h| h.sync_all())
            .map_err(io_err(&r.staged))?;
        apply_flags(&r.staged, f.flags).map_err(io_err(&r.staged))?;
        let parts = components(f);
        make_dirs(&d.install_dir, &parts, false)?;
        if fs::symlink_metadata(&r.dest).is_ok_and(|m| m.is_dir()) {
            return Err(EngineError::InTheWay(r.dest.clone()));
        }
        clear_read_only(&r.dest);
        fs::rename(&r.staged, &r.dest).map_err(io_err(&r.dest))?;
        {
            let mut j = self.journal.lock().unwrap_or_else(|e| e.into_inner());
            let depot = self.depot_id(r);
            for c in &f.chunks {
                j.0.staged.remove(&(depot, r.name, c.offset));
            }
        }
        self.rep.files_done.fetch_add(1, Ordering::Relaxed);
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .files_written += 1;
        self.rep.report(false);
        Ok(())
    }

    /// Step 1 for one file: creates it in staging and fills in every chunk
    /// found locally. Returns the indices of the chunks left to download.
    fn local(&self, r: &Rebuild, cancel: &CancelToken) -> Result<Vec<usize>, EngineError> {
        let f = self.file(r);
        let depot = self.depot_id(r);
        let dir = r
            .staged
            .parent()
            .expect("staged files are in a depot directory");
        fs::create_dir_all(dir).map_err(io_err(dir))?;
        let journaled = f
            .chunks
            .iter()
            .any(|c| self.p.journal.staged.contains(&(depot, r.name, c.offset)));
        let handle = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(!journaled)
            .open(&r.staged)
            .map_err(io_err(&r.staged))?;
        handle.set_len(f.size).map_err(io_err(&r.staged))?;
        drop(handle);

        let mut missing = Vec::new();
        for (ci, c) in f.chunks.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let len = c.original as usize;
            if self.p.journal.staged.contains(&(depot, r.name, c.offset))
                && read_at(&r.staged, c.offset, len)
                    .is_ok_and(|data| chunk::check_quick(c, &data).is_ok())
            {
                self.record(r, c.offset);
                self.rep
                    .bytes_done
                    .fetch_add(u64::from(c.original), Ordering::Relaxed);
                continue;
            }
            let found = self
                .p
                .local
                .get(&c.id)
                .into_iter()
                .flatten()
                .find_map(|(path, offset)| {
                    if !is_regular_file(path) {
                        return None;
                    }
                    read_at(path, *offset, len)
                        .ok()
                        .filter(|data| chunk::check_quick(c, data).is_ok())
                });
            match found {
                Some(data) => {
                    write_at(&r.staged, c.offset, &data).map_err(io_err(&r.staged))?;
                    self.record(r, c.offset);
                    self.rep
                        .bytes_done
                        .fetch_add(u64::from(c.original), Ordering::Relaxed);
                    self.outcome
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .chunks_copied += 1;
                }
                None => missing.push(ci),
            }
            self.rep.report(false);
        }
        Ok(missing)
    }

    /// Step 2 for one chunk.
    fn download(
        &self,
        dl: &Downloader,
        key: &DepotKey,
        r: &Rebuild,
        ci: usize,
        cancel: &CancelToken,
    ) -> Result<(), EngineError> {
        let c = &self.file(r).chunks[ci];
        let data = dl.chunk(self.depot_id(r), c, key, cancel)?;
        write_at(&r.staged, c.offset, &data).map_err(io_err(&r.staged))?;
        self.rep
            .downloaded
            .fetch_add(u64::from(c.compressed), Ordering::Relaxed);
        self.record(r, c.offset);
        self.rep
            .bytes_done
            .fetch_add(u64::from(c.original), Ordering::Relaxed);
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bytes_downloaded += u64::from(c.compressed);
        self.rep.report(false);
        Ok(())
    }

    fn make_link(&self, di: usize, fi: usize) -> Result<bool, EngineError> {
        let d = &self.p.job.depots[di];
        let f = &d.manifest.files[fi];
        let parts = components(f);
        let dest = join(&d.install_dir, &parts);
        let target = f
            .link_target
            .as_deref()
            .expect("links have targets once checked");
        make_dirs(&d.install_dir, &parts, false)?;
        match fs::symlink_metadata(&dest) {
            Ok(m) if m.file_type().is_symlink() => {
                if fs::read_link(&dest).is_ok_and(|t| t == Path::new(target)) {
                    return Ok(false);
                }
                remove_file(&dest).map_err(io_err(&dest))?;
            }
            Ok(m) if m.is_dir() => return Err(EngineError::InTheWay(dest)),
            Ok(_) => {
                remove_file(&dest).map_err(io_err(&dest))?;
            }
            Err(_) => {}
        }
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, &dest);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target.replace('/', "\\"), &dest);
        made.map_err(|e| EngineError::Symlink(dest, e))?;
        Ok(true)
    }
}

impl Prepared<'_> {
    /// Files the run writes.
    pub fn files_to_write(&self) -> usize {
        self.rebuilds.len()
    }

    /// Runs the job with `threads` downloads at once.
    pub fn run(
        self,
        dl: &Downloader,
        threads: usize,
        hooks: &Hooks<'_>,
        cancel: &CancelToken,
    ) -> Result<Outcome, EngineError> {
        let job = self.job;
        let run = Run {
            p: &self,
            rep: Reporter::new(hooks),
            journal: Mutex::new((self.journal.clone(), Instant::now())),
            remaining: self.rebuilds.iter().map(|_| AtomicUsize::new(0)).collect(),
            outcome: Mutex::new(Outcome::default()),
        };
        let total: u64 = self
            .rebuilds
            .iter()
            .map(|r| job.depots[r.depot].manifest.files[r.file].size)
            .sum();
        run.rep.bytes_total.store(total, Ordering::Relaxed);
        run.rep
            .files_total
            .store(self.rebuilds.len() as u64, Ordering::Relaxed);
        // A resumed job counts what its earlier runs downloaded too.
        run.rep
            .downloaded
            .store(self.journal.downloaded, Ordering::Relaxed);
        run.rep
            .download_total
            .store(self.journal.downloaded, Ordering::Relaxed);

        // Staging holds only what the journal knows of (`content.md` §9).
        let keep: HashSet<PathBuf> = self
            .rebuilds
            .iter()
            .filter(|r| {
                let depot = job.depots[r.depot].manifest.depot;
                self.journal
                    .staged
                    .iter()
                    .any(|(d, n, _)| *d == depot && *n == r.name)
            })
            .map(|r| r.staged.clone())
            .collect();
        clean_staging(&job.staging, &keep)?;

        for &(di, fi) in &self.dirs {
            let d = &job.depots[di];
            make_dirs(&d.install_dir, &components(&d.manifest.files[fi]), true)?;
        }
        for (path, file_flags) in &self.reflag {
            apply_flags(path, *file_flags).map_err(io_err(path))?;
        }

        // Step 1.
        run.rep.phase(Phase::Copying);
        let mut downloads = Vec::new();
        let mut ready = Vec::new();
        for (ri, r) in self.rebuilds.iter().enumerate() {
            let missing = run.local(r, cancel)?;
            if missing.is_empty() {
                ready.push(ri);
            }
            run.remaining[ri].store(missing.len(), Ordering::Relaxed);
            let f = &job.depots[r.depot].manifest.files[r.file];
            for ci in missing {
                run.rep
                    .download_total
                    .fetch_add(u64::from(f.chunks[ci].compressed), Ordering::Relaxed);
                downloads.push((ri, ci));
            }
        }
        run.flush_journal();
        for ri in ready {
            run.finish_file(&self.rebuilds[ri])?;
        }

        // Step 2.
        run.rep.phase(Phase::Downloading);
        if !downloads.is_empty() {
            // The journal is stored on failure too, so a resumed job keeps
            // what was downloaded, and a stopped job shows how far it got.
            let result = download_all(&run, dl, threads, downloads, cancel);
            run.flush_journal();
            if result.is_err() {
                run.rep.report(true);
            }
            result?;
        }

        // Step 3.
        run.rep.phase(Phase::Finishing);
        for &(di, fi) in &self.links {
            if run.make_link(di, fi)? {
                run.outcome.lock().unwrap_or_else(|e| e.into_inner()).links += 1;
            }
        }
        for path in &self.delete_files {
            if remove_file(path).map_err(io_err(path))? {
                run.outcome
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .files_deleted += 1;
            }
        }
        for path in &self.delete_dirs {
            // A directory still holding files the user put there is kept.
            if let Err(e) = fs::remove_dir(path)
                && e.kind() != io::ErrorKind::NotFound
            {
                tracing::debug!(path = %path.display(), error = %e, "directory kept");
            }
        }
        match fs::remove_dir_all(&job.staging) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&job.staging)(e)),
        }
        run.rep.report(true);
        Ok(run.outcome.into_inner().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Deletes everything under `staging` except `keep`.
fn clean_staging(staging: &Path, keep: &HashSet<PathBuf>) -> Result<(), EngineError> {
    let Ok(depots) = fs::read_dir(staging) else {
        return Ok(());
    };
    for entry in depots.flatten() {
        let dir = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_file(&dir).map_err(io_err(&dir))?;
            continue;
        }
        for file in fs::read_dir(&dir).map_err(io_err(&dir))?.flatten() {
            let path = file.path();
            if keep.contains(&path) {
                continue;
            }
            if file.file_type().is_ok_and(|t| t.is_dir()) {
                fs::remove_dir_all(&path).map_err(io_err(&path))?;
            } else {
                remove_file(&path).map_err(io_err(&path))?;
            }
        }
    }
    Ok(())
}

/// Step 2: a pool of `threads` workers takes chunks from a queue. The
/// first error stops the others.
fn download_all(
    run: &Run<'_>,
    dl: &Downloader,
    threads: usize,
    downloads: Vec<(usize, usize)>,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    let (tx, rx) = crossbeam_channel::unbounded();
    for d in downloads {
        tx.send(d).expect("the receiver is held here");
    }
    drop(tx);
    let stop = CancelToken::new();
    let failed: Mutex<Option<EngineError>> = Mutex::new(None);
    let done = AtomicBool::new(false);
    let fail = |e: EngineError| {
        let mut f = failed.lock().unwrap_or_else(|e| e.into_inner());
        if f.is_none() {
            *f = Some(e);
        }
        stop.cancel();
    };
    std::thread::scope(|s| {
        // Passes the caller's cancellation to the workers, whose requests
        // watch `stop` (CONC-2), and sends progress held back while the
        // workers wait.
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) && !stop.is_cancelled() {
                if cancel.is_cancelled() {
                    stop.cancel();
                }
                run.rep.flush();
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let workers: Vec<_> = (0..threads.max(1))
            .map(|_| {
                s.spawn(|| {
                    while let Ok((ri, ci)) = rx.recv() {
                        if stop.is_cancelled() {
                            return;
                        }
                        let r = &run.p.rebuilds[ri];
                        let key = &run.p.job.depots[r.depot].key;
                        let result = run.download(dl, key, r, ci, &stop).and_then(|()| {
                            if run.remaining[ri].fetch_sub(1, Ordering::AcqRel) == 1 {
                                run.finish_file(r)
                            } else {
                                Ok(())
                            }
                        });
                        if let Err(e) = result {
                            fail(e);
                            return;
                        }
                    }
                })
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
        done.store(true, Ordering::Relaxed);
    });
    if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
        // A worker that saw `stop` reports Cancelled; the caller's
        // cancellation is what matters then.
        return Err(match e {
            EngineError::Download(DownloadError::Cancelled) => EngineError::Cancelled,
            e => e,
        });
    }
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    Ok(())
}
