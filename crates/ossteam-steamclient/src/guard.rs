// SPDX-License-Identifier: GPL-2.0-or-later
//! Panics never cross into the game (ABI-4).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::log::log;

/// Runs an implemented slot. A panic is logged and becomes the slot's
/// failure value.
pub fn run<R>(slot: usize, f: impl FnOnce() -> R, fail: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            let s = &crate::vtables::SLOTS[slot];
            let v = &crate::vtables::VERSIONS[s.version];
            log!(
                Warn,
                "{}::{} panicked; it returns its failure value",
                v.name.to_string_lossy(),
                s.method
            );
            fail()
        }
    }
}

/// Runs an export, as [`run`] does.
pub fn export<R>(name: &str, f: impl FnOnce() -> R, fail: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            log!(Warn, "{name} panicked; it returns its failure value");
            fail()
        }
    }
}
