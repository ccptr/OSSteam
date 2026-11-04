// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamNetworkingSocketsSerialized`
//! (`docs/spec/steamworks/isteamnetworkingsocketsserialized.md`): the
//! relay certificate (§3) and the relay ticket cache (§5). The other
//! methods are stubs (§4).

use std::ffi::c_void;

use ossteam_gameipc::{
    CacheRelayTicketRequest, NetworkingCertRequest, RelayTicketsRequest, RequestBody, ResponseBody,
};

use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe;

/// The largest ticket the daemon keeps (§5).
const MAX_TICKET: usize = 16 * 1024;

pub fn get_cert_async(o: &Object) -> u64 {
    let Some(p) = pipe::get(o.pipe) else {
        return 0;
    };
    p.begin_call(
        RequestBody::NetworkingCert(NetworkingCertRequest {}),
        callbacks::NETWORKING_CERT,
        callbacks::NETWORKING_CERT_SIZE,
        Box::new(|_, answer| {
            let ResponseBody::NetworkingCert(c) = answer else {
                return None;
            };
            let result = c.result as i32;
            Some(if result == 1 {
                callbacks::networking_cert(
                    result,
                    &c.cert,
                    c.ca_key_id,
                    &c.signature,
                    &c.private_key,
                )
            } else {
                log!(Info, "no networking certificate: {} ({result})", c.message);
                callbacks::networking_cert(result, c.message.as_bytes(), 0, &[], &[])
            })
        }),
    )
}

pub fn cache_relay_ticket(o: &Object, ticket: *mut c_void, size: u32) {
    let len = size as usize;
    if ticket.is_null() || len == 0 || len > MAX_TICKET {
        return;
    }
    let Some(p) = pipe::get(o.pipe) else {
        return;
    };
    // SAFETY: the game passes `size` bytes of ticket.
    let ticket = unsafe { std::slice::from_raw_parts(ticket.cast::<u8>(), len) }.to_vec();
    p.send_then(
        RequestBody::CacheRelayTicket(CacheRelayTicketRequest { ticket }),
        Box::new(|_, answer| {
            if !matches!(answer, Some(ResponseBody::Done(_))) {
                log!(Info, "a relay ticket was not kept: {answer:?}");
            }
        }),
    );
}

/// The app's relay tickets, or none when the daemon cannot say.
fn tickets(o: &Object) -> Vec<Vec<u8>> {
    let Some(p) = pipe::get(o.pipe) else {
        return Vec::new();
    };
    match p.request(RequestBody::RelayTickets(RelayTicketsRequest {})) {
        Some(ResponseBody::RelayTickets(t)) => t.tickets,
        other => {
            log!(Info, "relay tickets: the daemon sent {other:?}");
            Vec::new()
        }
    }
}

pub fn get_cached_relay_ticket_count(o: &Object) -> u32 {
    tickets(o).len() as u32
}

pub fn get_cached_relay_ticket(o: &Object, index: u32, buf: *mut c_void, cap: u32) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let all = tickets(o);
    let Some(t) = all.get(index as usize) else {
        return 0;
    };
    if t.len() > cap as usize {
        return 0;
    }
    // SAFETY: the game passes a buffer of `cap` bytes, which the ticket
    // fits in.
    unsafe { std::ptr::copy_nonoverlapping(t.as_ptr(), buf.cast(), t.len()) };
    t.len() as i32
}
