// SPDX-License-Identifier: GPL-2.0-or-later
//! A fake CM for the daemon integration tests (SPEC §19.1 level 3). Test
//! code only (FAITH-4).
//!
//! It answers the messages M1a to M1c send, as `cm.md`, `auth.md`,
//! `pics.md`, `content.md` and `launch.md` describe them: the sign-in
//! service methods, logon and logoff, the license list, PICS, persona
//! state, depot keys, manifest request codes, the content server list, CDN
//! auth tokens and play sessions. It checks what the daemon sends
//! (the password is decrypted, codes and tokens are compared) so a test
//! fails when the daemon gets the protocol wrong. Where those pages mark a
//! server behaviour as unverified, the fake picks one reading; the
//! comments say so.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use crossbeam_channel::Sender;
use ossteam_content::download::Tokens;
use ossteam_content::fake::{Depot, FakeCdn};
use ossteam_net::conn::method_name;
use ossteam_net::{CmKind, CmServer, ConnectError, Connection, Incoming, Packet};
use ossteam_proto::ServiceMethod;
use ossteam_proto::steam::{
    CAuthenticationAllowedConfirmation, CAuthenticationBeginAuthSessionViaCredentialsRequest,
    CAuthenticationBeginAuthSessionViaCredentialsResponse,
    CAuthenticationBeginAuthSessionViaQrResponse, CAuthenticationGetPasswordRsaPublicKeyRequest,
    CAuthenticationGetPasswordRsaPublicKeyResponse, CAuthenticationPollAuthSessionStatusRequest,
    CAuthenticationPollAuthSessionStatusResponse,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse,
    CContentServerDirectoryGetCdnAuthTokenRequest, CContentServerDirectoryGetCdnAuthTokenResponse,
    CContentServerDirectoryGetManifestRequestCodeRequest,
    CContentServerDirectoryGetManifestRequestCodeResponse,
    CContentServerDirectoryGetServersForSteamPipeRequest,
    CContentServerDirectoryGetServersForSteamPipeResponse, CContentServerDirectoryServerInfo,
    CMsgClientAccountInfo, CMsgClientChangeStatus, CMsgClientGamesPlayed,
    CMsgClientGetDepotDecryptionKey, CMsgClientGetDepotDecryptionKeyResponse,
    CMsgClientLicenseList, CMsgClientLoggedOff, CMsgClientLogon, CMsgClientLogonResponse,
    CMsgClientPicsAccessTokenRequest, CMsgClientPicsAccessTokenResponse,
    CMsgClientPicsChangesSinceRequest, CMsgClientPicsChangesSinceResponse,
    CMsgClientPicsProductInfoRequest, CMsgClientPicsProductInfoResponse, EAuthSessionGuardType,
    EMsg, authentication, c_msg_client_license_list, c_msg_client_pics_access_token_response,
    c_msg_client_pics_changes_since_response, c_msg_client_pics_product_info_response,
    content_server_directory as csd,
};
use ossteam_types::{CancelToken, EResult, SteamId};
use ossteam_vdf::{Object, Value, binary, text};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};

use super::loopback::{self, ServerEnd};
use crate::session::{Dialer, NoServer};

/// The fake server's address. Nothing listens there; the dialer connects
/// over [`loopback`].
pub const ENDPOINT: &str = "fake-cm.invalid:443";
/// What the fake tells the daemon to poll at, in seconds. The daemon
/// floors it at 100 ms.
const POLL_INTERVAL: f32 = 0.1;
const HEARTBEAT_SECONDS: i32 = 9;

/// The one content server the fake lists. Nothing resolves it: the
/// daemon's downloads go to a [`FakeCdn`].
pub const CONTENT_SERVER: &str = "cdn.fake.invalid";

/// How the fake asks for Steam Guard (`auth.md` §2 step 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    None,
    EmailCode(String),
    DeviceCode(String),
    /// Approval in the mobile app; see [`FakeCm::approve`].
    DeviceConfirmation,
}

#[derive(Clone, Debug)]
pub struct Account {
    pub name: String,
    pub password: String,
    pub steam_id: SteamId,
    pub persona_name: String,
    pub guard: Guard,
    /// Licensed packages, with their license flags.
    pub licenses: Vec<(u32, u32)>,
}

impl Account {
    pub fn new(name: &str, account_id: u32) -> Self {
        Self {
            name: name.into(),
            password: format!("{name}-password"),
            steam_id: SteamId::individual(account_id),
            persona_name: format!("{name} persona"),
            guard: Guard::None,
            licenses: Vec::new(),
        }
    }

    pub fn guard(mut self, guard: Guard) -> Self {
        self.guard = guard;
        self
    }

    pub fn license(mut self, package: u32, flags: u32) -> Self {
        self.licenses.push((package, flags));
        self
    }
}

#[derive(Clone, Debug)]
struct App {
    name: String,
    app_type: String,
    parent: Option<u32>,
    change_number: u32,
    /// Further sections of the app's info, such as `depots` and `config`.
    sections: Vec<(String, Object)>,
}

struct AuthSession {
    request_id: Vec<u8>,
    /// Unknown for a QR session until it is approved.
    account: Option<String>,
    approved: bool,
    qr: bool,
    /// QR challenges still to rotate, one per poll.
    rotations: u32,
}

#[derive(Default)]
struct State {
    accounts: Vec<Account>,
    packages: BTreeMap<u32, (Vec<u32>, u32)>,
    apps: BTreeMap<u32, App>,
    change_number: u32,
    sessions: HashMap<u64, AuthSession>,
    next_id: u64,
    /// Refresh token → account name, for tokens that were issued and not
    /// revoked.
    tokens: HashMap<String, String>,
    /// Guard data → account name.
    guard_data: HashMap<String, String>,
    qr_rotations: u32,
    /// Every logon attempt, by the account name in it.
    logons: Vec<String>,
    /// `ClientChangeStatus` persona states, in arrival order.
    persona_states: Vec<(SteamId, u32)>,
    /// `ClientGamesPlayedWithDataBlob` messages, in arrival order.
    games_played: Vec<(SteamId, CMsgClientGamesPlayed)>,
    /// Asks every live connection to drop.
    generation: u64,
    /// Depot keys Steam grants.
    depot_keys: HashMap<u32, [u8; 32]>,
    /// Manifest request codes, by depot and manifest.
    request_codes: HashMap<(u32, u64), u64>,
    cdn: Option<Arc<FakeCdn>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    rsa: &'static RsaPrivateKey,
}

/// One fake CM. Clones share it.
#[derive(Clone)]
pub struct FakeCm {
    shared: Arc<Shared>,
}

/// Generated once: key generation is slow in debug builds.
fn rsa_key() -> &'static RsaPrivateKey {
    static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
    KEY.get_or_init(|| RsaPrivateKey::new(&mut rand_core::OsRng, 1024).expect("RSA key"))
}

impl FakeCm {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    change_number: 1000,
                    next_id: 1,
                    ..Default::default()
                }),
                changed: Condvar::new(),
                rsa: rsa_key(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn notify(&self) {
        self.shared.changed.notify_all();
    }

    pub fn account(self, account: Account) -> Self {
        self.state().accounts.push(account);
        self
    }

    /// A package granting `apps`.
    pub fn package(self, id: u32, apps: &[u32]) -> Self {
        {
            let mut s = self.state();
            let n = s.change_number;
            s.packages.insert(id, (apps.to_vec(), n));
        }
        self
    }

    pub fn app(self, id: u32, name: &str, app_type: &str, parent: Option<u32>) -> Self {
        {
            let mut s = self.state();
            let change_number = s.change_number;
            s.apps.insert(
                id,
                App {
                    name: name.into(),
                    app_type: app_type.into(),
                    parent,
                    change_number,
                    sections: Vec::new(),
                },
            );
        }
        self
    }

    /// How many times each QR session rotates its challenge before it can
    /// be approved.
    pub fn qr_rotations(self, n: u32) -> Self {
        self.state().qr_rotations = n;
        self
    }

    pub fn dialer(&self) -> Box<dyn Dialer> {
        Box::new(FakeDialer { cm: self.clone() })
    }

    /// Renames an app in a new PICS change.
    pub fn rename_app(&self, id: u32, name: &str) {
        let mut s = self.state();
        s.change_number += 1;
        let n = s.change_number;
        let app = s.apps.get_mut(&id).expect("a known app");
        app.name = name.into();
        app.change_number = n;
    }

    /// Sets a section of an app's info (`depots`, `config`, ...) in a new
    /// PICS change.
    pub fn app_section(&self, id: u32, key: &str, section: Object) {
        let mut s = self.state();
        s.change_number += 1;
        let n = s.change_number;
        let app = s.apps.get_mut(&id).expect("a known app");
        app.sections.retain(|(k, _)| k != key);
        app.sections.push((key.into(), section));
        app.change_number = n;
    }

    /// Serves content from `cdn`: the content server list names
    /// [`CONTENT_SERVER`], and CDN auth tokens are the ones `cdn` wants.
    pub fn cdn(self, cdn: Arc<FakeCdn>) -> Self {
        self.state().cdn = Some(cdn);
        self
    }

    /// Publishes a depot's manifest under `code` on the CDN, and grants
    /// its key.
    pub fn publish(&self, depot: &Depot, code: u64) {
        let mut s = self.state();
        let id = depot.manifest.depot.0;
        s.depot_keys.insert(id, *depot.key.expose());
        s.request_codes.insert((id, depot.manifest.id.0), code);
        s.cdn
            .as_ref()
            .expect("FakeCm::cdn first")
            .publish(depot, code);
    }

    /// Refuses the key of `depot` from now on, as Steam does for a depot
    /// the account does not own.
    pub fn refuse_key(&self, depot: u32) {
        self.state().depot_keys.remove(&depot);
    }

    /// Approves the pending sign-in of `account` in the "mobile app": a
    /// device confirmation, or a scanned QR code.
    pub fn approve(&self, account: &str) {
        let mut s = self.state();
        let session = s
            .sessions
            .values_mut()
            .find(|a| {
                !a.approved && a.rotations == 0 && (a.qr || a.account.as_deref() == Some(account))
            })
            .expect("a pending sign-in to approve");
        session.account = Some(account.into());
        session.approved = true;
    }

    /// Revokes every refresh token of `account`.
    pub fn revoke(&self, account: &str) {
        self.state().tokens.retain(|_, a| a != account);
    }

    /// Drops every live connection, as a CM going away would.
    pub fn drop_connections(&self) {
        self.state().generation += 1;
    }

    /// Every logon attempt so far, by account name.
    pub fn logons(&self) -> Vec<String> {
        self.state().logons.clone()
    }

    /// Waits until `f` holds for the fake's state.
    pub fn wait(&self, timeout: Duration, what: &str, f: impl Fn(&FakeView) -> bool) {
        let deadline = Instant::now() + timeout;
        let mut s = self.state();
        loop {
            if f(&FakeView(&s)) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for the fake CM: {what}");
            s = self
                .shared
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Whether the account has a refresh token that the fake would accept.
    pub fn has_token(&self, account: &str) -> bool {
        self.state().tokens.values().any(|a| a == account)
    }

    fn serve(&self, mut end: ServerEnd) {
        let generation = self.state().generation;
        let mut conn = ConnState::default();
        loop {
            if self.state().generation != generation {
                end.close();
                return;
            }
            match end.recv() {
                Ok(Some(p)) => {
                    for reply in self.handle(&mut conn, &p) {
                        if end.send(&reply).is_err() {
                            return;
                        }
                    }
                    self.notify();
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    }
}

/// A read-only look at the fake's state for [`FakeCm::wait`].
pub struct FakeView<'a>(&'a State);

impl FakeView<'_> {
    pub fn persona_states(&self) -> &[(SteamId, u32)] {
        &self.0.persona_states
    }

    /// The play sessions the daemon reported, in arrival order.
    // Used by the launch tests, which run on Linux only.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn games_played(&self) -> &[(SteamId, CMsgClientGamesPlayed)] {
        &self.0.games_played
    }
}

#[derive(Default)]
struct ConnState {
    logged_on: Option<SteamId>,
    session_id: i32,
}

fn reply_to(request: &Packet, emsg: EMsg, body: &impl prost::Message) -> Packet {
    let mut p = Packet::proto(emsg, body);
    let h = p.proto_header_mut().expect("protobuf header");
    h.jobid_target = Some(request.source_job());
    p
}

/// A service method response, or a failure with `eresult` and no body
/// (`cm.md` §3.5).
fn service_reply<M: prost::Message>(request: &Packet, result: Result<M, EResult>) -> Packet {
    match result {
        Ok(body) => {
            let mut p = reply_to(request, EMsg::KEMsgServiceMethodResponse, &body);
            p.proto_header_mut().expect("protobuf header").eresult = Some(EResult::OK.0);
            p
        }
        Err(e) => {
            let mut p = reply_to(request, EMsg::KEMsgServiceMethodResponse, &());
            p.proto_header_mut().expect("protobuf header").eresult = Some(e.0);
            p
        }
    }
}

fn is<M: ServiceMethod>(name: &str) -> bool {
    name == method_name::<M>()
}

fn decode<M: prost::Message + Default>(p: &Packet) -> M {
    p.decode_body()
        .expect("the daemon sends well-formed messages")
}

impl FakeCm {
    fn handle(&self, conn: &mut ConnState, p: &Packet) -> Vec<Packet> {
        let Some(kind) = p.kind() else {
            panic!("the daemon sent unknown message {}", p.emsg);
        };
        match kind {
            EMsg::KEMsgClientHello | EMsg::KEMsgClientHeartBeat => Vec::new(),
            EMsg::KEMsgServiceMethodCallFromClientNonAuthed
            | EMsg::KEMsgServiceMethodCallFromClient => vec![self.service(p)],
            EMsg::KEMsgClientLogon => self.logon(conn, p),
            EMsg::KEMsgClientLogOff => {
                conn.logged_on = None;
                vec![Packet::proto(
                    EMsg::KEMsgClientLoggedOff,
                    &CMsgClientLoggedOff {
                        eresult: Some(EResult::OK.0),
                    },
                )]
            }
            EMsg::KEMsgClientChangeStatus => {
                let steam_id = conn.logged_on.expect("persona state before logon");
                let body: CMsgClientChangeStatus = decode(p);
                self.state()
                    .persona_states
                    .push((steam_id, body.persona_state()));
                Vec::new()
            }
            EMsg::KEMsgClientGamesPlayedWithDataBlob => {
                let steam_id = conn.logged_on.expect("play session before logon");
                let body: CMsgClientGamesPlayed = decode(p);
                self.state().games_played.push((steam_id, body));
                Vec::new()
            }
            EMsg::KEMsgClientPicsChangesSinceRequest => vec![self.changes_since(p)],
            EMsg::KEMsgClientPicsAccessTokenRequest => {
                let body: CMsgClientPicsAccessTokenRequest = decode(p);
                // No app in the fake needs a token.
                let r = CMsgClientPicsAccessTokenResponse {
                    app_access_tokens: body
                        .appids
                        .iter()
                        .map(|&a| c_msg_client_pics_access_token_response::AppToken {
                            appid: Some(a),
                            access_token: Some(0),
                        })
                        .collect(),
                    ..Default::default()
                };
                vec![reply_to(p, EMsg::KEMsgClientPicsAccessTokenResponse, &r)]
            }
            EMsg::KEMsgClientPicsProductInfoRequest => vec![self.product_info(p)],
            EMsg::KEMsgClientGetDepotDecryptionKey => {
                assert!(conn.logged_on.is_some(), "depot key before logon");
                let body: CMsgClientGetDepotDecryptionKey = decode(p);
                let key = self.state().depot_keys.get(&body.depot_id()).copied();
                let r = CMsgClientGetDepotDecryptionKeyResponse {
                    eresult: Some(key.map_or(EResult::ACCESS_DENIED, |_| EResult::OK).0),
                    depot_id: body.depot_id,
                    depot_encryption_key: key.map(|k| k.to_vec()),
                };
                vec![reply_to(
                    p,
                    EMsg::KEMsgClientGetDepotDecryptionKeyResponse,
                    &r,
                )]
            }
            other => panic!("the fake CM does not handle {other:?}"),
        }
    }

    fn service(&self, p: &Packet) -> Packet {
        let name = p
            .proto_header()
            .and_then(|h| h.target_job_name.clone())
            .expect("service calls name their method");
        if is::<authentication::GetPasswordRsaPublicKey>(&name) {
            let _: CAuthenticationGetPasswordRsaPublicKeyRequest = decode(p);
            let key = self.shared.rsa.to_public_key();
            service_reply(
                p,
                Ok(CAuthenticationGetPasswordRsaPublicKeyResponse {
                    publickey_mod: Some(key.n().to_str_radix(16)),
                    publickey_exp: Some(key.e().to_str_radix(16)),
                    timestamp: Some(77),
                }),
            )
        } else if is::<authentication::BeginAuthSessionViaCredentials>(&name) {
            service_reply(p, self.begin_credentials(&decode(p)))
        } else if is::<authentication::BeginAuthSessionViaQr>(&name) {
            let mut s = self.state();
            let client_id = s.next_id;
            s.next_id += 1;
            let rotations = s.qr_rotations;
            s.sessions.insert(
                client_id,
                AuthSession {
                    request_id: client_id.to_le_bytes().to_vec(),
                    account: None,
                    approved: false,
                    qr: true,
                    rotations,
                },
            );
            service_reply(
                p,
                Ok(CAuthenticationBeginAuthSessionViaQrResponse {
                    client_id: Some(client_id),
                    challenge_url: Some(challenge_url(client_id)),
                    request_id: Some(client_id.to_le_bytes().to_vec()),
                    interval: Some(POLL_INTERVAL),
                    ..Default::default()
                }),
            )
        } else if is::<authentication::UpdateAuthSessionWithSteamGuardCode>(&name) {
            service_reply(p, self.submit_code(&decode(p)))
        } else if is::<authentication::PollAuthSessionStatus>(&name) {
            service_reply(p, self.poll(&decode(p)))
        } else if is::<csd::GetServersForSteamPipe>(&name) {
            let _: CContentServerDirectoryGetServersForSteamPipeRequest = decode(p);
            let server = CContentServerDirectoryServerInfo {
                r#type: Some("CDN".into()),
                source_id: Some(1),
                cell_id: Some(0),
                load: Some(0),
                weighted_load: Some(0.0),
                num_entries_in_client_list: Some(1),
                host: Some(CONTENT_SERVER.into()),
                vhost: Some(CONTENT_SERVER.into()),
                https_support: Some("mandatory".into()),
                ..Default::default()
            };
            service_reply(
                p,
                Ok(CContentServerDirectoryGetServersForSteamPipeResponse {
                    servers: vec![server],
                    no_change: None,
                }),
            )
        } else if is::<csd::GetManifestRequestCode>(&name) {
            let body: CContentServerDirectoryGetManifestRequestCodeRequest = decode(p);
            // Unverified (content.md §3): what Steam answers for a manifest
            // it will not serve. The fake answers code 0.
            let code = self
                .state()
                .request_codes
                .get(&(body.depot_id(), body.manifest_id()))
                .copied()
                .unwrap_or(0);
            service_reply(
                p,
                Ok(CContentServerDirectoryGetManifestRequestCodeResponse {
                    manifest_request_code: Some(code),
                }),
            )
        } else if is::<csd::GetCdnAuthToken>(&name) {
            let body: CContentServerDirectoryGetCdnAuthTokenRequest = decode(p);
            let cdn = self.state().cdn.clone().expect("FakeCm::cdn first");
            let (token, until) = cdn
                .token(
                    ossteam_types::DepotId(body.depot_id()),
                    body.host_name(),
                    &CancelToken::new(),
                )
                .expect("the fake CDN issues tokens");
            service_reply(
                p,
                Ok(CContentServerDirectoryGetCdnAuthTokenResponse {
                    token: Some(token),
                    expiration_time: Some(until.min(u64::from(u32::MAX)) as u32),
                }),
            )
        } else {
            panic!("the fake CM does not handle {name}");
        }
    }

    fn begin_credentials(
        &self,
        req: &CAuthenticationBeginAuthSessionViaCredentialsRequest,
    ) -> Result<CAuthenticationBeginAuthSessionViaCredentialsResponse, EResult> {
        assert_eq!(req.encryption_timestamp, Some(77), "the key's timestamp");
        let encrypted = STANDARD
            .decode(req.encrypted_password())
            .expect("the password is base64");
        let password = self
            .shared
            .rsa
            .decrypt(Pkcs1v15Encrypt, &encrypted)
            .expect("the password is encrypted with the key the fake gave");
        let mut s = self.state();
        let account = s
            .accounts
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(req.account_name()))
            .cloned()
            .filter(|a| a.password.as_bytes() == password)
            .ok_or(EResult::INVALID_PASSWORD)?;
        let remembered_machine = req
            .guard_data
            .as_ref()
            .and_then(|g| s.guard_data.get(g))
            .is_some_and(|a| *a == account.name);
        let guard = if remembered_machine {
            EAuthSessionGuardType::KEAuthSessionGuardTypeMachineToken
        } else {
            match account.guard {
                Guard::None => EAuthSessionGuardType::KEAuthSessionGuardTypeNone,
                Guard::EmailCode(_) => EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode,
                Guard::DeviceCode(_) => EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode,
                Guard::DeviceConfirmation => {
                    EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceConfirmation
                }
            }
        };
        let client_id = s.next_id;
        s.next_id += 1;
        s.sessions.insert(
            client_id,
            AuthSession {
                request_id: client_id.to_le_bytes().to_vec(),
                account: Some(account.name.clone()),
                approved: matches!(
                    guard,
                    EAuthSessionGuardType::KEAuthSessionGuardTypeNone
                        | EAuthSessionGuardType::KEAuthSessionGuardTypeMachineToken
                ),
                qr: false,
                rotations: 0,
            },
        );
        let mut allowed = CAuthenticationAllowedConfirmation {
            associated_message: matches!(account.guard, Guard::EmailCode(_))
                .then(|| "example.com".to_owned()),
            ..Default::default()
        };
        allowed.set_confirmation_type(guard);
        Ok(CAuthenticationBeginAuthSessionViaCredentialsResponse {
            client_id: Some(client_id),
            request_id: Some(client_id.to_le_bytes().to_vec()),
            interval: Some(POLL_INTERVAL),
            allowed_confirmations: vec![allowed],
            steamid: Some(account.steam_id.0),
            ..Default::default()
        })
    }

    fn submit_code(
        &self,
        req: &CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest,
    ) -> Result<CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse, EResult> {
        let mut s = self.state();
        let session_account = s
            .sessions
            .get(&req.client_id())
            .and_then(|a| a.account.clone())
            .ok_or(EResult::FILE_NOT_FOUND)?;
        let account = s
            .accounts
            .iter()
            .find(|a| a.name == session_account)
            .expect("sessions name known accounts");
        assert_eq!(req.steamid(), account.steam_id.0, "the session's Steam id");
        let ok = match (&account.guard, req.code_type()) {
            (Guard::EmailCode(c), EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode) => (c
                == req.code())
            .then_some(())
            .ok_or(EResult::INVALID_LOGIN_AUTH_CODE),
            (Guard::DeviceCode(c), EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode) => (c
                == req.code())
            .then_some(())
            .ok_or(EResult::TWO_FACTOR_CODE_MISMATCH),
            _ => Err(EResult::INVALID_PARAM),
        };
        ok?;
        s.sessions
            .get_mut(&req.client_id())
            .expect("looked up above")
            .approved = true;
        Ok(Default::default())
    }

    fn poll(
        &self,
        req: &CAuthenticationPollAuthSessionStatusRequest,
    ) -> Result<CAuthenticationPollAuthSessionStatusResponse, EResult> {
        let mut s = self.state();
        let id = req.client_id();
        let session = s.sessions.get_mut(&id).ok_or(EResult::FILE_NOT_FOUND)?;
        assert_eq!(
            req.request_id(),
            session.request_id,
            "the session's request id"
        );
        if session.qr && session.rotations > 0 {
            // The challenge rotates under a new client id (`auth.md` §3).
            session.rotations -= 1;
            let session = s.sessions.remove(&id).expect("looked up above");
            let new_id = s.next_id;
            s.next_id += 1;
            s.sessions.insert(new_id, session);
            return Ok(CAuthenticationPollAuthSessionStatusResponse {
                new_client_id: Some(new_id),
                new_challenge_url: Some(challenge_url(new_id)),
                ..Default::default()
            });
        }
        if !session.approved {
            return Ok(Default::default());
        }
        let session = s.sessions.remove(&id).expect("looked up above");
        let name = session.account.expect("approved sessions name an account");
        let account = s
            .accounts
            .iter()
            .find(|a| a.name == name)
            .cloned()
            .expect("a known account");
        let serial = s.next_id;
        s.next_id += 1;
        let refresh = jwt(account.steam_id, &["client", "web"], serial);
        s.tokens.insert(refresh.clone(), name.clone());
        let new_guard_data = matches!(account.guard, Guard::EmailCode(_)).then(|| {
            let g = format!("guard-{name}-{serial}");
            s.guard_data.insert(g.clone(), name.clone());
            g
        });
        Ok(CAuthenticationPollAuthSessionStatusResponse {
            refresh_token: Some(refresh),
            access_token: Some(jwt(account.steam_id, &["web"], serial)),
            account_name: Some(account.name),
            new_guard_data,
            ..Default::default()
        })
    }

    fn logon(&self, conn: &mut ConnState, p: &Packet) -> Vec<Packet> {
        let body: CMsgClientLogon = decode(p);
        let mut s = self.state();
        s.logons.push(body.account_name().to_owned());
        // Which EResult Steam gives a revoked or unknown token is
        // unverified (`auth.md` §4); the fake uses AccessDenied.
        let account = s
            .tokens
            .get(body.access_token())
            .filter(|a| a.eq_ignore_ascii_case(body.account_name()))
            .and_then(|name| s.accounts.iter().find(|a| &a.name == name))
            .cloned();
        let Some(account) = account else {
            return vec![Packet::proto(
                EMsg::KEMsgClientLogOnResponse,
                &CMsgClientLogonResponse {
                    eresult: Some(EResult::ACCESS_DENIED.0),
                    ..Default::default()
                },
            )];
        };
        conn.session_id += 1;
        conn.logged_on = Some(account.steam_id);
        let mut response = Packet::proto(
            EMsg::KEMsgClientLogOnResponse,
            &CMsgClientLogonResponse {
                eresult: Some(EResult::OK.0),
                heartbeat_seconds: Some(HEARTBEAT_SECONDS),
                cell_id: Some(0),
                ..Default::default()
            },
        );
        let h = response.proto_header_mut().expect("protobuf header");
        h.steamid = Some(account.steam_id.0);
        h.client_sessionid = Some(conn.session_id);

        let licenses = CMsgClientLicenseList {
            eresult: Some(EResult::OK.0),
            licenses: account
                .licenses
                .iter()
                .map(|&(package, flags)| c_msg_client_license_list::License {
                    package_id: Some(package),
                    flags: Some(flags),
                    owner_id: Some(account.steam_id.account_id()),
                    access_token: Some(0),
                    ..Default::default()
                })
                .collect(),
        };
        vec![
            response,
            Packet::proto(
                EMsg::KEMsgClientAccountInfo,
                &CMsgClientAccountInfo {
                    persona_name: Some(account.persona_name.clone()),
                    ..Default::default()
                },
            ),
            Packet::proto(EMsg::KEMsgClientLicenseList, &licenses),
        ]
    }

    fn changes_since(&self, p: &Packet) -> Packet {
        let body: CMsgClientPicsChangesSinceRequest = decode(p);
        let s = self.state();
        let since = body.since_change_number();
        // Asking from 0 is answered with a full update (`pics.md` §4).
        let full = since == 0;
        let r = CMsgClientPicsChangesSinceResponse {
            current_change_number: Some(s.change_number),
            since_change_number: Some(since),
            force_full_update: Some(full),
            app_changes: if full {
                Vec::new()
            } else {
                s.apps
                    .iter()
                    .filter(|(_, a)| a.change_number > since)
                    .map(
                        |(&id, a)| c_msg_client_pics_changes_since_response::AppChange {
                            appid: Some(id),
                            change_number: Some(a.change_number),
                            needs_token: Some(false),
                        },
                    )
                    .collect()
            },
            ..Default::default()
        };
        reply_to(p, EMsg::KEMsgClientPicsChangesSinceResponse, &r)
    }

    fn product_info(&self, p: &Packet) -> Packet {
        use c_msg_client_pics_product_info_response as resp;
        let body: CMsgClientPicsProductInfoRequest = decode(p);
        let meta_only = body.meta_data_only();
        let s = self.state();
        let mut r = CMsgClientPicsProductInfoResponse {
            meta_data_only: Some(meta_only),
            response_pending: Some(false),
            ..Default::default()
        };
        for want in &body.apps {
            let id = want.appid();
            let Some(app) = s.apps.get(&id) else {
                r.unknown_appids.push(id);
                continue;
            };
            r.apps.push(resp::AppInfo {
                appid: Some(id),
                change_number: Some(app.change_number),
                missing_token: Some(false),
                buffer: (!meta_only).then(|| app_buffer(id, app)),
                ..Default::default()
            });
        }
        for want in &body.packages {
            let id = want.packageid();
            let Some((apps, change_number)) = s.packages.get(&id) else {
                r.unknown_packageids.push(id);
                continue;
            };
            r.packages.push(resp::PackageInfo {
                packageid: Some(id),
                change_number: Some(*change_number),
                missing_token: Some(false),
                buffer: (!meta_only).then(|| package_buffer(id, apps)),
                ..Default::default()
            });
        }
        reply_to(p, EMsg::KEMsgClientPicsProductInfoResponse, &r)
    }
}

fn challenge_url(client_id: u64) -> String {
    format!("https://s.team/q/1/{client_id}")
}

/// A token shaped like Steam's: a JWT whose payload names the account.
/// The signature is junk: the daemon only reads the payload (`token.rs`).
fn jwt(steam_id: SteamId, audience: &[&str], serial: u64) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"EdDSA"}"#);
    let payload = serde_json::json!({
        "sub": steam_id.0.to_string(),
        "aud": audience,
        "exp": 4_000_000_000u64,
        "jti": format!("fake-{serial}"),
    });
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{header}.{payload}.c2ln")
}

/// App info as text KeyValues with a trailing NUL (`pics.md` §3).
fn app_buffer(id: u32, app: &App) -> Vec<u8> {
    let mut common = Object::new();
    common.push("name", app.name.as_str());
    common.push("type", app.app_type.as_str());
    if let Some(parent) = app.parent {
        common.push("parent", parent.to_string());
    }
    let mut info = Object::new();
    info.push("appid", id.to_string());
    info.push("common", common);
    for (k, v) in &app.sections {
        info.push(k.as_str(), v.clone());
    }
    let mut root = Object::new();
    root.push("appinfo", info);
    let mut buf = text::to_string(&root).into_bytes();
    buf.push(0);
    buf
}

/// Package info: a 4-byte field, then binary KeyValues under the package
/// id (`pics.md` §3).
fn package_buffer(id: u32, apps: &[u32]) -> Vec<u8> {
    let mut appids = Object::new();
    for (i, &a) in apps.iter().enumerate() {
        appids.push(i.to_string(), Value::Int32(a as i32));
    }
    let mut info = Object::new();
    info.push("packageid", Value::Int32(id as i32));
    info.push("appids", appids);
    let mut root = Object::new();
    root.push(id.to_string(), info);
    let mut buf = 0u32.to_le_bytes().to_vec();
    buf.extend_from_slice(&binary::to_vec(&root));
    buf
}

/// Connects the daemon's session worker to a [`FakeCm`].
struct FakeDialer {
    cm: FakeCm,
}

impl Dialer for FakeDialer {
    fn next(&mut self, _cell_id: u32) -> Result<CmServer, NoServer> {
        Ok(CmServer {
            endpoint: ENDPOINT.into(),
            kind: CmKind::Websockets,
            dc: "fake".into(),
        })
    }

    fn failed(&mut self, _endpoint: &str) {}

    fn succeeded(&mut self, _endpoint: &str) {}

    fn connect(
        &mut self,
        server: &CmServer,
        _timeout: Duration,
        events: Sender<Incoming>,
        cancel: &CancelToken,
    ) -> Result<Connection, ConnectError> {
        let (client, server_end) = loopback::pair()?;
        let cm = self.cm.clone();
        std::thread::Builder::new()
            .name("fake-cm".into())
            .spawn(move || cm.serve(server_end))?;
        Connection::with_transport(server, Box::new(client), events, cancel)
    }
}
