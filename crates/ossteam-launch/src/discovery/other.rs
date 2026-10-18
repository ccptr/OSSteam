// SPDX-License-Identifier: GPL-2.0-or-later
//! macOS and other systems: the entries are established in M1e, so every
//! operation is honestly unsupported.

use std::path::Path;

use super::{Error, Libraries};

pub struct Entries;

impl Entries {
    pub fn new(_home: &Path, _data: &Path) -> Self {
        Self
    }

    pub fn check(&self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    pub fn create(&self, _libs: &Libraries, _pid: u32, _account_id: u32) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    pub fn remove(&self) -> Result<(), Error> {
        Ok(())
    }

    pub fn remove_stale(&self, _own_pid: u32) -> Result<(), Error> {
        Ok(())
    }
}
