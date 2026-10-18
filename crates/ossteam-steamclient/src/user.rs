// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamUser` (`docs/spec/steamworks/isteamuser.md`).

use std::ffi::c_void;
use std::path::Path;

use crate::abi::write_str;
use crate::log::log;
use crate::object::Object;
use crate::pipe;

pub fn h_steam_user(o: &Object) -> i32 {
    o.user
}

pub fn logged_on(o: &Object) -> bool {
    pipe::get(o.pipe).is_some_and(|p| p.logged_on())
}

pub fn steam_id(o: &Object) -> u64 {
    pipe::get(o.pipe).map_or(0, |p| {
        let i = p.info();
        if i.session.active {
            i.session.steam_id
        } else {
            0
        }
    })
}

pub fn user_data_folder(o: &Object, buf: *mut c_void, size: i32) -> bool {
    let dir = pipe::get(o.pipe).and_then(|p| {
        let i = p.info();
        (i.session.active && !i.app.user_dir.is_empty()).then(|| i.app.user_dir.clone())
    });
    let Some(dir) = dir else {
        // SAFETY: the SDK declares `buf` as `size` bytes.
        unsafe { write_str(buf, size.into(), "") };
        return false;
    };
    let local = Path::new(&dir).join("local");
    if let Err(e) = std::fs::create_dir_all(&local) {
        log!(Warn, "cannot create {}: {e}", local.display());
        // SAFETY: as above.
        unsafe { write_str(buf, size.into(), "") };
        return false;
    }
    // SAFETY: as above.
    unsafe { write_str(buf, size.into(), &local.to_string_lossy()) }
}
