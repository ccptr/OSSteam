// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamFriends` (`docs/spec/steamworks/isteamfriends.md`).

use std::ffi::c_char;

use crate::abi::{EMPTY, intern};
use crate::object::Object;
use crate::pipe;

pub fn persona_name(o: &Object) -> *const c_char {
    pipe::get(o.pipe).map_or(EMPTY.as_ptr(), |p| {
        let i = p.info();
        if i.session.active {
            intern(&i.session.persona_name)
        } else {
            EMPTY.as_ptr()
        }
    })
}

pub fn persona_state(o: &Object) -> u32 {
    pipe::get(o.pipe).map_or(0, |p| {
        if p.logged_on() {
            p.info().session.persona_state
        } else {
            0
        }
    })
}
