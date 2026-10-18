// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamApps` (`docs/spec/steamworks/isteamapps.md`).

use std::ffi::{c_char, c_void};

use ossteam_gameipc::{App, AppState, AppStateRequest, RequestBody, ResponseBody};

use crate::abi::{EMPTY, intern, put, write_str, write_str_truncated};
use crate::object::Object;
use crate::pipe;

/// Answers from the app's state, or `default` without an open pipe.
fn with_app<R>(o: &Object, default: R, f: impl FnOnce(&App) -> R) -> R {
    match pipe::get(o.pipe) {
        Some(p) => f(&p.info().app),
        None => default,
    }
}

/// The daemon's state of another app; `None` when it cannot answer.
fn app_state(o: &Object, app_id: u32) -> Option<AppState> {
    let p = pipe::get(o.pipe)?;
    match p.request(RequestBody::AppState(AppStateRequest { app_id }))? {
        ResponseBody::AppState(s) => Some(s),
        ResponseBody::Error(e) => {
            crate::log::log!(Warn, "AppState for {app_id}: {}", e.message);
            None
        }
        other => {
            crate::log::log!(Warn, "AppState for {app_id}: the daemon sent {other:?}");
            None
        }
    }
}

/// What is known about `app_id` without asking the daemon: the app
/// itself, or one of its DLC, as (owned, installed).
fn known(o: &Object, app_id: u32) -> Option<(bool, bool)> {
    with_app(o, None, |a| {
        if a.app_id == app_id {
            Some((a.owned, a.installed))
        } else {
            a.dlc
                .iter()
                .find(|d| d.app_id == app_id)
                .map(|d| (d.owned, d.owned && d.installed))
        }
    })
}

pub fn subscribed(o: &Object) -> bool {
    with_app(o, false, |a| a.owned)
}

pub fn subscribed_app(o: &Object, app_id: u32) -> bool {
    match known(o, app_id) {
        Some((owned, _)) => owned,
        None => app_state(o, app_id).is_some_and(|s| s.owned),
    }
}

pub fn app_installed(o: &Object, app_id: u32) -> bool {
    if with_app(o, false, |a| a.app_id == app_id) {
        return with_app(o, false, |a| a.installed);
    }
    app_state(o, app_id).is_some_and(|s| s.installed)
}

pub fn dlc_installed(o: &Object, app_id: u32) -> bool {
    match known(o, app_id) {
        Some((_, installed)) => installed,
        None => app_state(o, app_id).is_some_and(|s| s.owned && s.installed),
    }
}

pub fn dlc_count(o: &Object) -> i32 {
    with_app(o, 0, |a| a.dlc.len().min(i32::MAX as usize) as i32)
}

pub fn dlc_data_by_index(
    o: &Object,
    index: i32,
    app_id: *mut c_void,
    available: *mut c_void,
    name: *mut c_void,
    size: i32,
) -> bool {
    let dlc = with_app(o, None, |a| {
        usize::try_from(index)
            .ok()
            .and_then(|i| a.dlc.get(i).cloned())
    });
    let Some(d) = dlc else {
        return false;
    };
    // SAFETY: the SDK declares an `AppId_t *`, a `bool *` and a buffer of
    // `size` bytes.
    unsafe {
        put(app_id, d.app_id);
        put(available, u8::from(d.available));
        write_str_truncated(name, size.into(), &d.name);
    }
    true
}

pub fn current_beta_name(o: &Object, name: *mut c_void, size: i32) -> bool {
    let branch = with_app(o, String::new(), |a| a.branch.clone());
    if branch.is_empty() {
        // SAFETY: the SDK declares a buffer of `size` bytes.
        unsafe { write_str(name, size.into(), "") };
        return false;
    }
    // SAFETY: as above.
    unsafe { write_str_truncated(name, size.into(), &branch) };
    true
}

pub fn build_id(o: &Object) -> i32 {
    with_app(o, 0, |a| if a.installed { a.build_id as i32 } else { 0 })
}

pub fn install_dir(o: &Object, app_id: u32, buf: *mut c_void, size: u32) -> u32 {
    let own = with_app(o, None, |a| {
        (a.app_id == app_id).then(|| a.installed.then(|| a.install_dir.clone()))
    });
    let dir = match own {
        Some(dir) => dir,
        None => app_state(o, app_id).and_then(|s| s.installed.then_some(s.install_dir)),
    };
    let Some(dir) = dir.filter(|d| !d.is_empty()) else {
        // SAFETY: the SDK declares a buffer of `size` bytes.
        unsafe { write_str(buf, size.into(), "") };
        return 0;
    };
    // SAFETY: as above.
    unsafe { write_str_truncated(buf, size.into(), &dir) as u32 }
}

pub fn installed_depots(o: &Object, app_id: u32, depots: *mut c_void, max: u32) -> u32 {
    let own = with_app(o, None, |a| {
        (a.app_id == app_id).then(|| {
            if a.installed {
                a.depots.clone()
            } else {
                Vec::new()
            }
        })
    });
    let list = match own {
        Some(list) => list,
        None => app_state(o, app_id)
            .filter(|s| s.installed)
            .map_or_else(Vec::new, |s| s.depots),
    };
    let n = list.len().min(max as usize);
    if depots.is_null() {
        return 0;
    }
    let out = depots.cast::<u32>();
    for (i, d) in list[..n].iter().enumerate() {
        // SAFETY: the SDK declares `DepotId_t *` with room for `max`.
        unsafe { out.add(i).write_unaligned(*d) };
    }
    n as u32
}

pub fn installed_depots_own(o: &Object, depots: *mut c_void, max: u32) -> u32 {
    let app_id = with_app(o, 0, |a| a.app_id);
    installed_depots(o, app_id, depots, max)
}

pub fn current_game_language(o: &Object) -> *const c_char {
    with_app(o, EMPTY.as_ptr(), |a| intern(&a.language))
}

pub fn available_game_languages(o: &Object) -> *const c_char {
    with_app(o, EMPTY.as_ptr(), |a| intern(&a.languages.join(",")))
}

pub fn app_owner(o: &Object) -> u64 {
    with_app(o, 0, |a| a.owner)
}

pub fn family_shared(_: &Object) -> bool {
    false
}

pub fn low_violence(o: &Object) -> bool {
    with_app(o, false, |a| a.low_violence)
}

pub fn cybercafe(_: &Object) -> bool {
    false
}

pub fn launch_query_param(_: &Object, _key: *mut c_void) -> *const c_char {
    EMPTY.as_ptr()
}

pub fn launch_command_line(_: &Object, buf: *mut c_void, size: i32) -> i32 {
    // SAFETY: the SDK declares a buffer of `size` bytes.
    unsafe { write_str(buf, size.into(), "") };
    0
}
