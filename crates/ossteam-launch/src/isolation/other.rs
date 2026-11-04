// SPDX-License-Identifier: GPL-2.0-or-later
//! No private `~/.steam` off Linux (`steamworks-abi.md` §1.2, §1.3).

use std::io;
use std::path::{Path, PathBuf};

use super::{Failure, Step};

/// Never made off Linux.
#[derive(Debug)]
pub enum Plan {}

impl Plan {
    pub fn new(_home: &Path, _data: &Path) -> Result<Self, Failure> {
        Err(unsupported())
    }

    pub fn mount_points(&self) -> impl Iterator<Item = &Path> {
        match *self {}
        #[allow(unreachable_code)]
        std::iter::empty()
    }
}

fn unsupported() -> Failure {
    Failure {
        step: Step::Unshare,
        path: None,
        source: io::ErrorKind::Unsupported.into(),
    }
}

pub fn check(_data: &Path) -> Result<(), Failure> {
    Err(unsupported())
}

pub fn official_roots(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

pub fn restrictions() -> Vec<String> {
    Vec::new()
}
