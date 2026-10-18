// SPDX-License-Identifier: GPL-2.0-or-later
//! Unix sockets in the per-user runtime directory.

use std::fs;
use std::io;
use std::net::Shutdown;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::{Address, Connection};

pub fn locate() -> io::Result<Address> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "macos"))]
    let base = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is not set to an absolute path",
            ));
        }
    };
    Ok(Address::in_dir(base.join("ossteam")))
}

pub fn connect(addr: &Address) -> io::Result<Connection> {
    let meta = fs::symlink_metadata(&addr.dir)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} must be a directory owned by uid {me} with mode 0700",
                addr.dir.display()
            ),
        ));
    }
    let stream = UnixStream::connect(&addr.name)?;
    let reader = stream.try_clone()?;
    let closer = stream.try_clone()?;
    Ok(Connection {
        reader: Box::new(reader),
        writer: Box::new(stream),
        closer: Box::new(move || {
            let _ = closer.shutdown(Shutdown::Both);
        }),
    })
}
