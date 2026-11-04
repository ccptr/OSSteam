// SPDX-License-Identifier: GPL-2.0-or-later
//! A private `~/.steam` for each launch (`steamworks-abi.md` §1.1.2,
//! DISC-4): on Linux, every process the daemon starts for a launch runs in
//! a user and mount namespace of its own, in which `<data>/steam-sdk` is
//! mounted over `~/.steam`, and an official installation's `userdata/`
//! and `config/` are covered. Elsewhere there is none.

use std::fmt;
use std::io;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::attach;
#[cfg(target_os = "linux")]
pub use linux::{Plan, check, official_roots, restrictions};

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
pub use other::{Plan, check, official_roots, restrictions};

/// The steps of `steamworks-abi.md` §1.1.2, with what comes before them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    /// Making the mount points and directories, before the fork.
    Prepare = 0,
    Unshare = 1,
    Setgroups = 2,
    UidMap = 3,
    GidMap = 4,
    Private = 5,
    Steam = 6,
    Userdata = 7,
    Config = 8,
}

impl Step {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Prepare,
            1 => Self::Unshare,
            2 => Self::Setgroups,
            3 => Self::UidMap,
            4 => Self::GidMap,
            5 => Self::Private,
            6 => Self::Steam,
            7 => Self::Userdata,
            8 => Self::Config,
            _ => return None,
        })
    }

    /// The step's number in §1.1.2; none for [`Step::Prepare`].
    pub fn number(self) -> Option<u8> {
        match self {
            Self::Prepare => None,
            Self::Unshare => Some(1),
            Self::Setgroups | Self::UidMap | Self::GidMap => Some(2),
            Self::Private => Some(3),
            Self::Steam => Some(4),
            Self::Userdata => Some(5),
            Self::Config => Some(6),
        }
    }

    fn what(self) -> &'static str {
        match self {
            Self::Prepare => "preparing the private ~/.steam",
            Self::Unshare => "creating a user and mount namespace",
            Self::Setgroups => "writing /proc/self/setgroups",
            Self::UidMap => "writing /proc/self/uid_map",
            Self::GidMap => "writing /proc/self/gid_map",
            Self::Private => "making the mounts private",
            Self::Steam => "mounting the private ~/.steam read-only",
            Self::Userdata => "mounting ossteam's userdata over the official one",
            Self::Config => "mounting an empty directory over the official config",
        }
    }
}

/// A step that failed, and on what.
#[derive(Debug)]
pub struct Failure {
    pub step: Step,
    /// The mount point, for steps 4 to 6 and for preparing.
    pub path: Option<PathBuf>,
    pub source: io::Error,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step.number() {
            Some(n) => write!(f, "step {n} ({})", self.step.what())?,
            None => f.write_str(self.step.what())?,
        }
        if let Some(p) = &self.path {
            write!(f, " at {}", p.display())?;
        }
        write!(f, " failed: {}", self.source)
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
