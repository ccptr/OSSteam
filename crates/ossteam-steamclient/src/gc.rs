// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamGameCoordinator` (`docs/spec/steamworks/isteamgamecoordinator.md`
//! §3): messages between the game and its app's Game Coordinator. The
//! daemon carries them to and from Steam; the shim keeps the ones that
//! arrived until the game retrieves them.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use ossteam_gameipc::{GcMessage, GcSendRequest, RequestBody, ResponseBody};

use crate::abi::put;
use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe::{self, Pipe};

/// `EGCResults`.
const OK: u32 = 0;
const NO_MESSAGE: u32 = 1;
const BUFFER_TOO_SMALL: u32 = 2;
const NOT_LOGGED_ON: u32 = 3;
const INVALID_MESSAGE: u32 = 4;

/// The largest message the shim sends: the game IPC's frame limit, with
/// room for the rest of the frame (§3).
const MAX_MESSAGE: usize = 15 * 1024 * 1024;

/// A pipe's messages from the GC, oldest first.
#[derive(Default)]
pub struct Inbox(VecDeque<GcMessage>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A `GcMessage` notice: the message waits for the game, which hears of
/// it through callback 1701.
pub fn on_message(p: &Pipe, m: GcMessage) {
    let size = m.payload.len() as u32;
    lock(&p.gc).0.push_back(m);
    p.push(
        callbacks::GC_MESSAGE_AVAILABLE,
        callbacks::gc_message_available(size),
    );
}

pub fn send_message(o: &Object, msg_type: u32, data: *mut c_void, size: u32) -> u32 {
    let Ok(len) = usize::try_from(size) else {
        return INVALID_MESSAGE;
    };
    if (data.is_null() && len > 0) || len > MAX_MESSAGE {
        return INVALID_MESSAGE;
    }
    let Some(p) = pipe::get(o.pipe) else {
        return NOT_LOGGED_ON;
    };
    if !p.logged_on() {
        return NOT_LOGGED_ON;
    }
    let payload = if len == 0 {
        Vec::new()
    } else {
        // SAFETY: the SDK declares `size` bytes of data.
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }.to_vec()
    };
    let body = RequestBody::GcSend(GcSendRequest { msg_type, payload });
    let sent = p.send_then(
        body,
        Box::new(move |p, answer| match answer {
            Some(ResponseBody::GcSent(s)) if s.result == OK => {}
            other => {
                log!(
                    Info,
                    "a message to the game coordinator was not sent: {other:?}"
                );
                p.push(callbacks::GC_MESSAGE_FAILED, callbacks::gc_message_failed());
            }
        }),
    );
    if sent { OK } else { NOT_LOGGED_ON }
}

pub fn is_message_available(o: &Object, size: *mut c_void) -> bool {
    let len = pipe::get(o.pipe).and_then(|p| lock(&p.gc).0.front().map(|m| m.payload.len()));
    // SAFETY: the SDK declares a `uint32 *`; `put` skips null.
    unsafe { put(size, len.unwrap_or(0) as u32) };
    len.is_some()
}

pub fn retrieve_message(
    o: &Object,
    msg_type: *mut c_void,
    dest: *mut c_void,
    cap: u32,
    size: *mut c_void,
) -> u32 {
    let Some(p) = pipe::get(o.pipe) else {
        return NO_MESSAGE;
    };
    let mut inbox = lock(&p.gc);
    let Some(m) = inbox.0.front() else {
        return NO_MESSAGE;
    };
    let len = m.payload.len();
    // SAFETY: the SDK declares `uint32 *`s; `put` skips null.
    unsafe {
        put(size, len as u32);
        put(msg_type, m.msg_type);
    }
    if dest.is_null() || (cap as usize) < len {
        return BUFFER_TOO_SMALL;
    }
    // SAFETY: the SDK declares a buffer of `cap` bytes, which the message
    // fits in.
    unsafe { std::ptr::copy_nonoverlapping(m.payload.as_ptr(), dest.cast(), len) };
    inbox.0.pop_front();
    OK
}
