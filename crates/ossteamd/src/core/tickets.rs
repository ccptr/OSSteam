// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of games' tickets (`docs/spec/steamworks/isteamuser.md`
//! §2, §3): the requests, the CM messages about tickets, and the timers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ossteam_gameipc::{
    AuthTicket, BeginAuthSession, BeginAuthSessionRequest, Done, EncryptedAppTicket, Message,
    NoticeBody, RequestBody, Response, ResponseBody, TicketActive, TicketValidated, UserLicense,
};
use ossteam_net::{CallError, Packet};
use ossteam_proto::Message as _;
use ossteam_proto::steam::{
    CMsgClientAuthListAck, CMsgClientGameConnectTokens, CMsgClientGetAppOwnershipTicket,
    CMsgClientGetAppOwnershipTicketResponse, CMsgClientRequestEncryptedAppTicket,
    CMsgClientRequestEncryptedAppTicketResponse, CMsgClientTicketAuthComplete, EMsg,
};
use ossteam_types::{AppId, EResult};

use super::shim::error;
use super::{Command, Core};
use crate::gameipc::ConnId;
use crate::tickets::{self as auth, Event, Waiting};

/// How long a ticket request waits for a token or an ownership ticket
/// (§3).
const WAIT: Duration = Duration::from_secs(4);
/// How long Steam has to acknowledge an own ticket (§2.4).
const ACK_WAIT: Duration = Duration::from_secs(10);
/// How long a CM request may take (§2.2, §2.7).
const TIMEOUT: Duration = Duration::from_secs(15);
/// An ownership ticket closer than this to its expiry is fetched again
/// (§2.2).
const REFRESH: u64 = 6 * 3600;
/// The ticket sizes `BeginAuthSession` accepts.
const MAX_TICKET: usize = 4096;

const OK: u32 = EResult::OK.0 as u32;
const FAIL: u32 = EResult::FAIL.0 as u32;
const NO_CONNECTION: u32 = EResult::NO_CONNECTION.0 as u32;
const TIMEOUT_RESULT: u32 = EResult::TIMEOUT.0 as u32;

/// The requests of §3.
pub fn is_ticket_request(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::AuthTicket(_)
            | RequestBody::CancelAuthTicket(_)
            | RequestBody::BeginAuthSession(_)
            | RequestBody::EndAuthSession(_)
            | RequestBody::UserLicense(_)
            | RequestBody::EncryptedAppTicket(_)
    )
}

fn ticket_answer(result: u32) -> ResponseBody {
    ResponseBody::AuthTicket(AuthTicket {
        result,
        ..Default::default()
    })
}

impl Core {
    /// A ticket request (§3). `None` when the answer comes later.
    pub(super) fn ticket_request(
        &mut self,
        conn: ConnId,
        id: u32,
        body: RequestBody,
    ) -> Option<ResponseBody> {
        let Some(app) = self.shim_app_of(conn) else {
            return Some(error("the connection is closing"));
        };
        if let Err(e) = self.shim_owner(app) {
            return Some(e);
        }
        match body {
            RequestBody::AuthTicket(r) => {
                if r.kind != auth::SESSION && r.kind != auth::WEB_API {
                    return Some(error(&format!("no ticket of kind {}", r.kind)));
                }
                if !self.session_state.logged_on {
                    return Some(ticket_answer(NO_CONNECTION));
                }
                self.tickets.waiting.push(Waiting {
                    conn,
                    id,
                    app,
                    kind: r.kind,
                    identity: r.identity,
                    max: r.max,
                });
                self.serve_tickets();
                if self
                    .tickets
                    .waiting
                    .iter()
                    .any(|w| w.conn == conn && w.id == id)
                {
                    self.ticket_timer(WAIT, Event::WaitOver { conn, id });
                }
                None
            }
            RequestBody::CancelAuthTicket(r) => {
                let (removed, gone) = self
                    .tickets
                    .remove(|e| e.conn == conn && e.own() && e.crc == r.crc);
                if removed > 0 {
                    self.send_auth_list(&gone);
                }
                Some(ResponseBody::Done(Done {}))
            }
            RequestBody::BeginAuthSession(r) => {
                let result = self.begin_auth_session(conn, app, r);
                Some(ResponseBody::BeginAuthSession(BeginAuthSession { result }))
            }
            RequestBody::EndAuthSession(r) => {
                let (removed, gone) = self
                    .tickets
                    .remove(|e| e.conn == conn && !e.own() && e.steam_id == r.steam_id);
                if removed > 0 {
                    self.send_auth_list(&gone);
                }
                Some(ResponseBody::Done(Done {}))
            }
            RequestBody::UserLicense(r) => {
                let session = self
                    .tickets
                    .entries
                    .iter()
                    .find(|e| e.conn == conn && !e.own() && e.steam_id == r.steam_id);
                let result = match session {
                    None => auth::LICENSE_NO_AUTH,
                    Some(e) if auth::grants(&e.ownership, r.app_id) => auth::LICENSE_YES,
                    Some(_) => auth::LICENSE_NO,
                };
                Some(ResponseBody::UserLicense(UserLicense { result }))
            }
            RequestBody::EncryptedAppTicket(r) => {
                let body = CMsgClientRequestEncryptedAppTicket {
                    app_id: Some(app.0),
                    userdata: Some(r.userdata),
                };
                let packet = Packet::proto(EMsg::KEMsgClientRequestEncryptedAppTicket, &body);
                let started = self.ticket_call(packet, move |reply| Event::Encrypted {
                    conn,
                    id,
                    answer: encrypted_answer(reply),
                });
                if started {
                    None
                } else {
                    Some(ResponseBody::EncryptedAppTicket(EncryptedAppTicket {
                        result: NO_CONNECTION,
                        ticket: Vec::new(),
                    }))
                }
            }
            _ => Some(error("not a ticket request")),
        }
    }

    /// The checks of §2.6, and the ticket's place in the list.
    fn begin_auth_session(&mut self, conn: ConnId, app: AppId, r: BeginAuthSessionRequest) -> u32 {
        if !self.session_state.logged_on {
            tracing::info!(conn, "BeginAuthSession while not logged on");
            return auth::BEGIN_INVALID;
        }
        let Some(split) = auth::split(&r.ticket).filter(|_| r.ticket.len() <= MAX_TICKET) else {
            return auth::BEGIN_INVALID;
        };
        let Some(o) = auth::ownership(split.ownership) else {
            return auth::BEGIN_INVALID;
        };
        if o.steam_id != r.steam_id || r.steam_id == 0 {
            return auth::BEGIN_INVALID;
        }
        if o.app != app.0 {
            return auth::BEGIN_GAME_MISMATCH;
        }
        if u64::from(o.expires) <= self.steam_now() {
            return auth::BEGIN_EXPIRED;
        }
        if self
            .tickets
            .entries
            .iter()
            .any(|e| e.conn == conn && e.steam_id == r.steam_id)
        {
            return auth::BEGIN_DUPLICATE;
        }
        self.tickets.entries.push(auth::Entry {
            conn,
            app,
            steam_id: r.steam_id,
            crc: auth::crc(split.auth),
            auth: split.auth.to_vec(),
            secret: None,
            ownership: split.ownership.to_vec(),
            active: false,
        });
        self.send_auth_list(&[]);
        auth::BEGIN_OK
    }

    /// Makes the tickets that can be made now, and asks for the ownership
    /// tickets they wait for.
    fn serve_tickets(&mut self) {
        let now = self.steam_now();
        let mut made = false;
        for w in std::mem::take(&mut self.tickets.waiting) {
            let fresh = self
                .tickets
                .ownership
                .get(&w.app)
                .filter(|t| {
                    auth::ownership(t).is_some_and(|o| u64::from(o.expires) > now + REFRESH)
                })
                .cloned();
            let Some(own) = fresh else {
                self.fetch_ownership(w.app);
                self.tickets.waiting.push(w);
                continue;
            };
            if !self.tickets.has_token() {
                self.tickets.waiting.push(w);
                continue;
            }
            if !self.tickets.fits(&own, w.max) {
                tracing::info!(
                    conn = w.conn,
                    max = w.max,
                    "the game's buffer is too small for a ticket"
                );
                self.ticket_reply(w.conn, w.id, ticket_answer(FAIL));
                continue;
            }
            let Some((crc, ticket)) = self.tickets.make(&w, &own) else {
                continue;
            };
            made = true;
            tracing::info!(conn = w.conn, app = %w.app, crc, kind = w.kind, "made a ticket");
            self.ticket_reply(
                w.conn,
                w.id,
                ResponseBody::AuthTicket(AuthTicket {
                    result: OK,
                    crc,
                    ticket,
                }),
            );
            self.ticket_timer(ACK_WAIT, Event::AckOver { conn: w.conn, crc });
        }
        if made {
            self.send_auth_list(&[]);
        }
    }

    /// Asks for `app`'s ownership ticket unless that is under way.
    fn fetch_ownership(&mut self, app: AppId) {
        let Ok(account) = self.shim_owner(app) else {
            return;
        };
        if !self.session_state.logged_on || self.tickets.fetching.contains(&app) {
            return;
        }
        let body = CMsgClientGetAppOwnershipTicket {
            app_id: Some(app.0),
        };
        let packet = Packet::proto(EMsg::KEMsgClientGetAppOwnershipTicket, &body);
        let started = self.ticket_call(packet, move |reply| Event::Fetched {
            account,
            app,
            ticket: ownership_answer(reply),
        });
        if started {
            self.tickets.fetching.insert(app);
        }
    }

    /// Sends `packet` as a CM request on a thread of its own; its reply
    /// comes back as the event `done` makes. False when the account is not
    /// logged on.
    fn ticket_call(
        &self,
        packet: Packet,
        done: impl FnOnce(Result<Packet, CallError>) -> Event + Send + 'static,
    ) -> bool {
        let Some(requester) = self
            .content
            .requester
            .clone()
            .filter(|_| self.session_state.logged_on)
        else {
            return false;
        };
        let commands = self.games.commands.clone();
        let stop = self.tickets.stop.clone();
        let spawned = std::thread::Builder::new()
            .name("ticket-request".into())
            .spawn(move || {
                let reply = requester.request(packet, TIMEOUT, &stop);
                let _ = commands.send(Command::Tickets(done(reply)));
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start a thread for a ticket request: {e}");
            return false;
        }
        true
    }

    fn ticket_timer(&self, after: Duration, event: Event) {
        let commands = self.games.commands.clone();
        let spawned = std::thread::Builder::new()
            .name("ticket-timer".into())
            .spawn(move || {
                std::thread::sleep(after);
                let _ = commands.send(Command::Tickets(event));
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start a ticket timer: {e}");
        }
    }

    fn ticket_reply(&mut self, conn: ConnId, id: u32, body: ResponseBody) {
        self.shim_send(conn, Message::Response(Response { id, body }));
    }

    fn ticket_notice(&mut self, conn: ConnId, notice: NoticeBody) {
        self.shim_send(conn, Message::Notice(notice));
    }

    /// Sends the auth list (§2.4), if the account is logged on; otherwise
    /// it goes after the next logon.
    fn send_auth_list(&mut self, gone: &[AppId]) {
        let Some(requester) = self
            .content
            .requester
            .clone()
            .filter(|_| self.session_state.logged_on)
        else {
            return;
        };
        let list = self.tickets.list(gone);
        let count = list.tickets.len();
        match requester.send_job(Packet::proto(EMsg::KEMsgClientAuthList, &list)) {
            Ok(_) => tracing::debug!(tickets = count, "auth list sent"),
            Err(e) => tracing::info!("could not send the auth list: {e}"),
        }
    }

    /// Steam's clock now, in Unix seconds; the local clock before the
    /// first logon.
    fn steam_now(&self) -> u64 {
        match self.shims.clock {
            Some((t, at)) => u64::from(t) + at.elapsed().as_secs(),
            None => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }

    pub(super) fn tickets_logged_on(&mut self, public_ip: Option<std::net::IpAddr>) {
        self.tickets.logged_on(public_ip);
        for app in self.shim_apps() {
            self.fetch_ownership(app);
        }
        if !self.tickets.entries.is_empty() {
            self.send_auth_list(&[]);
        }
    }

    pub(super) fn tickets_logged_off(&mut self) {
        for w in self.tickets.logged_off() {
            self.ticket_reply(w.conn, w.id, ticket_answer(NO_CONNECTION));
        }
    }

    pub(super) fn ticket_game_connected(&mut self, app: AppId) {
        self.fetch_ownership(app);
    }

    pub(super) fn ticket_game_disconnected(&mut self, conn: ConnId) {
        self.tickets.waiting.retain(|w| w.conn != conn);
        let (removed, gone) = self.tickets.remove(|e| e.conn == conn);
        if removed > 0 {
            tracing::debug!(conn, removed, "dropping a closed game's tickets");
            self.send_auth_list(&gone);
        }
    }

    pub(super) fn on_ticket_event(&mut self, e: Event) {
        match e {
            Event::Fetched {
                account,
                app,
                ticket,
            } => {
                self.tickets.fetching.remove(&app);
                let current = self.shim_owner(app).ok() == Some(account);
                match ticket {
                    Ok(t) if current && self.session_state.logged_on => {
                        tracing::debug!(app = %app, "ownership ticket received");
                        self.tickets.ownership.insert(app, t);
                        self.serve_tickets();
                    }
                    Ok(_) => {}
                    Err(why) => {
                        tracing::warn!(app = %app, "no ownership ticket: {why}");
                        let (failed, kept) = std::mem::take(&mut self.tickets.waiting)
                            .into_iter()
                            .partition(|w| w.app == app);
                        self.tickets.waiting = kept;
                        for w in failed {
                            self.ticket_reply(w.conn, w.id, ticket_answer(FAIL));
                        }
                    }
                }
            }
            Event::Encrypted { conn, id, answer } => {
                self.ticket_reply(conn, id, ResponseBody::EncryptedAppTicket(answer));
            }
            Event::WaitOver { conn, id } => {
                let Some(at) = self
                    .tickets
                    .waiting
                    .iter()
                    .position(|w| w.conn == conn && w.id == id)
                else {
                    return;
                };
                self.tickets.waiting.remove(at);
                tracing::info!(conn, "no game connect token or ownership ticket in time");
                self.ticket_reply(conn, id, ticket_answer(FAIL));
            }
            Event::AckOver { conn, crc } => {
                let late = self
                    .tickets
                    .entries
                    .iter()
                    .any(|e| e.conn == conn && e.own() && e.crc == crc && !e.active);
                if late {
                    tracing::info!(conn, crc, "Steam did not acknowledge the ticket in time");
                    self.ticket_notice(
                        conn,
                        NoticeBody::TicketActive(TicketActive {
                            crc,
                            result: TIMEOUT_RESULT,
                        }),
                    );
                }
            }
        }
    }

    /// `ClientGameConnectTokens`, `ClientAuthListAck` and
    /// `ClientTicketAuthComplete` (§2.1, §2.4, §2.5).
    pub(super) fn on_ticket_message(&mut self, p: &Packet) {
        if p.is(EMsg::KEMsgClientGameConnectTokens) {
            let Ok(body) = p.decode_body::<CMsgClientGameConnectTokens>() else {
                tracing::debug!("dropping an undecodable ClientGameConnectTokens");
                return;
            };
            tracing::debug!(tokens = body.tokens.len(), "game connect tokens");
            let keep = body.max_tokens_to_keep();
            self.tickets.add_tokens(body.tokens, keep);
            self.serve_tickets();
        } else if p.is(EMsg::KEMsgClientAuthListAck) {
            let Ok(body) = p.decode_body::<CMsgClientAuthListAck>() else {
                tracing::debug!("dropping an undecodable ClientAuthListAck");
                return;
            };
            for (conn, crc) in self.tickets.acked(&body.ticket_crc, body.message_sequence) {
                self.ticket_notice(
                    conn,
                    NoticeBody::TicketActive(TicketActive { crc, result: OK }),
                );
            }
        } else if p.is(EMsg::KEMsgClientTicketAuthComplete) {
            let Ok(body) = p.decode_body::<CMsgClientTicketAuthComplete>() else {
                tracing::debug!("dropping an undecodable ClientTicketAuthComplete");
                return;
            };
            let (steam_id, crc, response) = (
                body.steam_id(),
                body.ticket_crc(),
                body.eauth_session_response(),
            );
            let Some(e) = self
                .tickets
                .entries
                .iter()
                .find(|e| !e.own() && e.crc == crc && e.steam_id == steam_id)
            else {
                tracing::debug!(crc, response, "Steam's verdict on an own or unknown ticket");
                return;
            };
            let conn = e.conn;
            self.ticket_notice(
                conn,
                NoticeBody::TicketValidated(TicketValidated {
                    steam_id,
                    owner: body.owner_steam_id(),
                    response,
                }),
            );
            if response != 0 {
                self.tickets
                    .remove(|e| e.conn == conn && e.crc == crc && e.steam_id == steam_id);
            }
        }
    }
}

fn ownership_answer(reply: Result<Packet, CallError>) -> Result<Vec<u8>, String> {
    let p = reply.map_err(|e| e.to_string())?;
    let body = p
        .decode_body::<CMsgClientGetAppOwnershipTicketResponse>()
        .map_err(|e| format!("undecodable reply: {e}"))?;
    match body.eresult() {
        1 => {}
        r => return Err(format!("Steam answered {}", EResult(r as i32))),
    }
    let ticket = body.ticket.unwrap_or_default();
    if auth::ownership(&ticket).is_none() {
        return Err("the ticket is too short".into());
    }
    Ok(ticket)
}

fn encrypted_answer(reply: Result<Packet, CallError>) -> EncryptedAppTicket {
    let failed = |result| EncryptedAppTicket {
        result,
        ticket: Vec::new(),
    };
    let p = match reply {
        Ok(p) => p,
        Err(CallError::Closed | CallError::Cancelled) => return failed(NO_CONNECTION),
        Err(CallError::Failed { eresult, .. }) => return failed(eresult.0 as u32),
        Err(e) => {
            tracing::info!("encrypted app ticket: {e}");
            return failed(FAIL);
        }
    };
    let Ok(body) = p.decode_body::<CMsgClientRequestEncryptedAppTicketResponse>() else {
        return failed(FAIL);
    };
    let result = body.eresult() as u32;
    match body.encrypted_app_ticket {
        Some(t) if result == OK => EncryptedAppTicket {
            result,
            ticket: t.encode_to_vec(),
        },
        _ if result == OK => failed(FAIL),
        _ => failed(result),
    }
}
