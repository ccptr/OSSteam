// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamClient` (`docs/spec/steamworks/isteamclient.md`, §2.3).

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::abi::{find_version, read_str};
use crate::log::log;
use crate::object::{self, Object};
use crate::pipe;
use crate::vtables::VERSIONS;

/// `SetWarningMessageHook`'s function, stored and never called.
static WARNING_HOOK: AtomicUsize = AtomicUsize::new(0);
/// `Set_SteamAPI_CCheckCallbackRegisteredInProcess`'s function.
static CHECK_CALLBACK: AtomicUsize = AtomicUsize::new(0);

pub fn create_steam_pipe(_: &Object) -> i32 {
    pipe::create()
}

pub fn release_steam_pipe(_: &Object, pipe: i32) -> bool {
    pipe::release(pipe)
}

pub fn shutdown_if_all_pipes_closed(_: &Object) -> bool {
    !pipe::any_open()
}

pub fn connect_to_global_user(_: &Object, pipe: i32) -> i32 {
    let Some(p) = pipe::get(pipe) else {
        log!(Warn, "ConnectToGlobalUser: pipe {pipe} is not open");
        return 0;
    };
    {
        let i = p.info();
        if !i.session.active {
            log!(Warn, "no account is logged in to ossteam");
            return 0;
        }
        if !i.app.owned {
            log!(Warn, "the account does not own app {}", i.app.app_id);
            return 0;
        }
    }
    p.set_user(1);
    1
}

pub fn create_local_user(_: &Object, _pipe: *mut c_void, _kind: u32) -> i32 {
    log!(
        Warn,
        "game servers and anonymous users are not supported yet"
    );
    0
}

pub fn create_local_user_old(o: &Object, pipe: *mut c_void) -> i32 {
    create_local_user(o, pipe, 0)
}

pub fn release_user(_: &Object, pipe: i32, user: i32) {
    if let Some(p) = pipe::get(pipe)
        && user != 0
        && p.user() == user
    {
        p.set_user(0);
    }
}

/// The interface a getter's name stands for.
fn interface_of_getter(getter: &str) -> &str {
    getter.strip_prefix("Get").unwrap_or(getter)
}

/// The object of `version` for `pipe` and `user` (§2.3), when `version`
/// is a version of `interface` (or of any interface, for `None`).
///
/// # Safety
///
/// `version` is null or a NUL-terminated string.
unsafe fn hand_out(
    interface: Option<&str>,
    user: i32,
    pipe: i32,
    version: *mut c_void,
) -> *mut c_void {
    // SAFETY: as the caller promises.
    let Some(name) = (unsafe { read_str(version) }) else {
        return null_mut();
    };
    let found = find_version(name.as_bytes())
        .filter(|&v| interface.is_none_or(|i| VERSIONS[v].interface_name() == i));
    let Some(v) = found else {
        crate::report::unknown(name);
        return null_mut();
    };
    let iface = VERSIONS[v].interface_name();
    if iface == "ISteamClient" {
        return object::get(v, 0, 0).cast();
    }
    let Some(p) = pipe::get(pipe) else {
        log!(
            Warn,
            "{name} was asked for on pipe {pipe}, which is not open"
        );
        return null_mut();
    };
    let user = if matches!(iface, "ISteamUtils" | "ISteamNetworkingUtils") {
        0
    } else if user != 0 && user == p.user() {
        user
    } else {
        log!(
            Warn,
            "{name} was asked for with user {user}, which pipe {pipe} does not have"
        );
        return null_mut();
    };
    let obj = object::get(v, pipe, user);
    if iface == "ISteamUserStats" {
        crate::userstats::handed_out(&p, v);
    }
    obj.cast()
}

macro_rules! getters {
    ($($fn:ident => $getter:literal,)*) => {$(
        pub fn $fn(_: &Object, user: i32, pipe: i32, version: *mut c_void) -> *mut c_void {
            // SAFETY: steam_api passes a version string, as the SDK
            // declares.
            unsafe { hand_out(Some(interface_of_getter($getter)), user, pipe, version) }
        }
    )*};
}

getters! {
    get_app_list => "GetISteamAppList",
    get_apps => "GetISteamApps",
    get_billing => "GetISteamBilling",
    get_content_server => "GetISteamContentServer",
    get_controller => "GetISteamController",
    get_friends => "GetISteamFriends",
    get_game_search => "GetISteamGameSearch",
    get_game_server => "GetISteamGameServer",
    get_game_server_stats => "GetISteamGameServerStats",
    get_html_surface => "GetISteamHTMLSurface",
    get_http => "GetISteamHTTP",
    get_input => "GetISteamInput",
    get_inventory => "GetISteamInventory",
    get_master_server_updater => "GetISteamMasterServerUpdater",
    get_matchmaking => "GetISteamMatchmaking",
    get_matchmaking_servers => "GetISteamMatchmakingServers",
    get_music => "GetISteamMusic",
    get_music_remote => "GetISteamMusicRemote",
    get_networking => "GetISteamNetworking",
    get_parental_settings => "GetISteamParentalSettings",
    get_parties => "GetISteamParties",
    get_remote_play => "GetISteamRemotePlay",
    get_remote_storage => "GetISteamRemoteStorage",
    get_screenshots => "GetISteamScreenshots",
    get_ugc => "GetISteamUGC",
    get_unified_messages => "GetISteamUnifiedMessages",
    get_user => "GetISteamUser",
    get_user_stats => "GetISteamUserStats",
    get_video => "GetISteamVideo",
}

pub fn get_utils(_: &Object, pipe: i32, version: *mut c_void) -> *mut c_void {
    // SAFETY: as for the other getters.
    unsafe { hand_out(Some("ISteamUtils"), 0, pipe, version) }
}

pub fn get_generic(_: &Object, user: i32, pipe: i32, version: *mut c_void) -> *mut c_void {
    // SAFETY: as for the other getters.
    unsafe { hand_out(None, user, pipe, version) }
}

pub fn get_vac(_: &Object, _user: i32) -> *mut c_void {
    log!(Warn, "GetIVAC: VAC is not provided yet");
    null_mut()
}

pub fn set_local_ip_binding(_: &Object, _ip: u32, _port: u16) {}

pub fn set_local_ip_binding_address(_: &Object, _ip: *mut c_void, _port: u16) {}

pub fn run_frame(_: &Object) {}

pub fn ipc_call_count(_: &Object) -> u32 {
    pipe::take_ipc_calls()
}

pub fn set_warning_message_hook(_: &Object, hook: *mut c_void) {
    WARNING_HOOK.store(hook as usize, Ordering::Relaxed);
}

pub fn set_check_callback_registered(_: &Object, func: *mut c_void) {
    CHECK_CALLBACK.store(func as usize, Ordering::Relaxed);
}

pub fn ignore_post_api_result(_: &Object, _func: *mut c_void) {}

pub fn destroy_all_interfaces(_: &Object) {}
