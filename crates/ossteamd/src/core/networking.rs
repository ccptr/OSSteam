// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of `ISteamNetworkingSocketsSerialized`
//! (`docs/spec/steamworks/isteamnetworkingsocketsserialized.md` §3, §5):
//! relay certificates and the relay ticket cache.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use ossteam_gameipc::{
    CacheRelayTicketRequest, Done, Message, NetworkingCert, RelayTickets, RequestBody, Response,
    ResponseBody,
};
use ossteam_net::{CallError, Packet};
use ossteam_proto::steam::{CMsgClientNetworkingCertReply, CMsgClientNetworkingCertRequest, EMsg};
use ossteam_types::{AppId, CancelToken, EResult, SteamId};
use ring::rand::SecureRandom as _;
use ring::signature::KeyPair as _;

use super::shim::error;
use super::{Command, Core};
use crate::gameipc::ConnId;

/// How long Steam has to answer a certificate request (§3).
const TIMEOUT: Duration = Duration::from_secs(15);
/// A kept certificate with more time left than this is the answer (§3
/// step 1).
const FRESH: u64 = 4 * 3600;
/// A kept certificate with more time left than this stands in when Steam
/// gives none (§3 step 4).
const USABLE: u64 = 60;
/// What fits in `SteamNetworkingSocketsCert_t` (§2).
const MAX_CERT: usize = 512;
const MAX_SIGNATURE: usize = 128;
/// The relay tickets kept per account and app, and their size (§5).
const MAX_TICKETS: usize = 64;
const MAX_TICKET: usize = 16 * 1024;

const OK: u32 = EResult::OK.0 as u32;
const FAIL: u32 = EResult::FAIL.0 as u32;
const NO_CONNECTION: u32 = EResult::NO_CONNECTION.0 as u32;
const TIMEOUT_RESULT: u32 = EResult::TIMEOUT.0 as u32;

/// The part of `CMsgSteamDatagramCertificate` (community definitions,
/// `steamnetworkingsockets_messages_certs.proto`) that says when it ends.
#[derive(Clone, PartialEq, prost::Message)]
struct CertExpiry {
    #[prost(fixed32, optional, tag = "9")]
    time_expiry: Option<u32>,
}

/// A certificate Steam gave, with its key pair.
#[derive(Clone)]
struct Cert {
    cert: Vec<u8>,
    ca_key_id: u64,
    signature: Vec<u8>,
    /// The public key, then the private key's seed.
    keys: Vec<u8>,
    /// Unix seconds; 0 when the certificate does not say.
    expires: u64,
}

impl Cert {
    fn answer(&self) -> NetworkingCert {
        NetworkingCert {
            result: OK,
            message: String::new(),
            cert: self.cert.clone(),
            ca_key_id: self.ca_key_id,
            signature: self.signature.clone(),
            private_key: self.keys.clone(),
        }
    }

    fn left(&self, now: u64) -> u64 {
        self.expires.saturating_sub(now)
    }
}

#[derive(Default)]
pub struct Networking {
    certs: HashMap<(SteamId, AppId), Cert>,
    tickets: HashMap<(SteamId, AppId), VecDeque<Vec<u8>>>,
    pub stop: CancelToken,
}

/// A certificate request's outcome, back on the core's thread.
#[derive(Debug)]
pub struct CertReply {
    conn: ConnId,
    id: u32,
    key: (SteamId, AppId),
    keys: Vec<u8>,
    reply: Result<Packet, CallError>,
}

pub fn is_networking_request(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::NetworkingCert(_)
            | RequestBody::CacheRelayTicket(_)
            | RequestBody::RelayTickets(_)
    )
}

fn failed(result: u32, message: &str) -> NetworkingCert {
    NetworkingCert {
        result,
        message: message.to_owned(),
        ..Default::default()
    }
}

/// A new Ed25519 key pair: the public key, then the private key's seed.
fn key_pair() -> Option<Vec<u8>> {
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut seed).ok()?;
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).ok()?;
    let mut keys = pair.public_key().as_ref().to_vec();
    keys.extend_from_slice(&seed);
    Some(keys)
}

impl Core {
    /// A request of §3 or §5. `None` when the answer comes later.
    pub(super) fn networking_request(
        &mut self,
        conn: ConnId,
        id: u32,
        body: RequestBody,
    ) -> Option<ResponseBody> {
        let Some(app) = self.shim_app_of(conn) else {
            return Some(error("the connection is closing"));
        };
        let account = match self.shim_owner(app) {
            Ok(a) => a,
            Err(e) => return Some(e),
        };
        let key = (account, app);
        match body {
            RequestBody::NetworkingCert(_) => self
                .cert_request(conn, id, key)
                .map(ResponseBody::NetworkingCert),
            RequestBody::CacheRelayTicket(CacheRelayTicketRequest { ticket }) => {
                if !ticket.is_empty() && ticket.len() <= MAX_TICKET {
                    let kept = self.networking.tickets.entry(key).or_default();
                    if !kept.contains(&ticket) {
                        if kept.len() == MAX_TICKETS {
                            kept.pop_front();
                        }
                        kept.push_back(ticket);
                    }
                }
                Some(ResponseBody::Done(Done {}))
            }
            RequestBody::RelayTickets(_) => Some(ResponseBody::RelayTickets(RelayTickets {
                tickets: self
                    .networking
                    .tickets
                    .get(&key)
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default(),
            })),
            _ => Some(error("not a networking request")),
        }
    }

    /// Steps 1 and 2 of §3.
    fn cert_request(
        &mut self,
        conn: ConnId,
        id: u32,
        key: (SteamId, AppId),
    ) -> Option<NetworkingCert> {
        let now = self.steam_now();
        if let Some(c) = self
            .networking
            .certs
            .get(&key)
            .filter(|c| c.left(now) > FRESH)
        {
            return Some(c.answer());
        }
        let Some(keys) = key_pair() else {
            tracing::error!("cannot make a key pair for a networking certificate");
            return Some(self.cert_fallback(key, FAIL, "no key pair"));
        };
        let Some(requester) = self
            .content
            .requester
            .clone()
            .filter(|_| self.session_state.logged_on)
        else {
            return Some(self.cert_fallback(key, NO_CONNECTION, "not logged on"));
        };
        let body = CMsgClientNetworkingCertRequest {
            key_data: Some(keys[..32].to_vec()),
            app_id: Some(key.1.0),
        };
        let packet = Packet::proto(EMsg::KEMsgClientNetworkingCertRequest, &body);
        let commands = self.games.commands.clone();
        let stop = self.networking.stop.clone();
        let spawned = std::thread::Builder::new()
            .name("cert-request".into())
            .spawn(move || {
                let reply = requester.request(packet, TIMEOUT, &stop);
                let _ = commands.send(Command::Networking(CertReply {
                    conn,
                    id,
                    key,
                    keys,
                    reply,
                }));
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start a thread for a certificate request: {e}");
            return Some(self.cert_fallback(key, FAIL, "no thread for the request"));
        }
        None
    }

    /// Step 4 of §3: the kept certificate if it is still usable, else the
    /// failure.
    fn cert_fallback(&self, key: (SteamId, AppId), result: u32, message: &str) -> NetworkingCert {
        let now = self.steam_now();
        match self
            .networking
            .certs
            .get(&key)
            .filter(|c| c.left(now) > USABLE)
        {
            Some(c) => c.answer(),
            None => failed(result, message),
        }
    }

    /// Step 3 of §3.
    pub(super) fn on_cert_reply(&mut self, r: CertReply) {
        let (app, conn, id, key) = (r.key.1, r.conn, r.id, r.key);
        let answer = match r.reply {
            Ok(p) => {
                let result = p.header_eresult().map_or(OK, |e| e.0 as u32);
                let message = p
                    .proto_header()
                    .and_then(|h| h.error_message.clone())
                    .unwrap_or_default();
                match p.decode_body::<CMsgClientNetworkingCertReply>() {
                    Ok(body) if result == OK => self.keep_cert(key, body, r.keys),
                    Ok(_) => Err((result, message)),
                    Err(e) => Err((FAIL, format!("the reply does not decode: {e}"))),
                }
            }
            Err(CallError::Timeout) => Err((TIMEOUT_RESULT, "Steam did not answer".to_owned())),
            Err(CallError::Closed | CallError::Cancelled) => {
                Err((NO_CONNECTION, "the connection to Steam closed".to_owned()))
            }
            Err(e) => Err((FAIL, e.to_string())),
        };
        let answer = match answer {
            Ok(c) => {
                tracing::info!(app = %app, "Steam gave a networking certificate");
                c
            }
            Err((result, message)) => {
                tracing::info!(app = %app, result, "no networking certificate: {message}");
                self.cert_fallback(key, result, &message)
            }
        };
        self.shim_send(
            conn,
            Message::Response(Response {
                id,
                body: ResponseBody::NetworkingCert(answer),
            }),
        );
    }

    /// Keeps a certificate that fits the callback (§3).
    fn keep_cert(
        &mut self,
        key: (SteamId, AppId),
        body: CMsgClientNetworkingCertReply,
        keys: Vec<u8>,
    ) -> Result<NetworkingCert, (u32, String)> {
        let cert = body.cert.unwrap_or_default();
        let signature = body.ca_signature.unwrap_or_default();
        if cert.is_empty() || cert.len() > MAX_CERT || signature.len() > MAX_SIGNATURE {
            return Err((
                FAIL,
                format!(
                    "a certificate of {} bytes with a signature of {}",
                    cert.len(),
                    signature.len()
                ),
            ));
        }
        let expires = <CertExpiry as prost::Message>::decode(cert.as_slice())
            .ok()
            .and_then(|c| c.time_expiry)
            .map_or(0, u64::from);
        let c = Cert {
            cert,
            ca_key_id: body.ca_key_id.unwrap_or(0),
            signature,
            keys,
            expires,
        };
        let answer = c.answer();
        self.networking.certs.insert(key, c);
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_pairs_hold_the_public_key_then_the_seed() {
        let keys = key_pair().expect("a key pair");
        assert_eq!(keys.len(), 64);
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&keys[32..]).unwrap();
        assert_eq!(pair.public_key().as_ref(), &keys[..32]);
    }
}
