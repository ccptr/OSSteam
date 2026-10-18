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
    #[allow(unused_mut)]
    let mut addr = Address::in_dir(base.join("ossteam"));
    #[cfg(target_os = "linux")]
    {
        addr.abstract_name = Some(super::abstract_name(euid()));
    }
    Ok(addr)
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

pub fn connect(addr: &Address) -> io::Result<Connection> {
    #[cfg(target_os = "linux")]
    if let Some(name) = &addr.abstract_name
        && fs::symlink_metadata(&addr.name).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
    {
        return connect_abstract(name);
    }
    let meta = fs::symlink_metadata(&addr.dir)?;
    let me = euid();
    if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} must be a directory owned by uid {me} with mode 0700",
                addr.dir.display()
            ),
        ));
    }
    connection(UnixStream::connect(&addr.name)?)
}

/// The abstract socket has no permissions: the listener must run as this
/// user (`steamworks-abi.md` §3.1).
#[cfg(target_os = "linux")]
fn connect_abstract(name: &str) -> io::Result<Connection> {
    use std::os::fd::AsRawFd;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::SocketAddr;

    let stream = UnixStream::connect_addr(&SocketAddr::from_abstract_name(name.as_bytes())?)?;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes of the sizes given.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    let me = euid();
    if cred.uid != me {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the abstract socket {name} is held by uid {}, not uid {me}",
                cred.uid
            ),
        ));
    }
    connection(stream)
}

fn connection(stream: UnixStream) -> io::Result<Connection> {
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
