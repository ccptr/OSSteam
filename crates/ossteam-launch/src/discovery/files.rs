// SPDX-License-Identifier: GPL-2.0-or-later
//! The files the Unix entries are made of (`steamworks-abi.md` §1.1,
//! §1.3.3).

use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use super::Error;

pub(super) fn make_dir(dir: &Path, mode: u32) -> Result<(), Error> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(dir)
        .map_err(|e| Error::io(format!("creating {}", dir.display()), e))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn remove(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
    }
}

/// Makes `dest` a copy of `lib`, unless it is one already. On Linux a
/// container sees the home directory but not ossteam's install directory,
/// so a link would not do (`compat-tools.md` §9); on macOS `steam_api`
/// wants the name `steamclient.dylib`. The new copy is renamed over the
/// old file, which a running game keeps mapped.
pub(super) fn replace_copy(lib: &Path, dest: &Path) -> Result<(), Error> {
    let want =
        std::fs::read(lib).map_err(|e| Error::io(format!("reading {}", lib.display()), e))?;
    let is_copy = dest
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_file() && m.len() == want.len() as u64)
        && std::fs::read(dest).is_ok_and(|have| have == want);
    if is_copy {
        return Ok(());
    }
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dest.with_file_name(format!(".{name}.ossteam"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(&want)?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, dest));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("writing {}", dest.display()), e)
    })
}
