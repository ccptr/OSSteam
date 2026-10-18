// SPDX-License-Identifier: GPL-2.0-or-later
//! Auth session tickets, auth sessions and the auth list
//! (`docs/spec/steamworks/isteamuser.md` §2): the tickets' bytes, what
//! the daemon keeps for the CM session, and the messages it sends. The
//! core drives it (`core/tickets.rs`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::time::Instant;

use ossteam_gameipc::EncryptedAppTicket;
use ossteam_proto::steam::{CMsgAuthTicket, CMsgClientAuthList};
use ossteam_types::{AppId, CancelToken, SteamId};

use crate::gameipc::ConnId;

/// `type` in the session header.
pub const SESSION: u32 = 2;
pub const WEB_API: u32 = 5;
/// The length of the session header.
const HEADER: u32 = 24;
/// What an ownership ticket holds before its lists (§2.2).
const OWNERSHIP_FIXED: usize = 40;

/// `EBeginAuthSessionResult`.
pub const BEGIN_OK: u32 = 0;
pub const BEGIN_INVALID: u32 = 1;
pub const BEGIN_DUPLICATE: u32 = 2;
pub const BEGIN_GAME_MISMATCH: u32 = 4;
pub const BEGIN_EXPIRED: u32 = 5;

/// `EUserHasLicenseForAppResult`.
pub const LICENSE_YES: u32 = 0;
pub const LICENSE_NO: u32 = 1;
pub const LICENSE_NO_AUTH: u32 = 2;

/// The auth ticket: the token and the session header (§2.3).
pub fn auth_ticket(token: &[u8], kind: u32, public_ip: u32, ms: u32, n: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + token.len() + 4 + HEADER as usize);
    out.extend_from_slice(&(token.len() as u32).to_le_bytes());
    out.extend_from_slice(token);
    for word in [HEADER, 1, kind, public_ip, 0, ms, n] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// The ticket a game gets: the auth ticket and the ownership ticket.
pub fn full_ticket(auth: &[u8], ownership: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(auth.len() + 4 + ownership.len());
    out.extend_from_slice(auth);
    out.extend_from_slice(&(ownership.len() as u32).to_le_bytes());
    out.extend_from_slice(ownership);
    out
}

pub fn crc(auth: &[u8]) -> u32 {
    crc32fast::hash(auth)
}

/// The session header's address: the first byte of the address is the
/// word's most significant (§2.3).
pub fn ipv4_word(ip: Option<IpAddr>) -> u32 {
    match ip {
        Some(IpAddr::V4(v4)) => u32::from(v4),
        _ => 0,
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A ticket another user's game sent, split (§2.6).
#[derive(Debug, PartialEq)]
pub struct Split<'a> {
    pub auth: &'a [u8],
    pub ownership: &'a [u8],
}

pub fn split(ticket: &[u8]) -> Option<Split<'_>> {
    let token = u32_at(ticket, 0)? as usize;
    let header_at = 4usize.checked_add(token)?;
    if u32_at(ticket, header_at)? != HEADER {
        return None;
    }
    let auth_end = header_at + 4 + HEADER as usize;
    let len = u32_at(ticket, auth_end)? as usize;
    let ownership = ticket.get(auth_end + 4..)?.get(..len)?;
    if ownership.len() < OWNERSHIP_FIXED {
        return None;
    }
    Some(Split {
        auth: &ticket[..auth_end],
        ownership,
    })
}

/// The fixed fields of an ownership ticket that the daemon reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ownership {
    pub steam_id: u64,
    pub app: u32,
    pub expires: u32,
}

pub fn ownership(ticket: &[u8]) -> Option<Ownership> {
    if ticket.len() < OWNERSHIP_FIXED {
        return None;
    }
    Some(Ownership {
        steam_id: u64_at(ticket, 8)?,
        app: u32_at(ticket, 16)?,
        expires: u32_at(ticket, 36)?,
    })
}

/// Whether an ownership ticket grants `app`: its own app or one of its
/// DLC (§2.6). False when the lists do not parse.
pub fn grants(ticket: &[u8], app: u32) -> bool {
    let Some(o) = ownership(ticket) else {
        return false;
    };
    if o.app == app {
        return true;
    }
    let dlc = || -> Option<bool> {
        let packages = usize::from(u16_at(ticket, OWNERSHIP_FIXED)?);
        let mut at = OWNERSHIP_FIXED + 2 + 4 * packages;
        let count = u16_at(ticket, at)?;
        at += 2;
        for _ in 0..count {
            if u32_at(ticket, at)? == app {
                return Some(true);
            }
            let n = usize::from(u16_at(ticket, at + 4)?);
            at += 6 + 4 * n;
        }
        Some(false)
    };
    dlc().unwrap_or(false)
}

/// What the core hears back about tickets.
pub enum Event {
    /// An ownership ticket arrived for `account`, or could not be had.
    Fetched {
        account: SteamId,
        app: AppId,
        ticket: Result<Vec<u8>, String>,
    },
    /// The answer to a game's `EncryptedAppTicket` request.
    Encrypted {
        conn: ConnId,
        id: u32,
        answer: EncryptedAppTicket,
    },
    /// A request has waited long enough for a token or an ownership
    /// ticket (§3).
    WaitOver { conn: ConnId, id: u32 },
    /// An own ticket had its time to be acknowledged (§2.4).
    AckOver { conn: ConnId, crc: u32 },
}

/// One ticket in the auth list (§2.4).
#[derive(Debug, Clone)]
pub struct Entry {
    pub conn: ConnId,
    pub app: AppId,
    /// 0 for the account's own ticket, the other user's otherwise.
    pub steam_id: u64,
    pub crc: u32,
    pub auth: Vec<u8>,
    /// `str:<identity>` with a NUL, for a Web API ticket.
    pub secret: Option<Vec<u8>>,
    /// Another user's ownership ticket, for `UserHasLicenseForApp`.
    pub ownership: Vec<u8>,
    /// An own ticket that Steam has acknowledged.
    pub active: bool,
}

impl Entry {
    pub fn own(&self) -> bool {
        self.steam_id == 0
    }
}

/// A game's request for a ticket, waiting for a token or an ownership
/// ticket.
#[derive(Debug, Clone)]
pub struct Waiting {
    pub conn: ConnId,
    pub id: u32,
    pub app: AppId,
    pub kind: u32,
    pub identity: String,
    pub max: u32,
}

/// What the daemon keeps for the CM session (§2).
#[derive(Default)]
pub struct Tickets {
    tokens: VecDeque<Vec<u8>>,
    pub entries: Vec<Entry>,
    /// The last `message_sequence` sent, and the last acknowledged.
    seq_sent: u32,
    seq_acked: u32,
    /// Tickets made since the logon.
    made: u32,
    logon: Option<Instant>,
    public_ip: u32,
    /// The account's ownership ticket of each app.
    pub ownership: HashMap<AppId, Vec<u8>>,
    pub fetching: HashSet<AppId>,
    pub waiting: Vec<Waiting>,
    /// Interrupts the CM requests in flight at shutdown.
    pub stop: CancelToken,
}

impl Tickets {
    pub fn logged_on(&mut self, public_ip: Option<IpAddr>) {
        self.logon = Some(Instant::now());
        self.public_ip = ipv4_word(public_ip);
        self.made = 0;
    }

    /// The CM session ended: its tokens, sequence and ownership tickets
    /// go with it. The list stays, to be sent again after the next
    /// logon. Returns the requests that were waiting.
    pub fn logged_off(&mut self) -> Vec<Waiting> {
        self.tokens.clear();
        self.seq_sent = 0;
        self.seq_acked = 0;
        self.logon = None;
        self.ownership.clear();
        self.fetching.clear();
        std::mem::take(&mut self.waiting)
    }

    /// `ClientGameConnectTokens` (§2.1).
    pub fn add_tokens(&mut self, tokens: Vec<Vec<u8>>, keep: u32) {
        self.tokens.extend(tokens);
        while self.tokens.len() > keep as usize {
            self.tokens.pop_front();
        }
    }

    pub fn has_token(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// Whether the ticket [`Self::make`] would make fits in `max` bytes;
    /// 0 is no limit.
    pub fn fits(&self, ownership: &[u8], max: u32) -> bool {
        let token = self.tokens.front().map_or(0, Vec::len);
        let size = 4 + token + 4 + HEADER as usize + 4 + ownership.len();
        max == 0 || size <= max as usize
    }

    /// Makes an own ticket from the oldest token and `ownership`, and adds
    /// it to the list. `None` without a token.
    pub fn make(&mut self, w: &Waiting, ownership: &[u8]) -> Option<(u32, Vec<u8>)> {
        let token = self.tokens.pop_front()?;
        self.made = self.made.wrapping_add(1);
        let ms = self
            .logon
            .map_or(0, |t| t.elapsed().as_millis() as u64 as u32);
        let auth = auth_ticket(&token, w.kind, self.public_ip, ms, self.made);
        let crc = crc(&auth);
        let ticket = full_ticket(&auth, ownership);
        let secret = (w.kind == WEB_API).then(|| format!("str:{}\0", w.identity).into_bytes());
        self.entries.push(Entry {
            conn: w.conn,
            app: w.app,
            steam_id: 0,
            crc,
            auth,
            secret,
            ownership: Vec::new(),
            active: false,
        });
        Some((crc, ticket))
    }

    /// Removes the entries for which `drop` is true. Returns how many it
    /// removed and the apps left without a ticket, for [`Self::list`].
    pub fn remove(&mut self, mut drop: impl FnMut(&Entry) -> bool) -> (usize, Vec<AppId>) {
        let count = self.entries.len();
        let before: HashSet<AppId> = self.entries.iter().map(|e| e.app).collect();
        self.entries.retain(|e| !drop(e));
        let after: HashSet<AppId> = self.entries.iter().map(|e| e.app).collect();
        let mut gone: Vec<AppId> = before.difference(&after).copied().collect();
        gone.sort();
        (count - self.entries.len(), gone)
    }

    /// The next `ClientAuthList` (§2.4), naming also the apps in `gone`.
    pub fn list(&mut self, gone: &[AppId]) -> CMsgClientAuthList {
        let mut app_ids: Vec<u32> = Vec::new();
        for a in self
            .entries
            .iter()
            .map(|e| e.app)
            .chain(gone.iter().copied())
        {
            if !app_ids.contains(&a.0) {
                app_ids.push(a.0);
            }
        }
        let previous = self.seq_sent;
        self.seq_sent = self.seq_sent.wrapping_add(1);
        CMsgClientAuthList {
            tokens_left: Some(self.tokens.len() as u32),
            last_request_seq: Some(previous),
            last_request_seq_from_server: Some(self.seq_acked),
            tickets: self
                .entries
                .iter()
                .map(|e| CMsgAuthTicket {
                    estate: Some(if e.own() { 0 } else { 1 }),
                    steamid: Some(e.steam_id),
                    gameid: Some(u64::from(e.app.0)),
                    h_steam_pipe: Some(e.conn as u32),
                    ticket_crc: Some(e.crc),
                    ticket: Some(e.auth.clone()),
                    server_secret: e.secret.clone(),
                    ..Default::default()
                })
                .collect(),
            app_ids,
            message_sequence: Some(self.seq_sent),
            ..Default::default()
        }
    }

    /// `ClientAuthListAck`: the own tickets that became active.
    pub fn acked(&mut self, crcs: &[u32], sequence: Option<u32>) -> Vec<(ConnId, u32)> {
        if let Some(s) = sequence {
            self.seq_acked = s;
        }
        let mut now = Vec::new();
        for e in &mut self.entries {
            if e.own() && !e.active && crcs.contains(&e.crc) {
                e.active = true;
                now.push((e.conn, e.crc));
            }
        }
        now
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An ownership ticket of `app` for `steam_id`, expiring at
    /// `expires`, with one package and DLC 1001 and a fake signature.
    pub fn ownership_ticket(steam_id: u64, app: u32, expires: u32) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&4u32.to_le_bytes());
        t.extend_from_slice(&steam_id.to_le_bytes());
        t.extend_from_slice(&app.to_le_bytes());
        t.extend_from_slice(&[1, 2, 3, 4]);
        t.extend_from_slice(&[0; 4]);
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&(expires - 1000).to_le_bytes());
        t.extend_from_slice(&expires.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&200u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&1001u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&201u32.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes());
        let len = t.len() as u32;
        t[..4].copy_from_slice(&len.to_le_bytes());
        t.extend_from_slice(&[0xaa; 128]);
        t
    }

    #[test]
    fn a_ticket_has_the_layout_of_section_2_3() {
        let token = [7u8; 20];
        let auth = auth_ticket(&token, SESSION, 0x0102_0304, 99, 1);
        assert_eq!(auth.len(), 4 + 20 + 4 + 24);
        assert_eq!(auth[..4], 20u32.to_le_bytes());
        assert_eq!(auth[24..28], 24u32.to_le_bytes());
        assert_eq!(auth[28..32], 1u32.to_le_bytes());
        assert_eq!(auth[32..36], 2u32.to_le_bytes());
        assert_eq!(auth[36..40], [4, 3, 2, 1]);
        let own = ownership_ticket(76561197960287930, 480, 2_000_000_000);
        let full = full_ticket(&auth, &own);
        let s = split(&full).unwrap();
        assert_eq!(s.auth, &auth[..]);
        assert_eq!(s.ownership, &own[..]);
        assert_eq!(
            ownership(s.ownership),
            Some(Ownership {
                steam_id: 76561197960287930,
                app: 480,
                expires: 2_000_000_000
            })
        );
        assert!(grants(&own, 480));
        assert!(grants(&own, 1001));
        assert!(!grants(&own, 1002));
        // Cut anywhere, it does not split.
        for n in [0, 3, 30, 56, full.len() - 1] {
            assert_eq!(split(&full[..n]), None, "{n}");
        }
        assert_eq!(ipv4_word(Some("1.2.3.4".parse().unwrap())), 0x0102_0304);
        assert_eq!(ipv4_word(Some("::1".parse().unwrap())), 0);
    }

    #[test]
    fn the_list_follows_the_tickets() {
        let mut t = Tickets::default();
        t.logged_on(None);
        let w = Waiting {
            conn: 3,
            id: 1,
            app: AppId(480),
            kind: WEB_API,
            identity: "svc".into(),
            max: 0,
        };
        assert!(t.make(&w, b"own").is_none());
        t.add_tokens(vec![vec![1; 8], vec![2; 8], vec![3; 8]], 2);
        assert!(t.fits(b"own", 4 + 8 + 4 + 24 + 4 + 3));
        assert!(!t.fits(b"own", 4 + 8 + 4 + 24 + 4 + 2));
        assert!(t.fits(b"own", 0));
        let (crc, ticket) = t.make(&w, b"own").unwrap();
        assert_eq!(ticket[4..12], [2; 8]);
        let l = t.list(&[]);
        assert_eq!(l.tokens_left, Some(1));
        assert_eq!((l.last_request_seq, l.message_sequence), (Some(0), Some(1)));
        assert_eq!(l.app_ids, [480]);
        assert_eq!(l.tickets[0].ticket_crc, Some(crc));
        assert_eq!(l.tickets[0].estate, Some(0));
        assert_eq!(
            l.tickets[0].server_secret.as_deref(),
            Some(&b"str:svc\0"[..])
        );
        assert_eq!(t.acked(&[crc, 5], Some(9)), [(3, crc)]);
        assert!(t.acked(&[crc], None).is_empty());
        let (removed, gone) = t.remove(|e| e.conn == 3);
        assert_eq!((removed, &gone[..]), (1, &[AppId(480)][..]));
        let l = t.list(&gone);
        assert!(l.tickets.is_empty());
        assert_eq!(l.app_ids, [480]);
        assert_eq!(
            (l.last_request_seq, l.last_request_seq_from_server),
            (Some(1), Some(9))
        );
        assert!(t.logged_off().is_empty());
        assert!(!t.has_token());
    }
}
