// SPDX-License-Identifier: GPL-2.0-or-later
//! Where the daemon listens, and how both sides make sure that only the
//! same OS user is on the other end (SPEC IPC-1, IPC-2).
//!
//! | OS      | Endpoint                                   | Lock file                         |
//! |---------|--------------------------------------------|-----------------------------------|
//! | Linux   | `$XDG_RUNTIME_DIR/ossteam/daemon.sock`     | `$XDG_RUNTIME_DIR/ossteam/daemon.lock` |
//! | macOS   | `$TMPDIR/ossteam/daemon.sock`              | `$TMPDIR/ossteam/daemon.lock`     |
//! | Windows | `\\.\pipe\ossteam-<user SID>`              | `%LOCALAPPDATA%\ossteam\data\daemon.lock` |
//!
//! On Unix the directory is created with mode 0700 and must be owned by the
//! current user; clients refuse to connect through one that is not. On
//! Windows the pipe's DACL grants access to the user's SID only.
//!
//! Tests on Unix isolate a daemon through `XDG_RUNTIME_DIR` or `TMPDIR`.
//! Windows has no such variable, so debug builds there honour
//! [`TEST_ROOT_ENV`] instead (`ipc.md` §1).

use std::io;
use std::path::PathBuf;

use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream, prelude::*};

/// Windows, debug builds only: a directory that replaces the user's
/// endpoint and folders, so that tests can run a daemon beside a real one.
/// The pipe becomes `ossteam-test-<directory name>` and the lock file
/// `<directory>\data\daemon.lock`. Release builds ignore it.
pub const TEST_ROOT_ENV: &str = "OSSTEAM_TEST_ROOT";

/// [`TEST_ROOT_ENV`] when it applies: on Windows, in a debug build, set to
/// an absolute path.
pub fn test_root() -> Option<PathBuf> {
    if !cfg!(all(windows, debug_assertions)) {
        return None;
    }
    let root = PathBuf::from(std::env::var_os(TEST_ROOT_ENV)?);
    (root.is_absolute() && root.file_name().is_some()).then_some(root)
}

/// The client endpoint and the daemon's single-instance lock file.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The per-user runtime directory (Unix only).
    #[cfg(unix)]
    pub dir: PathBuf,
    /// The socket path (Unix) or pipe name (Windows, without the
    /// `\\.\pipe\` prefix).
    pub address: String,
    pub lock_file: PathBuf,
}

impl Endpoint {
    /// The endpoint leaf clients use.
    pub fn client() -> io::Result<Self> {
        imp::client_endpoint()
    }

    fn name(&self) -> io::Result<Name<'_>> {
        imp::name(self)
    }

    /// Connects to the daemon. On Unix the runtime directory's ownership and
    /// mode are checked first.
    pub fn connect(&self) -> io::Result<Stream> {
        imp::check_before_connect(self)?;
        Stream::connect(self.name()?)
    }

    /// Creates the directories the endpoint and lock file live in, with the
    /// required permissions, and checks them.
    pub fn ensure_dir(&self) -> io::Result<()> {
        imp::ensure_dir(self)
    }

    /// Creates the listener. The caller must hold the single-instance lock:
    /// on Unix a leftover socket file from a crashed daemon is removed.
    pub fn listen(&self) -> io::Result<Listener> {
        imp::listen(self)
    }
}

/// Checks that a connected peer runs as the same OS user as this process.
pub fn peer_is_same_user(stream: &Stream) -> io::Result<bool> {
    imp::peer_is_same_user(stream)
}

/// The peer's pid, or `None` where the OS reports none
/// (`steamworks-abi.md` §3.1): `SO_PEERCRED` on Linux, `LOCAL_PEERPID`
/// on macOS, `GetNamedPipeClientProcessId` on Windows.
pub fn peer_pid(stream: &Stream) -> io::Result<Option<u32>> {
    imp::peer_pid(stream)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use interprocess::local_socket::GenericFilePath;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    fn runtime_dir() -> io::Result<PathBuf> {
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
        Ok(base.join("ossteam"))
    }

    pub fn client_endpoint() -> io::Result<Endpoint> {
        let dir = runtime_dir()?;
        let address = dir.join("daemon.sock").to_string_lossy().into_owned();
        let lock_file = dir.join("daemon.lock");
        Ok(Endpoint {
            dir,
            address,
            lock_file,
        })
    }

    pub fn name(ep: &Endpoint) -> io::Result<Name<'_>> {
        ep.address.as_str().to_fs_name::<GenericFilePath>()
    }

    fn check_dir(ep: &Endpoint) -> io::Result<()> {
        let meta = fs::symlink_metadata(&ep.dir)?;
        let me = rustix::process::geteuid().as_raw();
        if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} must be a directory owned by uid {me} with mode 0700",
                    ep.dir.display()
                ),
            ));
        }
        Ok(())
    }

    pub fn check_before_connect(ep: &Endpoint) -> io::Result<()> {
        check_dir(ep)
    }

    /// Creates the runtime directory if needed and checks it.
    pub fn ensure_dir(ep: &Endpoint) -> io::Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(&ep.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        // Tighten a directory we own that was created with a wider mode.
        let meta = fs::symlink_metadata(&ep.dir)?;
        if meta.is_dir() && meta.uid() == rustix::process::geteuid().as_raw() {
            fs::set_permissions(&ep.dir, fs::Permissions::from_mode(0o700))?;
        }
        check_dir(ep)
    }

    pub fn listen(ep: &Endpoint) -> io::Result<Listener> {
        ensure_dir(ep)?;
        match fs::remove_file(&ep.address) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let opts = ListenerOptions::new().name(name(ep)?);
        #[cfg(target_os = "linux")]
        let opts = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;
            opts.mode(0o600)
        };
        let listener = opts.create_sync()?;
        fs::set_permissions(&ep.address, fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    pub fn peer_is_same_user(stream: &Stream) -> io::Result<bool> {
        let creds = stream.peer_creds()?;
        let me = rustix::process::geteuid().as_raw();
        Ok(creds.euid() == Some(me))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn peer_pid(stream: &Stream) -> io::Result<Option<u32>> {
        Ok(stream
            .peer_creds()?
            .pid()
            .and_then(|p| u32::try_from(p).ok()))
    }

    /// interprocess reads `LOCAL_PEERCRED` here, whose `xucred` has no pid
    /// on macOS, so the pid comes from `LOCAL_PEERPID`.
    #[cfg(target_os = "macos")]
    pub fn peer_pid(stream: &Stream) -> io::Result<Option<u32>> {
        use std::os::fd::{AsFd, AsRawFd};

        let Stream::UdSocket(socket) = stream;
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: `pid` and `len` are valid for writes of the sizes given.
        let r = unsafe {
            libc::getsockopt(
                socket.as_fd().as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&raw mut pid).cast(),
                &raw mut len,
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u32::try_from(pid).ok().filter(|&p| p != 0))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::os::windows as win;
    use interprocess::local_socket::GenericNamespaced;
    use interprocess::os::windows::local_socket::ListenerOptionsExt;
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    pub fn client_endpoint() -> io::Result<Endpoint> {
        if let Some(root) = test_root() {
            let name = root.file_name().expect("checked").to_string_lossy();
            return Ok(Endpoint {
                address: format!("ossteam-test-{name}"),
                lock_file: root.join("data").join("daemon.lock"),
            });
        }
        let sid = win::current_user_sid()?;
        let dirs = directories::ProjectDirs::from("", "", "ossteam").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no local application data directory",
            )
        })?;
        Ok(Endpoint {
            address: format!("ossteam-{sid}"),
            lock_file: dirs.data_local_dir().join("daemon.lock"),
        })
    }

    pub fn name(ep: &Endpoint) -> io::Result<Name<'_>> {
        ep.address.as_str().to_ns_name::<GenericNamespaced>()
    }

    pub fn check_before_connect(_ep: &Endpoint) -> io::Result<()> {
        Ok(())
    }

    pub fn ensure_dir(ep: &Endpoint) -> io::Result<()> {
        match ep.lock_file.parent() {
            Some(parent) => std::fs::create_dir_all(parent),
            None => Ok(()),
        }
    }

    pub fn listen(ep: &Endpoint) -> io::Result<Listener> {
        ensure_dir(ep)?;
        let sid = win::current_user_sid()?;
        // Protected DACL with one entry: generic-all for this user's SID.
        let sddl = format!("D:P(A;;GA;;;{sid})");
        let wide = widestring::U16CString::from_str(&sddl)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let sd = SecurityDescriptor::deserialize(&wide)?;
        ListenerOptions::new()
            .name(name(ep)?)
            .security_descriptor(sd)
            .create_sync()
    }

    pub fn peer_is_same_user(stream: &Stream) -> io::Result<bool> {
        let creds = stream.peer_creds()?;
        let pid = creds
            .pid()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "peer pid unavailable"))?;
        Ok(win::process_user_sid(pid)? == win::current_user_sid()?)
    }

    pub fn peer_pid(stream: &Stream) -> io::Result<Option<u32>> {
        Ok(stream.peer_creds()?.pid())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn unix_endpoint_round_trip_and_peer_check() {
        use super::*;
        use std::io::{Read, Write};

        let tmp = std::env::temp_dir().join(format!("ossteam-ipc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let ep = Endpoint {
            dir: tmp.clone(),
            address: tmp.join("t.sock").to_string_lossy().into_owned(),
            lock_file: tmp.join("t.lock"),
        };
        let listener = ep.listen().unwrap();
        let t = std::thread::spawn(move || {
            let mut s = listener.accept().unwrap();
            assert!(peer_is_same_user(&s).unwrap());
            let mut b = [0u8; 2];
            s.read_exact(&mut b).unwrap();
            b
        });
        let mut c = ep.connect().unwrap();
        c.write_all(b"ok").unwrap();
        assert_eq!(&t.join().unwrap(), b"ok");

        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ep.connect().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
