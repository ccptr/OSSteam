// SPDX-License-Identifier: GPL-2.0-or-later
//! What the process asked for that the shim does not provide, sent to
//! the daemon as `Report`s (`steamworks-abi.md` §4, ABI-5).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ossteam_gameipc::{Call, Report};

use crate::log::log;
use crate::object::Object;
use crate::vtables::{SLOT_COUNT, SLOTS, VERSIONS};

/// The exports that are not implemented (`steamworks-abi.md` §2.1).
pub const EXPORTS: [&str; 5] = [
    "Breakpad_SteamMiniDumpInit",
    "Breakpad_SteamSetAppID",
    "Breakpad_SteamSetSteamID",
    "Breakpad_SteamWriteMiniDumpSetComment",
    "Breakpad_SteamWriteMiniDumpUsingExceptionInfoWithBuildId",
];

/// Reports are sent at most this often, unless something is new.
const INTERVAL: Duration = Duration::from_secs(1);

static SLOT_CALLS: [AtomicU32; SLOT_COUNT] = [const { AtomicU32::new(0) }; SLOT_COUNT];
static EXPORT_CALLS: [AtomicU32; EXPORTS.len()] = [const { AtomicU32::new(0) }; EXPORTS.len()];
/// Something was counted since the last report.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// Something was recorded for the first time since the last report.
static NEW: AtomicBool = AtomicBool::new(false);

struct State {
    /// Versions handed out, in order, and how many were reported.
    interfaces: Vec<usize>,
    interfaces_sent: usize,
    unknown: Vec<String>,
    unknown_sent: usize,
    /// Counts already reported.
    slots_sent: Vec<u32>,
    exports_sent: [u32; EXPORTS.len()],
    last: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    interfaces: Vec::new(),
    interfaces_sent: 0,
    unknown: Vec::new(),
    unknown_sent: 0,
    slots_sent: Vec::new(),
    exports_sent: [0; EXPORTS.len()],
    last: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A stub ran (§2.4). The first call of each slot is logged and reported
/// right away.
pub fn stub(slot: usize, _this: *mut Object) {
    let before = SLOT_CALLS[slot].fetch_add(1, Ordering::Relaxed);
    DIRTY.store(true, Ordering::Release);
    if before == 0 {
        let s = &SLOTS[slot];
        log!(
            Info,
            "{}::{} is not implemented; it returns its failure value",
            VERSIONS[s.version].name.to_string_lossy(),
            s.method
        );
        NEW.store(true, Ordering::Release);
        crate::pipe::send_report(true);
    }
}

/// An unimplemented export ran.
pub fn export(index: usize) {
    let before = EXPORT_CALLS[index].fetch_add(1, Ordering::Relaxed);
    DIRTY.store(true, Ordering::Release);
    if before == 0 {
        log!(Info, "{} is not implemented", EXPORTS[index]);
        NEW.store(true, Ordering::Release);
        crate::pipe::send_report(true);
    }
}

/// Records that `version` was handed out. Returns whether it had been.
pub fn handed_out(version: usize) -> bool {
    let mut s = state();
    if s.interfaces.contains(&version) {
        return true;
    }
    s.interfaces.push(version);
    NEW.store(true, Ordering::Release);
    DIRTY.store(true, Ordering::Release);
    false
}

/// Records a version the shim does not have.
pub fn unknown(version: &str) {
    {
        let mut s = state();
        if s.unknown.iter().any(|u| u == version) {
            return;
        }
        s.unknown.push(version.to_owned());
    }
    log!(Warn, "interface version {version} is not provided");
    NEW.store(true, Ordering::Release);
    DIRTY.store(true, Ordering::Release);
    crate::pipe::send_report(true);
}

/// Whether a report is due: something new, or counts older than a
/// second (or `force`).
pub fn due(force: bool) -> bool {
    if NEW.load(Ordering::Acquire) {
        return true;
    }
    if !DIRTY.load(Ordering::Acquire) {
        return false;
    }
    force || state().last.is_none_or(|t| t.elapsed() >= INTERVAL)
}

/// What changed since the last report, marked as reported. `None` when
/// nothing did.
pub fn take() -> Option<Report> {
    let mut s = state();
    NEW.store(false, Ordering::Release);
    DIRTY.store(false, Ordering::Release);
    s.last = Some(Instant::now());
    if s.slots_sent.is_empty() {
        s.slots_sent = vec![0; SLOT_COUNT];
    }
    let interfaces = s.interfaces[s.interfaces_sent..]
        .iter()
        .map(|&v| VERSIONS[v].name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    s.interfaces_sent = s.interfaces.len();
    let unknown = s.unknown[s.unknown_sent..].to_vec();
    s.unknown_sent = s.unknown.len();
    let mut calls = Vec::new();
    for (i, c) in SLOT_CALLS.iter().enumerate() {
        let now = c.load(Ordering::Relaxed);
        if now != s.slots_sent[i] {
            let slot = &SLOTS[i];
            calls.push(Call {
                interface: VERSIONS[slot.version].name.to_string_lossy().into_owned(),
                method: slot.method.to_owned(),
                count: now - s.slots_sent[i],
            });
            s.slots_sent[i] = now;
        }
    }
    for (i, c) in EXPORT_CALLS.iter().enumerate() {
        let now = c.load(Ordering::Relaxed);
        if now != s.exports_sent[i] {
            calls.push(Call {
                interface: "exports".into(),
                method: EXPORTS[i].to_owned(),
                count: now - s.exports_sent[i],
            });
            s.exports_sent[i] = now;
        }
    }
    if interfaces.is_empty() && unknown.is_empty() && calls.is_empty() {
        return None;
    }
    Some(Report {
        interfaces,
        unknown,
        calls,
    })
}

/// Puts a report that could not be sent back, so the next one carries it.
pub fn restore(r: Report) {
    let mut s = state();
    for name in r.interfaces.iter().rev() {
        if let Some(v) = crate::abi::find_version(name.as_bytes())
            && let Some(pos) = s.interfaces.iter().position(|&i| i == v)
        {
            s.interfaces_sent = s.interfaces_sent.min(pos);
        }
    }
    s.unknown_sent -= r.unknown.len().min(s.unknown_sent);
    for c in r.calls {
        if c.interface == "exports" {
            if let Some(i) = EXPORTS.iter().position(|e| *e == c.method) {
                s.exports_sent[i] = s.exports_sent[i].saturating_sub(c.count);
            }
        } else if let Some(v) = crate::abi::find_version(c.interface.as_bytes()) {
            // Overloads share a name; the count goes back to the first,
            // which only shifts it between them.
            let first = VERSIONS[v].first_slot;
            let n = VERSIONS[v].vtable.len();
            if let Some(i) = (first..first + n).find(|&i| SLOTS[i].method == c.method) {
                s.slots_sent[i] = s.slots_sent[i].saturating_sub(c.count);
            }
        }
    }
    NEW.store(true, Ordering::Release);
    DIRTY.store(true, Ordering::Release);
}
