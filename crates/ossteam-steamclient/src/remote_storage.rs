// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamRemoteStorage` (`docs/spec/steamworks/isteamremotestorage.md`).

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use crate::abi::{EMPTY, intern, put, read_str};
use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe;
use crate::vtables::VERSIONS;

/// `k_unMaxCloudFileChunkSize`.
const MAX_WRITE: u64 = 100 * 1024 * 1024;
/// `k_cchFilenameMax`, less the NUL.
const MAX_NAME: usize = 259;
/// The end of a temporary file's name (§2).
const PART: &str = ".ossteam-part";
/// `k_UGCFileStreamHandleInvalid`.
const STREAM_INVALID: u64 = u64::MAX;

/// `EResult`s.
const OK: i32 = 1;
const FAIL: i32 = 2;

/// Where the app's files are and how much it may keep (§1).
#[derive(Clone)]
struct Cloud {
    root: PathBuf,
    quota: u64,
    max_files: u32,
}

/// A `FileWriteStreamOpen` stream.
struct Stream {
    cloud: Cloud,
    name: String,
    file: File,
    temp: PathBuf,
    size: u64,
}

#[derive(Default)]
struct State {
    /// What `FileReadAsync` read, by call.
    reads: HashMap<u64, Vec<u8>>,
    streams: HashMap<u64, Stream>,
    next_stream: u64,
    /// The listing `GetFileNameAndSize` reads, with the root it is of.
    listing: Option<(PathBuf, Vec<(String, u64)>)>,
    batch: bool,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
/// Numbers temporary files.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The app's cloud, while an account is active.
fn cloud(o: &Object) -> Option<Cloud> {
    let p = pipe::get(o.pipe)?;
    let i = p.info();
    if i.app.user_dir.is_empty() {
        log!(Warn, "remote storage needs an active account");
        return None;
    }
    Some(Cloud {
        root: Path::new(&i.app.user_dir).join("remote"),
        quota: i.app.cloud_quota,
        max_files: i.app.cloud_max_files,
    })
}

/// The name a game passes, made canonical, when §1 accepts it.
fn checked(name: *mut c_void) -> Option<String> {
    // SAFETY: the SDK declares a NUL-terminated string.
    let raw = unsafe { read_str(name) };
    let Some(n) = raw.filter(|n| !n.is_empty() && n.len() <= MAX_NAME) else {
        log!(Warn, "remote storage: refusing the file name {raw:?}");
        return None;
    };
    let n = n.replace('\\', "/");
    let bad = n.contains(':')
        || n.ends_with(PART)
        || n.split('/').any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        log!(Warn, "remote storage: refusing the file name {n:?}");
        return None;
    }
    Some(n)
}

/// The cloud and the checked name for a method on one file.
fn target(o: &Object, name: *mut c_void) -> Option<(Cloud, String)> {
    let n = checked(name)?;
    Some((cloud(o)?, n))
}

impl Cloud {
    fn path(&self, name: &str) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(name.split('/'));
        p
    }

    /// Whether `name` may become `size` bytes (§1).
    fn fits(&self, name: &str, size: u64) -> bool {
        if self.quota == 0 {
            log!(Warn, "remote storage: the app has no Steam Cloud quota");
            return false;
        }
        let files = list(&self.root);
        let others: u64 = files
            .iter()
            .filter(|(n, _)| n != name)
            .map(|(_, s)| s)
            .sum();
        if others.saturating_add(size) > self.quota {
            log!(
                Warn,
                "remote storage: {name} would exceed the quota of {} bytes",
                self.quota
            );
            return false;
        }
        let new = !files.iter().any(|(n, _)| n == name);
        if new && self.max_files != 0 && files.len() >= self.max_files as usize {
            log!(
                Warn,
                "remote storage: {name} would exceed the limit of {} files",
                self.max_files
            );
            return false;
        }
        true
    }

    /// A new temporary file next to `name`'s.
    fn temp(&self, name: &str) -> io::Result<(File, PathBuf)> {
        let path = self.path(name);
        let dir = path.parent().unwrap_or(&self.root);
        fs::create_dir_all(dir)?;
        let base = path.file_name().unwrap_or_default().to_string_lossy();
        let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".{base}.{}-{n}{PART}", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        Ok((file, temp))
    }
}

/// Every file under `root`, sorted by name, with its size.
fn list(root: &Path) -> Vec<(String, u64)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let (Ok(name), Ok(kind)) = (e.file_name().into_string(), e.file_type()) else {
                continue;
            };
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                walk(&e.path(), &full, out);
            } else if kind.is_file()
                && !name.ends_with(PART)
                && let Ok(m) = e.metadata()
            {
                out.push((full, m.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort();
    out
}

/// Replaces `path` with `temp`, which `file` wrote, when `written`.
fn replace(file: File, temp: &Path, path: &Path, written: io::Result<()>) -> bool {
    let done = written.and_then(|()| file.sync_all()).and_then(|()| {
        drop(file);
        fs::rename(temp, path)
    });
    match done {
        Ok(()) => true,
        Err(e) => {
            log!(
                Warn,
                "remote storage: writing {} failed: {e}",
                path.display()
            );
            let _ = fs::remove_file(temp);
            false
        }
    }
}

/// Writes a file: `None` when §1 refuses it, `Some(false)` when writing
/// failed.
fn write(cloud: &Cloud, name: &str, data: &[u8]) -> Option<bool> {
    if data.len() as u64 > MAX_WRITE || !cloud.fits(name, data.len() as u64) {
        return None;
    }
    let path = cloud.path(name);
    match cloud.temp(name) {
        Ok((mut file, temp)) => {
            let written = file.write_all(data);
            Some(replace(file, &temp, &path, written))
        }
        Err(e) => {
            log!(
                Warn,
                "remote storage: writing {} failed: {e}",
                path.display()
            );
            Some(false)
        }
    }
}

/// The `size` bytes a game passes at `p`, when there may be that many.
fn data<'a>(p: *mut c_void, size: i64) -> Option<&'a [u8]> {
    let size = u64::try_from(size).ok().filter(|&s| s <= MAX_WRITE)?;
    if p.is_null() {
        return None;
    }
    // SAFETY: the SDK declares a buffer of `size` bytes.
    Some(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), size as usize) })
}

/// Reads from `offset` until `buf` is full or the file ends.
fn read_at(path: &Path, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn file_write(o: &Object, name: *mut c_void, p: *mut c_void, size: i32) -> bool {
    let Some((cloud, name)) = target(o, name) else {
        return false;
    };
    let Some(bytes) = data(p, size.into()) else {
        return false;
    };
    write(&cloud, &name, bytes) == Some(true)
}

pub fn file_read(o: &Object, name: *mut c_void, buf: *mut c_void, size: i32) -> i32 {
    let Some((cloud, name)) = target(o, name) else {
        return 0;
    };
    if buf.is_null() || size <= 0 {
        return 0;
    }
    // SAFETY: the SDK declares a buffer of `size` bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(buf.cast::<u8>(), size as usize) };
    read_at(&cloud.path(&name), 0, out).map_or(0, |n| n as i32)
}

pub fn file_write_async(o: &Object, name: *mut c_void, p: *mut c_void, size: u32) -> u64 {
    let (Some(pipe), Some((cloud, name))) = (pipe::get(o.pipe), target(o, name)) else {
        return 0;
    };
    let Some(bytes) = data(p, size.into()) else {
        return 0;
    };
    let Some(written) = write(&cloud, &name, bytes) else {
        return 0;
    };
    let result = if written { OK } else { FAIL };
    pipe.local_call(callbacks::FILE_WRITE_ASYNC_COMPLETE, |_| {
        callbacks::file_write_async_complete(result)
    })
}

pub fn file_read_async(o: &Object, name: *mut c_void, offset: u32, size: u32) -> u64 {
    let (Some(pipe), Some((cloud, name))) = (pipe::get(o.pipe), target(o, name)) else {
        return 0;
    };
    let path = cloud.path(&name);
    let Ok(len) = fs::metadata(&path).map(|m| m.len()) else {
        log!(Warn, "remote storage: there is no file {name}");
        return 0;
    };
    if size == 0 || u64::from(offset) + u64::from(size) > len {
        log!(
            Warn,
            "remote storage: reading {size} bytes at {offset} is past the end of {name}"
        );
        return 0;
    }
    let mut buf = vec![0; size as usize];
    let (result, read) = match read_at(&path, offset.into(), &mut buf) {
        Ok(n) => (OK, n),
        Err(e) => {
            log!(Warn, "remote storage: reading {name} failed: {e}");
            (FAIL, 0)
        }
    };
    buf.truncate(read);
    pipe.local_call(callbacks::FILE_READ_ASYNC_COMPLETE, |call| {
        state().reads.insert(call, buf);
        callbacks::file_read_async_complete(call, result, offset, read as u32)
    })
}

pub fn file_read_async_complete(_: &Object, call: u64, buf: *mut c_void, size: u32) -> bool {
    if buf.is_null() {
        return false;
    }
    let Some(data) = state().reads.remove(&call) else {
        return false;
    };
    let n = data.len().min(size as usize);
    // SAFETY: the SDK declares a buffer of `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf.cast(), n) };
    true
}

pub fn file_exists(o: &Object, name: *mut c_void) -> bool {
    target(o, name).is_some_and(|(c, n)| c.path(&n).is_file())
}

/// `FileForget`: there is nothing in Steam Cloud to forget.
pub fn file_forget(o: &Object, name: *mut c_void) -> bool {
    file_exists(o, name)
}

pub fn file_persisted(_: &Object, _name: *mut c_void) -> bool {
    false
}

pub fn file_delete(o: &Object, name: *mut c_void) -> bool {
    target(o, name).is_some_and(|(c, n)| fs::remove_file(c.path(&n)).is_ok())
}

pub fn file_size(o: &Object, name: *mut c_void) -> i32 {
    target(o, name)
        .and_then(|(c, n)| fs::metadata(c.path(&n)).ok())
        .map_or(0, |m| m.len().min(i32::MAX as u64) as i32)
}

pub fn file_timestamp(o: &Object, name: *mut c_void) -> i64 {
    target(o, name)
        .and_then(|(c, n)| fs::metadata(c.path(&n)).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn stream_open(o: &Object, name: *mut c_void) -> u64 {
    let Some((cloud, name)) = target(o, name) else {
        return STREAM_INVALID;
    };
    let (file, temp) = match cloud.temp(&name) {
        Ok(t) => t,
        Err(e) => {
            log!(Warn, "remote storage: cannot start writing {name}: {e}");
            return STREAM_INVALID;
        }
    };
    let mut s = state();
    s.next_stream += 1;
    let handle = s.next_stream;
    s.streams.insert(
        handle,
        Stream {
            cloud,
            name,
            file,
            temp,
            size: 0,
        },
    );
    handle
}

pub fn stream_write_chunk(_: &Object, handle: u64, p: *mut c_void, size: i32) -> bool {
    let mut s = state();
    let Some(stream) = s.streams.get_mut(&handle) else {
        return false;
    };
    let Some(bytes) = data(p, size.into()) else {
        return false;
    };
    let total = stream.size + bytes.len() as u64;
    if total > MAX_WRITE || !stream.cloud.fits(&stream.name, total) {
        return false;
    }
    match stream.file.write_all(bytes) {
        Ok(()) => {
            stream.size = total;
            true
        }
        Err(e) => {
            log!(Warn, "remote storage: writing {} failed: {e}", stream.name);
            false
        }
    }
}

pub fn stream_close(_: &Object, handle: u64) -> bool {
    let Some(stream) = state().streams.remove(&handle) else {
        return false;
    };
    if !stream.cloud.fits(&stream.name, stream.size) {
        drop(stream.file);
        let _ = fs::remove_file(&stream.temp);
        return false;
    }
    let path = stream.cloud.path(&stream.name);
    replace(stream.file, &stream.temp, &path, Ok(()))
}

pub fn stream_cancel(_: &Object, handle: u64) -> bool {
    let Some(stream) = state().streams.remove(&handle) else {
        return false;
    };
    drop(stream.file);
    let _ = fs::remove_file(&stream.temp);
    true
}

pub fn file_count(o: &Object) -> i32 {
    let Some(cloud) = cloud(o) else {
        return 0;
    };
    let files = list(&cloud.root);
    let n = files.len().min(i32::MAX as usize) as i32;
    state().listing = Some((cloud.root, files));
    n
}

pub fn file_name_and_size(o: &Object, i: i32, size: *mut c_void) -> *const c_char {
    let Some(cloud) = cloud(o) else {
        return EMPTY.as_ptr();
    };
    let mut s = state();
    if s.listing
        .as_ref()
        .is_none_or(|(root, _)| *root != cloud.root)
    {
        let files = list(&cloud.root);
        s.listing = Some((cloud.root, files));
    }
    let found = s
        .listing
        .as_ref()
        .and_then(|(_, files)| files.get(usize::try_from(i).ok()?));
    let Some((name, len)) = found else {
        return EMPTY.as_ptr();
    };
    // SAFETY: the SDK declares an `int32 *`.
    unsafe { put(size, (*len).min(i32::MAX as u64) as i32) };
    intern(name)
}

/// `GetQuota` writes `int32`s up to version 013 and `uint64`s from 014.
fn quota_is_wide(version: usize) -> bool {
    let name = VERSIONS[version].name.to_string_lossy();
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    digits.parse::<u32>().is_ok_and(|v| v >= 14)
}

pub fn quota(o: &Object, total: *mut c_void, available: *mut c_void) -> bool {
    let Some(cloud) = cloud(o) else {
        return false;
    };
    let used: u64 = list(&cloud.root).iter().map(|(_, s)| s).sum();
    let left = cloud.quota.saturating_sub(used);
    // SAFETY: the SDK declares two `uint64 *` from version 014 and two
    // `int32 *` before.
    unsafe {
        if quota_is_wide(o.version) {
            put(total, cloud.quota);
            put(available, left);
        } else {
            let narrow = |n: u64| n.min(i32::MAX as u64) as i32;
            put(total, narrow(cloud.quota));
            put(available, narrow(left));
        }
    }
    true
}

/// No Steam Cloud until M3, for the account or any app.
pub fn cloud_enabled(_: &Object) -> bool {
    false
}

/// Nothing is synced, so no file ever changes on its own.
pub fn local_file_change_count(_: &Object) -> i32 {
    0
}

pub fn local_file_change(
    _: &Object,
    _i: i32,
    change: *mut c_void,
    path_type: *mut c_void,
) -> *const c_char {
    // SAFETY: the SDK declares two enum pointers, 4 bytes each; 0 is
    // `Invalid` in both.
    unsafe {
        put(change, 0u32);
        put(path_type, 0u32);
    }
    EMPTY.as_ptr()
}

pub fn begin_file_write_batch(_: &Object) -> bool {
    !std::mem::replace(&mut state().batch, true)
}

pub fn end_file_write_batch(_: &Object) -> bool {
    std::mem::replace(&mut state().batch, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Option<String> {
        let c = std::ffi::CString::new(s).unwrap();
        checked(c.as_ptr().cast_mut().cast())
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(name("save.dat").as_deref(), Some("save.dat"));
        assert_eq!(name("a\\b/c.txt").as_deref(), Some("a/b/c.txt"));
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/",
            "./a",
            "a/../b",
            "..",
            "C:x",
            "x.ossteam-part",
        ] {
            assert_eq!(name(bad), None, "{bad:?}");
        }
        assert!(name(&"x".repeat(259)).is_some());
        assert_eq!(name(&"x".repeat(260)), None);
        assert_eq!(checked(std::ptr::null_mut()), None);
    }

    #[test]
    fn quota_width_follows_the_version() {
        let find = |n: &str| crate::abi::find_version(n.as_bytes()).unwrap();
        assert!(!quota_is_wide(find(
            "STEAMREMOTESTORAGE_INTERFACE_VERSION013"
        )));
        assert!(quota_is_wide(find(
            "STEAMREMOTESTORAGE_INTERFACE_VERSION014"
        )));
        assert!(quota_is_wide(find(
            "STEAMREMOTESTORAGE_INTERFACE_VERSION016"
        )));
        assert!(!quota_is_wide(find(
            "STEAMREMOTESTORAGE_INTERFACE_VERSION001"
        )));
    }

    #[test]
    fn writes_respect_the_quota() {
        let dir = std::env::temp_dir().join(format!("ossteam-rs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let cloud = Cloud {
            root: dir.clone(),
            quota: 10,
            max_files: 2,
        };
        assert_eq!(write(&cloud, "a/one", b"12345"), Some(true));
        assert_eq!(write(&cloud, "two", b"123456"), None);
        assert_eq!(write(&cloud, "two", b"12345"), Some(true));
        // Replacing a file counts its new size only.
        assert_eq!(write(&cloud, "two", b"54321"), Some(true));
        assert_eq!(write(&cloud, "three", b""), None);
        assert_eq!(list(&dir), [("a/one".to_owned(), 5), ("two".to_owned(), 5)]);
        let mut buf = [0; 3];
        assert_eq!(read_at(&cloud.path("two"), 1, &mut buf).unwrap(), 3);
        assert_eq!(&buf, b"432");
        let none = Cloud { quota: 0, ..cloud };
        assert_eq!(write(&none, "two", b"1"), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
