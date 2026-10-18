// SPDX-License-Identifier: GPL-2.0-or-later
//! The game endpoint (`steamworks-abi.md` §3.1): where it is, and the
//! shim's side of connecting to it.
//!
//! The daemon listens with the same address through its own local socket
//! library. The shim connects with `std` and OS bindings only (WS-2).

use std::io::{self, Read, Write};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

/// The game endpoint's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// The per-user runtime directory that holds the socket (Unix only).
    #[cfg(unix)]
    pub dir: std::path::PathBuf,
    /// The socket path (Unix) or the pipe name without `\\.\pipe\`
    /// (Windows).
    pub name: String,
    /// Linux: the abstract socket the daemon also listens on, for games
    /// in a container that has its own runtime directory
    /// (`compat-tools.md` §9).
    #[cfg(target_os = "linux")]
    pub abstract_name: Option<String>,
}

impl Address {
    /// The address for this user (`steamworks-abi.md` §3.1).
    pub fn locate() -> io::Result<Self> {
        os::locate()
    }

    /// The socket in `dir`, which must be the runtime directory's
    /// equivalent: for tests and for the daemon's own checks.
    #[cfg(unix)]
    pub fn in_dir(dir: impl Into<std::path::PathBuf>) -> Self {
        let dir = dir.into();
        let name = dir.join("game.sock").to_string_lossy().into_owned();
        Self {
            dir,
            name,
            #[cfg(target_os = "linux")]
            abstract_name: None,
        }
    }
}

/// Linux: the abstract socket name for user `uid`, `ossteam-game-<uid>`.
#[cfg(target_os = "linux")]
pub fn abstract_name(uid: u32) -> String {
    format!("ossteam-game-{uid}")
}

/// The two directions of one connection. Each may be used from its own
/// thread at the same time.
pub struct Connection {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Ends the connection in both directions. A read that is blocked, or
    /// starts later, returns the end of the stream or an error; the peer
    /// sees the connection close once both halves are dropped.
    pub closer: Box<dyn Fn() + Send + Sync>,
}

/// Connects to the daemon, after checking that the endpoint belongs to
/// this user: the directory's owner and mode on Unix, the serving
/// process's user on Windows. On Linux, when the socket file is not
/// there, the abstract socket is tried, and its listener's user checked.
pub fn connect(addr: &Address) -> io::Result<Connection> {
    os::connect(addr)
}
