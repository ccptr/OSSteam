// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamUtils` (`docs/spec/steamworks/isteamutils.md`).

use std::ffi::{c_char, c_void};

use crate::abi::{EMPTY, intern, put};
use crate::object::Object;
use crate::pipe;

/// `k_unBatteryPowerAC`: on mains power, or no battery.
const ON_MAINS: u8 = 255;

pub fn app_id(o: &Object) -> u32 {
    pipe::get(o.pipe).map_or_else(pipe::claimed_app_id, |p| p.info().app.app_id)
}

pub fn connected_universe(o: &Object) -> u32 {
    pipe::get(o.pipe).map_or(0, |p| {
        let i = p.info();
        if i.session.active {
            i.session.universe
        } else {
            0
        }
    })
}

pub fn server_real_time(o: &Object) -> u32 {
    pipe::get(o.pipe).map_or(0, |p| p.info().server_time())
}

pub fn ip_country(o: &Object) -> *const c_char {
    pipe::get(o.pipe).map_or(EMPTY.as_ptr(), |p| intern(&p.info().session.ip_country))
}

pub fn ui_language(o: &Object) -> *const c_char {
    pipe::get(o.pipe).map_or(EMPTY.as_ptr(), |p| intern(&p.info().session.ui_language))
}

pub fn battery_power(_: &Object) -> u8 {
    battery().unwrap_or(ON_MAINS)
}

/// The charge in percent while running on a battery.
#[cfg(target_os = "linux")]
fn battery() -> Option<u8> {
    let read = |p: &std::path::Path, f: &str| {
        std::fs::read_to_string(p.join(f))
            .ok()
            .map(|s| s.trim().to_owned())
    };
    let mut charges = Vec::new();
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = entry.path();
        match read(&p, "type").as_deref() {
            Some("Mains" | "USB") if read(&p, "online").as_deref() == Some("1") => return None,
            Some("Battery") if read(&p, "scope").as_deref() != Some("Device") => {
                if read(&p, "status").as_deref() == Some("Discharging")
                    && let Some(c) = read(&p, "capacity").and_then(|c| c.parse::<u8>().ok())
                {
                    charges.push(c.min(100));
                }
            }
            _ => {}
        }
    }
    charges.into_iter().min()
}

#[cfg(windows)]
fn battery() -> Option<u8> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: plain data; zero is a valid value.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `s` is a valid out-pointer.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
        return None;
    }
    // ACLineStatus 0 is offline; a percentage of 255 is unknown.
    (s.ACLineStatus == 0 && s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn battery() -> Option<u8> {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        crate::log::log!(
            Warn,
            "GetCurrentBatteryPower is not implemented on this OS yet; it reports mains power"
        );
    });
    None
}

pub fn api_call_completed(o: &Object, call: u64, failed: *mut c_void) -> bool {
    let done = pipe::get(o.pipe).and_then(|p| p.call_completed(call));
    if let Some(f) = done {
        // SAFETY: the SDK declares a `bool *`.
        unsafe { put(failed, u8::from(f)) };
    }
    done.is_some()
}

pub fn api_call_failure_reason(o: &Object, call: u64) -> u32 {
    pipe::get(o.pipe).map_or(pipe::FAILURE_INVALID_HANDLE, |p| p.call_failure(call)) as u32
}

pub fn get_api_call_result(
    o: &Object,
    call: u64,
    callback: *mut c_void,
    size: i32,
    expected: i32,
    failed: *mut c_void,
) -> bool {
    // SAFETY: the SDK declares a buffer of `size` bytes and a `bool *`.
    unsafe { api_call_result(o.pipe, call, callback, size, expected, failed) }
}

/// `GetAPICallResult` and `Steam_GetAPICallResult` (§2.5).
///
/// # Safety
///
/// `callback` is null or holds `size` writable bytes; `failed` is null or
/// writable.
pub unsafe fn api_call_result(
    pipe: i32,
    call: u64,
    callback: *mut c_void,
    size: i32,
    expected: i32,
    failed: *mut c_void,
) -> bool {
    let Some(p) = pipe::get(pipe) else {
        return false;
    };
    // SAFETY: as the caller promises.
    match unsafe { p.take_result(call, callback.cast(), size, expected) } {
        Some(f) => {
            // SAFETY: as the caller promises.
            unsafe { put(failed, u8::from(f)) };
            true
        }
        None => false,
    }
}

pub fn run_frame(_: &Object) {}

pub fn ipc_call_count(_: &Object) -> u32 {
    pipe::take_ipc_calls()
}

pub fn set_warning_message_hook(o: &Object, hook: *mut c_void) {
    crate::client::set_warning_message_hook(o, hook);
}

pub fn set_overlay_notification_position(_: &Object, _position: u32) {}

pub fn set_overlay_notification_inset(_: &Object, _horizontal: i32, _vertical: i32) {}

pub fn no(_: &Object) -> bool {
    false
}
