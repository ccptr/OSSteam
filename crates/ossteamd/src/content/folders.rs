// SPDX-License-Identifier: GPL-2.0-or-later
//! Library folders on disk: the `library.toml` marker, the layout inside
//! a folder, and free space (`ipc.md` §5.5, `content.md` §8, SPEC §17).

use std::io;
use std::path::{Component, Path, PathBuf};

use ossteam_types::{AppId, DepotId, ManifestId};
use serde::{Deserialize, Serialize};

/// The marker's path inside a folder.
pub const MARKER: &str = ".ossteam/library.toml";

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    version: u32,
    id: String,
}

/// A new folder id: 128 random bits in hex.
pub fn new_id() -> String {
    use rand_core::RngCore;
    let mut b = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The id in a folder's marker; `None` when it has none.
pub fn read_id(folder: &Path) -> io::Result<Option<String>> {
    let text = match std::fs::read_to_string(folder.join(MARKER)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m: Marker = toml::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{MARKER}: {e}")))?;
    if m.id.len() != 32 || !m.id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{MARKER}: {:?} is not a folder id", m.id),
        ));
    }
    Ok(Some(m.id.to_ascii_lowercase()))
}

/// Creates the folder and its marker if needed, and returns its id.
pub fn ensure(folder: &Path) -> io::Result<String> {
    if let Some(id) = read_id(folder)? {
        return Ok(id);
    }
    let id = new_id();
    let marker = folder.join(MARKER);
    std::fs::create_dir_all(marker.parent().expect("the marker is in a directory"))?;
    let text = toml::to_string(&Marker {
        version: 1,
        id: id.clone(),
    })
    .expect("the marker serialises");
    let tmp = marker.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &marker)?;
    Ok(id)
}

/// Whether one path is the other or contains it, comparing components.
pub fn overlap(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> Vec<Component<'_>> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect()
    }
    let (a, b) = (norm(a), norm(b));
    let n = a.len().min(b.len());
    a[..n] == b[..n]
}

/// Free bytes on the file system that holds `path`, for this user.
#[cfg(unix)]
pub fn free_space(path: &Path) -> io::Result<u64> {
    let s = rustix::fs::statvfs(path)?;
    Ok(s.f_bavail.saturating_mul(s.f_frsize))
}

#[cfg(windows)]
pub fn free_space(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out
    // pointers are valid or null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

/// Where things are inside one folder (`content.md` §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `common/<installdir>`; `install_dir` must be checked first.
    pub fn app_dir(&self, install_dir: &str) -> PathBuf {
        self.root.join("common").join(install_dir)
    }

    pub fn manifests(&self) -> PathBuf {
        self.root.join(".ossteam").join("manifests")
    }

    pub fn manifest(&self, depot: DepotId, manifest: ManifestId) -> PathBuf {
        self.manifests()
            .join(format!("{}_{}.manifest", depot.0, manifest.0))
    }

    pub fn staging(&self, app: AppId) -> PathBuf {
        self.root
            .join(".ossteam")
            .join("staging")
            .join(app.0.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_created_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("ossteamd-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_id(&dir).unwrap(), None);
        let id = ensure(&dir).unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(ensure(&dir).unwrap(), id);
        assert_eq!(read_id(&dir).unwrap(), Some(id));
        assert!(free_space(&dir).unwrap() > 0);
        std::fs::write(dir.join(MARKER), "version = 1\nid = \"../x\"\n").unwrap();
        assert!(read_id(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlapping_paths() {
        assert!(overlap(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(overlap(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(overlap(Path::new("/a/b"), Path::new("/a/./b")));
        assert!(!overlap(Path::new("/a/b"), Path::new("/a/bc")));
    }

    #[test]
    fn layout() {
        let l = Layout::new("/lib");
        assert_eq!(l.app_dir("Spacewar"), Path::new("/lib/common/Spacewar"));
        assert_eq!(
            l.manifest(DepotId(481), ManifestId(5)),
            Path::new("/lib/.ossteam/manifests/481_5.manifest")
        );
        assert_eq!(
            l.staging(AppId(480)),
            Path::new("/lib/.ossteam/staging/480")
        );
    }
}
