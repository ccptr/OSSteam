// SPDX-License-Identifier: GPL-2.0-or-later
//! ossteam's Steamworks client library (`steamworks-abi.md` §2, SPEC
//! §12.3). Games load it through their own `steam_api`, which finds it
//! through the discovery entries (§1). It talks to `ossteamd` over the
//! game IPC (§3) and never to Steam itself.
//!
//! Every slot of every interface version in `vtables.txt` exists, with
//! its exact signature. The ones listed in `implemented.txt` call the
//! modules below; the rest are stubs that record the call (ABI-3, §4).

mod abi;
mod apps;
mod callbacks;
mod client;
mod friends;
mod gc;
mod guard;
mod images;
mod log;
mod networking;
mod object;
pub mod pipe;
mod remote_storage;
mod report;
#[cfg(all(test, unix))]
mod tests;
mod tickets;
mod user;
mod userstats;
mod utils;

#[allow(clippy::all)]
mod vtables {
    use crate::abi::{Fp, SlotInfo, VersionInfo};
    include!(concat!(env!("OUT_DIR"), "/vtables.rs"));
}

use std::ffi::{c_char, c_int, c_void};

use crate::abi::CallbackMsg;

/// `CreateInterface` (§2.1): an `ISteamClient`.
///
/// # Safety
///
/// `version` is null or a NUL-terminated string; `return_code` is null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CreateInterface(
    version: *const c_char,
    return_code: *mut c_int,
) -> *mut c_void {
    let found = guard::export(
        "CreateInterface",
        || {
            // SAFETY: as the caller promises.
            let name = unsafe { abi::read_str(version.cast()) }?;
            match abi::find_version(name.as_bytes()) {
                Some(v) if vtables::VERSIONS[v].interface_name() == "ISteamClient" => {
                    Some(object::get(v, 0, 0).cast::<c_void>())
                }
                _ => {
                    report::unknown(name);
                    None
                }
            }
        },
        || None,
    );
    // SAFETY: as the caller promises.
    unsafe { abi::put(return_code.cast(), c_int::from(found.is_none())) };
    found.unwrap_or(std::ptr::null_mut())
}

/// `Steam_BGetCallback` (§2.5).
///
/// # Safety
///
/// `msg` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Steam_BGetCallback(
    pipe: i32,
    msg: *mut CallbackMsg,
    _call: *mut i32,
) -> bool {
    guard::export(
        "Steam_BGetCallback",
        || {
            pipe::send_report(false);
            if msg.is_null() {
                return false;
            }
            let Some(p) = pipe::get(pipe) else {
                return false;
            };
            let Some((user, callback, param, size)) = p.next_callback() else {
                return false;
            };
            let m = CallbackMsg {
                user,
                callback,
                param,
                size,
            };
            // SAFETY: as the caller promises.
            unsafe { abi::put(msg.cast(), m) };
            true
        },
        || false,
    )
}

/// `Steam_FreeLastCallback` (§2.5).
#[unsafe(no_mangle)]
pub extern "C" fn Steam_FreeLastCallback(pipe: i32) -> bool {
    guard::export(
        "Steam_FreeLastCallback",
        || pipe::get(pipe).is_some_and(|p| p.free_callback()),
        || false,
    )
}

/// `Steam_GetAPICallResult` (§2.5).
///
/// # Safety
///
/// `callback` is null or holds `size` writable bytes; `failed` is null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Steam_GetAPICallResult(
    pipe: i32,
    call: u64,
    callback: *mut c_void,
    size: c_int,
    expected: c_int,
    failed: *mut bool,
) -> bool {
    guard::export(
        "Steam_GetAPICallResult",
        // SAFETY: as the caller promises.
        || unsafe { utils::api_call_result(pipe, call, callback, size, expected, failed.cast()) },
        || false,
    )
}

/// `Steam_IsKnownInterface` (§2.1).
///
/// # Safety
///
/// `version` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Steam_IsKnownInterface(version: *const c_char) -> bool {
    guard::export(
        "Steam_IsKnownInterface",
        // SAFETY: as the caller promises.
        || {
            unsafe { abi::read_str(version.cast()) }
                .is_some_and(|v| abi::find_version(v.as_bytes()).is_some())
        },
        || false,
    )
}

/// `Steam_NotifyMissingInterface` (§2.1).
///
/// # Safety
///
/// `version` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Steam_NotifyMissingInterface(_pipe: i32, version: *const c_char) {
    guard::export(
        "Steam_NotifyMissingInterface",
        || {
            // SAFETY: as the caller promises.
            if let Some(v) = unsafe { abi::read_str(version.cast()) } {
                report::unknown(v);
            }
        },
        || (),
    );
}

/// `Steam_ReleaseThreadLocalMemory` (§2.1): the shim keeps none.
#[unsafe(no_mangle)]
pub extern "C" fn Steam_ReleaseThreadLocalMemory(_thread_exit: c_int) {}

#[unsafe(no_mangle)]
pub extern "C" fn Breakpad_SteamMiniDumpInit(_: u32, _: *const c_char, _: *const c_char) {
    report::export(0);
}

#[unsafe(no_mangle)]
pub extern "C" fn Breakpad_SteamSetAppID(_: u32) {
    report::export(1);
}

#[unsafe(no_mangle)]
pub extern "C" fn Breakpad_SteamSetSteamID(_: u64) -> c_int {
    report::export(2);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn Breakpad_SteamWriteMiniDumpSetComment(_: *const c_char) -> c_int {
    report::export(3);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn Breakpad_SteamWriteMiniDumpUsingExceptionInfoWithBuildId(_: c_int, _: c_int) {
    report::export(4);
}
