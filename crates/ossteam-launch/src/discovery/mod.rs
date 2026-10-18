// SPDX-License-Identifier: GPL-2.0-or-later
//! The entries that let a game's `steam_api` find ossteam's client
//! library (`steamworks-abi.md` §1, DISC-1 to DISC-3).

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Entries;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Entries;
#[cfg(not(any(target_os = "linux", windows)))]
mod other;
#[cfg(not(any(target_os = "linux", windows)))]
pub use other::Entries;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An official Steam client owns an entry (DISC-3).
    #[error(
        "{0} belongs to an official Steam client, so ossteam leaves it alone and cannot launch games; see docs/spec/steamworks-abi.md §1"
    )]
    Conflict(String),
    #[error("ossteam's client library is not installed: {} is missing", .0.display())]
    NoLibrary(PathBuf),
    #[error("ossteam has no discovery entries for this OS yet (M1e)")]
    Unsupported,
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    #[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
    pub(crate) fn io(what: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            what: what.into(),
            source,
        }
    }
}

/// ossteam's client libraries (`steamworks-abi.md` §1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Libraries {
    pub lib64: PathBuf,
    pub lib32: Option<PathBuf>,
}

#[cfg(windows)]
pub(crate) const LIBRARY: &str = "ossteam_steamclient.dll";
#[cfg(not(windows))]
pub(crate) const LIBRARY: &str = "libossteam_steamclient.so";

/// The directory the libraries are looked for in:
/// `OSSTEAM_STEAMCLIENT_DIR`, or the directory of this executable.
pub fn library_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("OSSTEAM_STEAMCLIENT_DIR") {
        return Some(PathBuf::from(d));
    }
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The libraries in `dir`. The 64-bit one is required.
pub fn find_libraries(dir: &Path) -> Result<Libraries, Error> {
    let lib64 = dir.join(LIBRARY);
    if !lib64.is_file() {
        return Err(Error::NoLibrary(lib64));
    }
    let lib32 = dir.join("lib32").join(LIBRARY);
    let lib64 = std::path::absolute(&lib64).unwrap_or(lib64);
    let lib32 = lib32
        .is_file()
        .then(|| std::path::absolute(&lib32).unwrap_or(lib32));
    Ok(Libraries { lib64, lib32 })
}
