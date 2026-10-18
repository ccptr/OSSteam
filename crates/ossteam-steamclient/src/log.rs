// SPDX-License-Identifier: GPL-2.0-or-later
//! The shim's log on standard error (`steamworks-abi.md` §2.6).

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn,
    Info,
    Debug,
}

/// The most detailed level that is written: `OSSTEAM_SHIM_LOG`, or info.
pub fn enabled(level: Level) -> bool {
    static MAX: OnceLock<Level> = OnceLock::new();
    let max = *MAX.get_or_init(|| match std::env::var("OSSTEAM_SHIM_LOG").as_deref() {
        Ok("debug") => Level::Debug,
        Ok("warn") => Level::Warn,
        _ => Level::Info,
    });
    level <= max
}

macro_rules! log {
    ($level:ident, $($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::$level) {
            eprintln!("[ossteam-steamclient] {}", format_args!($($arg)*));
        }
    };
}
pub(crate) use log;
