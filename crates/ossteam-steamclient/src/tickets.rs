// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamUser`'s tickets and auth sessions
//! (`docs/spec/steamworks/isteamuser.md` §4). The daemon makes the
//! tickets and keeps Steam informed; the shim hands out handles and turns
//! the daemon's notices into callbacks.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use ossteam_gameipc::{
    AuthTicketRequest, BeginAuthSessionRequest, CancelAuthTicketRequest, EncryptedAppTicketRequest,
    EndAuthSessionRequest, NoticeBody, RequestBody, ResponseBody, UserLicenseRequest,
};

use crate::abi::{put, read_str};
use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe::{self, Pipe};

/// `type` of a session ticket and of a Web API ticket (§2.3).
const SESSION: u32 = 2;
const WEB_API: u32 = 5;
/// `k_HAuthTicketInvalid`.
const INVALID_HANDLE: u32 = 0;
/// `k_EBeginAuthSessionResultInvalidTicket`.
const BEGIN_INVALID: u32 = 1;
/// `k_EUserHasLicenseResultNoAuth`.
const LICENSE_NO_AUTH: u32 = 2;
/// The ticket sizes `BeginAuthSession` passes on.
const MAX_TICKET: i32 = 4096;
const OK: u32 = 1;

static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

/// A pipe's tickets.
#[derive(Default)]
pub struct Tickets {
    /// The CRC of each handle's ticket.
    handles: HashMap<u32, u32>,
    /// Web API tickets waiting for their callback, by CRC.
    web: HashMap<u32, Vec<u8>>,
    /// `TicketActive` notices that arrived before their handle was given
    /// out, by CRC.
    early: HashMap<u32, u32>,
    /// The last encrypted app ticket.
    encrypted: Option<Vec<u8>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A daemon's notice (§3).
pub fn on_notice(p: &Pipe, n: NoticeBody) {
    match n {
        NoticeBody::TicketActive(a) => {
            let mut t = lock(&p.tickets);
            match t.handles.iter().find(|(_, c)| **c == a.crc) {
                Some((&handle, _)) => ticket_ready(p, &mut t, handle, a.crc, a.result),
                None => {
                    t.early.insert(a.crc, a.result);
                }
            }
        }
        NoticeBody::TicketValidated(v) => p.push(
            callbacks::VALIDATE_AUTH_TICKET_RESPONSE,
            callbacks::validate_auth_ticket_response(v.steam_id, v.response, v.owner),
        ),
    }
}

/// Queues callback 163 or 168 for a ticket Steam took, or did not.
fn ticket_ready(p: &Pipe, t: &mut Tickets, handle: u32, crc: u32, result: u32) {
    match t.web.remove(&crc) {
        Some(ticket) => p.push(
            callbacks::GET_TICKET_FOR_WEB_API_RESPONSE,
            callbacks::ticket_for_web_api_response(handle, result, &ticket),
        ),
        None => p.push(
            callbacks::GET_AUTH_SESSION_TICKET_RESPONSE,
            callbacks::auth_session_ticket_response(handle, result),
        ),
    }
}

/// Asks the daemon for a ticket and gives it a handle. `None` when there
/// is none.
fn ask_ticket(p: &Pipe, kind: u32, identity: &str, max: u32) -> Option<(u32, Vec<u8>)> {
    if !p.logged_on() {
        log!(Warn, "a ticket needs a connection to Steam");
        return None;
    }
    let body = RequestBody::AuthTicket(AuthTicketRequest {
        kind,
        identity: identity.into(),
        max,
    });
    let a = match p.request(body) {
        Some(ResponseBody::AuthTicket(a)) if a.result == OK => a,
        Some(ResponseBody::AuthTicket(a)) => {
            log!(Warn, "no ticket: result {}", a.result);
            return None;
        }
        other => {
            log!(Warn, "no ticket: {other:?}");
            return None;
        }
    };
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let mut t = lock(&p.tickets);
    t.handles.insert(handle, a.crc);
    if kind == WEB_API {
        t.web.insert(a.crc, a.ticket.clone());
    }
    if let Some(result) = t.early.remove(&a.crc) {
        ticket_ready(p, &mut t, handle, a.crc, result);
    }
    Some((handle, a.ticket))
}

pub fn get_auth_session_ticket(o: &Object, buf: *mut c_void, max: i32, size: *mut c_void) -> u32 {
    // SAFETY: the SDK declares a `uint32 *`.
    unsafe { put(size, 0u32) };
    let Some(p) = pipe::get(o.pipe) else {
        return INVALID_HANDLE;
    };
    let Ok(max) = u32::try_from(max) else {
        return INVALID_HANDLE;
    };
    if buf.is_null() || max == 0 {
        return INVALID_HANDLE;
    }
    let Some((handle, ticket)) = ask_ticket(&p, SESSION, "", max) else {
        return INVALID_HANDLE;
    };
    if ticket.len() > max as usize {
        // The daemon checked; this is only a guard.
        return INVALID_HANDLE;
    }
    // SAFETY: the SDK declares a buffer of `max` bytes, which the ticket
    // fits in, and a `uint32 *`.
    unsafe {
        std::ptr::copy_nonoverlapping(ticket.as_ptr(), buf.cast(), ticket.len());
        put(size, ticket.len() as u32);
    }
    handle
}

/// Versions `022` and `023`, with a `SteamNetworkingIdentity *` that is
/// not used.
pub fn get_auth_session_ticket_for(
    o: &Object,
    buf: *mut c_void,
    max: i32,
    size: *mut c_void,
    _identity: *mut c_void,
) -> u32 {
    get_auth_session_ticket(o, buf, max, size)
}

pub fn get_auth_ticket_for_web_api(o: &Object, identity: *mut c_void) -> u32 {
    let Some(p) = pipe::get(o.pipe) else {
        return INVALID_HANDLE;
    };
    // SAFETY: the SDK declares a NUL-terminated string.
    let identity = unsafe { read_str(identity) }.unwrap_or("");
    ask_ticket(&p, WEB_API, identity, 0).map_or(INVALID_HANDLE, |(h, _)| h)
}

pub fn cancel_auth_ticket(o: &Object, handle: u32) {
    let Some(p) = pipe::get(o.pipe) else {
        return;
    };
    let crc = {
        let mut t = lock(&p.tickets);
        let Some(crc) = t.handles.remove(&handle) else {
            return;
        };
        t.web.remove(&crc);
        crc
    };
    p.send_then(
        RequestBody::CancelAuthTicket(CancelAuthTicketRequest { crc }),
        Box::new(|_, _| {}),
    );
}

pub fn begin_auth_session(o: &Object, ticket: *mut c_void, size: i32, user: u64) -> u32 {
    let Some(p) = pipe::get(o.pipe) else {
        return BEGIN_INVALID;
    };
    if ticket.is_null() || !(1..=MAX_TICKET).contains(&size) {
        return BEGIN_INVALID;
    }
    // SAFETY: the SDK declares a ticket of `size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ticket.cast::<u8>(), size as usize) };
    let body = RequestBody::BeginAuthSession(BeginAuthSessionRequest {
        steam_id: user,
        ticket: bytes.to_vec(),
    });
    match p.request(body) {
        Some(ResponseBody::BeginAuthSession(b)) => b.result,
        _ => BEGIN_INVALID,
    }
}

pub fn end_auth_session(o: &Object, user: u64) {
    if let Some(p) = pipe::get(o.pipe) {
        p.send_then(
            RequestBody::EndAuthSession(EndAuthSessionRequest { steam_id: user }),
            Box::new(|_, _| {}),
        );
    }
}

pub fn user_has_license_for_app(o: &Object, user: u64, app: u32) -> u32 {
    let Some(p) = pipe::get(o.pipe) else {
        return LICENSE_NO_AUTH;
    };
    let body = RequestBody::UserLicense(UserLicenseRequest {
        steam_id: user,
        app_id: app,
    });
    match p.request(body) {
        Some(ResponseBody::UserLicense(u)) => u.result,
        _ => LICENSE_NO_AUTH,
    }
}

pub fn request_encrypted_app_ticket(o: &Object, data: *mut c_void, size: i32) -> u64 {
    let Some(p) = pipe::get(o.pipe) else {
        return 0;
    };
    let Ok(len) = usize::try_from(size) else {
        return 0;
    };
    if data.is_null() && len > 0 {
        return 0;
    }
    let userdata = if len == 0 {
        Vec::new()
    } else {
        // SAFETY: the SDK declares `size` bytes of data.
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }.to_vec()
    };
    let body = RequestBody::EncryptedAppTicket(EncryptedAppTicketRequest { userdata });
    p.begin_call(
        body,
        callbacks::ENCRYPTED_APP_TICKET_RESPONSE,
        4,
        Box::new(|p, answer| {
            let ResponseBody::EncryptedAppTicket(e) = answer else {
                return None;
            };
            if e.result == OK {
                lock(&p.tickets).encrypted = Some(e.ticket.clone());
            }
            Some(e.result.to_le_bytes().to_vec())
        }),
    )
}

pub fn get_encrypted_app_ticket(o: &Object, buf: *mut c_void, max: i32, size: *mut c_void) -> bool {
    // SAFETY: the SDK declares a `uint32 *`.
    unsafe { put(size, 0u32) };
    let Some(p) = pipe::get(o.pipe) else {
        return false;
    };
    let t = lock(&p.tickets);
    let Some(ticket) = &t.encrypted else {
        return false;
    };
    if buf.is_null() || usize::try_from(max).map_or(true, |m| m < ticket.len()) {
        return false;
    }
    // SAFETY: the SDK declares a buffer of `max` bytes, which the ticket
    // fits in, and a `uint32 *`.
    unsafe {
        std::ptr::copy_nonoverlapping(ticket.as_ptr(), buf.cast(), ticket.len());
        put(size, ticket.len() as u32);
    }
    true
}
