// SPDX-License-Identifier: GPL-2.0-or-later
//! A fake CM for the daemon integration tests (SPEC §19.1 level 3). Test
//! code only (FAITH-4).
//!
//! It answers the messages M1a to M1c send, as `cm.md`, `auth.md`,
//! `pics.md`, `content.md` and `launch.md` describe them: the sign-in
//! service methods, logon and logoff, the license list, PICS, persona
//! state, depot keys, manifest request codes, the content server list, CDN
//! auth tokens, play sessions, games' stats and tickets. It checks what the daemon sends
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
    CMsgClientAccountInfo, CMsgClientAuthList, CMsgClientAuthListAck, CMsgClientChangeStatus,
    CMsgClientGameConnectTokens, CMsgClientGamesPlayed, CMsgClientGetAppOwnershipTicket,
    CMsgClientGetAppOwnershipTicketResponse, CMsgClientGetDepotDecryptionKey,
    CMsgClientGetDepotDecryptionKeyResponse, CMsgClientGetUserStats,
    CMsgClientGetUserStatsResponse, CMsgClientLicenseList, CMsgClientLoggedOff, CMsgClientLogon,
    CMsgClientLogonResponse, CMsgClientPicsAccessTokenRequest, CMsgClientPicsAccessTokenResponse,
    CMsgClientPicsChangesSinceRequest, CMsgClientPicsChangesSinceResponse,
    CMsgClientPicsProductInfoRequest, CMsgClientPicsProductInfoResponse,
    CMsgClientRequestEncryptedAppTicket, CMsgClientRequestEncryptedAppTicketResponse,
    CMsgClientStoreUserStats2, CMsgClientStoreUserStatsResponse, CMsgClientTicketAuthComplete,
    CMsgIpAddress, CPlayerGetGameAchievementsRequest, CPlayerGetGameAchievementsResponse,
    EAuthSessionGuardType, EMsg, EncryptedAppTicket, authentication,
    c_msg_client_get_user_stats_response, c_msg_client_license_list,
    c_msg_client_pics_access_token_response, c_msg_client_pics_changes_since_response,
    c_msg_client_pics_product_info_response, c_msg_ip_address,
    c_player_get_game_achievements_response, content_server_directory as csd, player,
};
use ossteam_types::{CancelToken, EResult, SteamId};
use ossteam_vdf::{Object, Value, binary, text};
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};

use super::loopback::{self, ServerEnd};

/// What the fake CM's logon response says about the address and the clock.
pub const FAKE_COUNTRY: &str = "DE";
pub const FAKE_SERVER_TIME: u32 = 1_790_000_000;
pub const FAKE_PUBLIC_IP: u32 = 0xc633_6407;
/// How long the fake's ownership tickets last.
pub const OWNERSHIP_LIFETIME: u32 = 21 * 24 * 3600;
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
    /// Stats schemas by app, as `ClientGetUserStatsResponse` carries them.
    schemas: HashMap<u32, Vec<u8>>,
    /// Users' stats by (Steam id, app).
    stats: HashMap<(u64, u32), FakeStats>,
    /// `ClientStoreUserStats2` messages, in arrival order.
    stores: Vec<CMsgClientStoreUserStats2>,
    /// Global achievement percentages by app, as Steam's strings.
    percentages: HashMap<u32, Vec<(String, String)>>,
    /// `ClientAuthList` messages, in arrival order.
    auth_lists: Vec<CMsgClientAuthList>,
    /// Other users' tickets the fake has given its verdict on, by CRC.
    verdicts_sent: Vec<u32>,
    /// The `EAuthSessionResponse` for other users' tickets.
    verdict: u32,
    /// Game connect tokens handed out so far.
    tokens_issued: u32,
}

/// One user's stats for one app.
#[derive(Clone, Debug, Default)]
pub struct FakeStats {
    pub crc: u32,
    pub values: BTreeMap<u32, u32>,
    /// By (stat, bit).
    pub unlocks: BTreeMap<(u32, u32), u32>,
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

    /// Gives `app` a stats schema (binary KeyValues).
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn stats_schema(self, app: u32, schema: Vec<u8>) -> Self {
        self.state().schemas.insert(app, schema);
        self
    }

    /// Sets a user's stats for `app`.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn set_stats(&self, user: SteamId, app: u32, stats: FakeStats) {
        self.state().stats.insert((user.0, app), stats);
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn stats(&self, user: SteamId, app: u32) -> FakeStats {
        self.state()
            .stats
            .get(&(user.0, app))
            .cloned()
            .unwrap_or_default()
    }

    /// The `ClientStoreUserStats2` messages so far.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn stores(&self) -> Vec<CMsgClientStoreUserStats2> {
        self.state().stores.clone()
    }

    /// The auth lists the daemon sent, in arrival order.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn auth_lists(&self) -> Vec<CMsgClientAuthList> {
        self.state().auth_lists.clone()
    }

    /// The verdict Steam gives on other users' tickets from now on.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn set_verdict(&self, response: u32) {
        self.state().verdict = response;
    }

    /// Sets `app`'s global achievement percentages.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn achievement_percentages(&self, app: u32, list: &[(&str, &str)]) {
        self.state().percentages.insert(
            app,
            list.iter()
                .map(|(n, p)| ((*n).to_owned(), (*p).to_owned()))
                .collect(),
        );
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

    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn auth_lists(&self) -> &[CMsgClientAuthList] {
        &self.0.auth_lists
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
            EMsg::KEMsgClientGetUserStats => {
                assert!(conn.logged_on.is_some(), "stats before logon");
                vec![self.get_user_stats(p)]
            }
            EMsg::KEMsgClientStoreUserStats2 => {
                let steam_id = conn.logged_on.expect("stats before logon");
                vec![self.store_user_stats(steam_id, p)]
            }
            EMsg::KEMsgClientGetAppOwnershipTicket => {
                let steam_id = conn.logged_on.expect("a ticket before logon");
                vec![self.ownership_ticket(steam_id, p)]
            }
            EMsg::KEMsgClientRequestEncryptedAppTicket => {
                let steam_id = conn.logged_on.expect("a ticket before logon");
                vec![self.encrypted_app_ticket(steam_id, p)]
            }
            EMsg::KEMsgClientAuthList => {
                let steam_id = conn.logged_on.expect("an auth list before logon");
                self.auth_list(steam_id, p)
            }
            other => panic!("the fake CM does not handle {other:?}"),
        }
    }

    fn owns(s: &State, steam_id: SteamId, app: u32) -> bool {
        s.accounts
            .iter()
            .filter(|a| a.steam_id == steam_id)
            .flat_map(|a| a.licenses.iter())
            .any(|(package, _)| {
                s.packages
                    .get(package)
                    .is_some_and(|(apps, _)| apps.contains(&app))
            })
    }

    /// `isteamuser.md` §2.2: a ticket of the layout the daemon reads, with
    /// a signature of junk. An app the account does not own gets
    /// `k_EResultAccessDenied` (the real answer is unverified).
    fn ownership_ticket(&self, steam_id: SteamId, p: &Packet) -> Packet {
        let body: CMsgClientGetAppOwnershipTicket = decode(p);
        let app = body.app_id();
        let s = self.state();
        let r = if Self::owns(&s, steam_id, app) {
            CMsgClientGetAppOwnershipTicketResponse {
                eresult: Some(EResult::OK.0 as u32),
                app_id: Some(app),
                ticket: Some(fake_ownership_ticket(steam_id.0, app)),
            }
        } else {
            CMsgClientGetAppOwnershipTicketResponse {
                eresult: Some(EResult::ACCESS_DENIED.0 as u32),
                app_id: Some(app),
                ticket: None,
            }
        };
        reply_to(p, EMsg::KEMsgClientGetAppOwnershipTicketResponse, &r)
    }

    /// `isteamuser.md` §2.7. The "encrypted" ticket is the user data
    /// reversed, so a test can tell it carried them.
    fn encrypted_app_ticket(&self, steam_id: SteamId, p: &Packet) -> Packet {
        let body: CMsgClientRequestEncryptedAppTicket = decode(p);
        let app = body.app_id();
        let s = self.state();
        let r = if Self::owns(&s, steam_id, app) {
            let mut data = body.userdata().to_vec();
            data.reverse();
            CMsgClientRequestEncryptedAppTicketResponse {
                app_id: Some(app),
                eresult: Some(EResult::OK.0),
                encrypted_app_ticket: Some(EncryptedAppTicket {
                    ticket_version_no: Some(1),
                    crc_encryptedticket: Some(crc32fast::hash(&data)),
                    cb_encrypteduserdata: Some(data.len() as u32),
                    cb_encrypted_appownershipticket: Some(0),
                    encrypted_ticket: Some(data),
                }),
            }
        } else {
            CMsgClientRequestEncryptedAppTicketResponse {
                app_id: Some(app),
                eresult: Some(EResult::ACCESS_DENIED.0),
                encrypted_app_ticket: None,
            }
        };
        reply_to(p, EMsg::KEMsgClientRequestEncryptedAppTicketResponse, &r)
    }

    /// `isteamuser.md` §2.4, §2.5: an ack naming every ticket, a verdict on
    /// each new ticket of another user, and two more tokens when the
    /// daemon says it has fewer than two (how Steam tops tokens up is
    /// unverified).
    fn auth_list(&self, steam_id: SteamId, p: &Packet) -> Vec<Packet> {
        let body: CMsgClientAuthList = decode(p);
        let mut s = self.state();
        let mut out = vec![reply_to(
            p,
            EMsg::KEMsgClientAuthListAck,
            &CMsgClientAuthListAck {
                ticket_crc: body.tickets.iter().map(|t| t.ticket_crc()).collect(),
                app_ids: body.app_ids.clone(),
                message_sequence: body.message_sequence,
            },
        )];
        for t in body.tickets.iter().filter(|t| t.estate() == 1) {
            if s.verdicts_sent.contains(&t.ticket_crc()) {
                continue;
            }
            s.verdicts_sent.push(t.ticket_crc());
            out.push(Packet::proto(
                EMsg::KEMsgClientTicketAuthComplete,
                &CMsgClientTicketAuthComplete {
                    steam_id: t.steamid,
                    game_id: t.gameid,
                    estate: Some(1),
                    eauth_session_response: Some(s.verdict),
                    ticket_crc: t.ticket_crc,
                    owner_steam_id: t.steamid,
                    ..Default::default()
                },
            ));
        }
        if body.tokens_left() < 2 {
            out.push(Self::tokens(&mut s, steam_id, 2));
        }
        s.auth_lists.push(body);
        self.notify();
        out
    }

    /// `ClientGameConnectTokens` with `n` new tokens of 20 bytes.
    fn tokens(s: &mut State, steam_id: SteamId, n: u32) -> Packet {
        let tokens = (0..n)
            .map(|_| {
                s.tokens_issued += 1;
                let mut t = u64::from(s.tokens_issued).to_le_bytes().to_vec();
                t.extend_from_slice(&steam_id.0.to_le_bytes());
                t.extend_from_slice(&FAKE_SERVER_TIME.to_le_bytes());
                t
            })
            .collect();
        Packet::proto(
            EMsg::KEMsgClientGameConnectTokens,
            &CMsgClientGameConnectTokens {
                max_tokens_to_keep: Some(10),
                tokens,
            },
        )
    }

    /// `isteamuserstats.md` §1.1. An app without a schema has no stats,
    /// which the fake answers with `k_EResultFail` (the real server's
    /// answer is unverified).
    fn get_user_stats(&self, p: &Packet) -> Packet {
        let body: CMsgClientGetUserStats = decode(p);
        assert_eq!(body.schema_local_version(), -1);
        let app = u32::try_from(body.game_id()).expect("an app's game id");
        let s = self.state();
        let r = match s.schemas.get(&app) {
            None => CMsgClientGetUserStatsResponse {
                game_id: body.game_id,
                eresult: Some(EResult::FAIL.0),
                ..Default::default()
            },
            Some(schema) => {
                let stats = s
                    .stats
                    .get(&(body.steam_id_for_user(), app))
                    .cloned()
                    .unwrap_or_default();
                let mut blocks: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
                for (&(stat, bit), &time) in &stats.unlocks {
                    blocks.entry(stat).or_insert_with(|| vec![0; 32])[bit as usize] = time;
                }
                CMsgClientGetUserStatsResponse {
                    game_id: body.game_id,
                    eresult: Some(EResult::OK.0),
                    crc_stats: Some(stats.crc),
                    schema: Some(schema.clone()),
                    stats: stats
                        .values
                        .iter()
                        .map(|(&id, &v)| c_msg_client_get_user_stats_response::Stats {
                            stat_id: Some(id),
                            stat_value: Some(v),
                        })
                        .collect(),
                    achievement_blocks: blocks
                        .into_iter()
                        .map(|(id, unlock_time)| {
                            c_msg_client_get_user_stats_response::AchievementBlocks {
                                achievement_id: Some(id),
                                unlock_time,
                            }
                        })
                        .collect(),
                }
            }
        };
        reply_to(p, EMsg::KEMsgClientGetUserStatsResponse, &r)
    }

    /// A store with a stale crc is out of date and refused, which is the
    /// fake's reading of `stats_out_of_date`. Otherwise the values are
    /// taken, a bit newly set gets the fake's clock as its unlock time,
    /// and the crc moves on.
    fn store_user_stats(&self, steam_id: SteamId, p: &Packet) -> Packet {
        let body: CMsgClientStoreUserStats2 = decode(p);
        assert_eq!(body.settor_steam_id(), steam_id.0);
        assert_eq!(body.settee_steam_id(), steam_id.0);
        let app = u32::try_from(body.game_id()).expect("an app's game id");
        let mut s = self.state();
        s.stores.push(body.clone());
        let stats = s.stats.entry((steam_id.0, app)).or_default();
        let r = if body.crc_stats() != stats.crc {
            CMsgClientStoreUserStatsResponse {
                game_id: body.game_id,
                eresult: Some(EResult::FAIL.0),
                crc_stats: Some(stats.crc),
                stats_out_of_date: Some(true),
                ..Default::default()
            }
        } else {
            for v in &body.stats {
                let old = stats.values.get(&v.stat_id()).copied().unwrap_or(0);
                for bit in 0..32 {
                    if v.stat_value() & !old & (1 << bit) != 0 {
                        stats.unlocks.insert((v.stat_id(), bit), FAKE_SERVER_TIME);
                    }
                }
                stats.values.insert(v.stat_id(), v.stat_value());
            }
            stats.crc += 1;
            CMsgClientStoreUserStatsResponse {
                game_id: body.game_id,
                eresult: Some(EResult::OK.0),
                crc_stats: Some(stats.crc),
                ..Default::default()
            }
        };
        reply_to(p, EMsg::KEMsgClientStoreUserStatsResponse, &r)
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
        } else if is::<player::GetGameAchievements>(&name) {
            let body: CPlayerGetGameAchievementsRequest = decode(p);
            let list = self.state().percentages.get(&body.appid()).cloned();
            service_reply(
                p,
                list.ok_or(EResult::FAIL)
                    .map(|list| CPlayerGetGameAchievementsResponse {
                        achievements: list
                            .into_iter()
                            .map(
                                |(n, pct)| c_player_get_game_achievements_response::Achievement {
                                    internal_name: Some(n),
                                    player_percent_unlocked: Some(pct),
                                    ..Default::default()
                                },
                            )
                            .collect(),
                        ..Default::default()
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
                ip_country_code: Some(FAKE_COUNTRY.into()),
                rtime32_server_time: Some(FAKE_SERVER_TIME),
                public_ip: Some(CMsgIpAddress {
                    ip: Some(c_msg_ip_address::Ip::V4(FAKE_PUBLIC_IP)),
                }),
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
            Self::tokens(&mut s, account.steam_id, 3),
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

/// An ownership ticket for `steam_id` and `app` as `isteamuser.md` §2.2
/// lays it out, with no packages or DLC and 128 bytes of junk for a
/// signature.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn fake_ownership_ticket(steam_id: u64, app: u32) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&4u32.to_le_bytes());
    t.extend_from_slice(&steam_id.to_le_bytes());
    t.extend_from_slice(&app.to_le_bytes());
    t.extend_from_slice(&FAKE_PUBLIC_IP.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&FAKE_SERVER_TIME.to_le_bytes());
    t.extend_from_slice(&(FAKE_SERVER_TIME + OWNERSHIP_LIFETIME).to_le_bytes());
    t.extend_from_slice(&0u16.to_le_bytes());
    t.extend_from_slice(&0u16.to_le_bytes());
    t.extend_from_slice(&0u16.to_le_bytes());
    let len = t.len() as u32;
    t[..4].copy_from_slice(&len.to_le_bytes());
    t.extend_from_slice(&[0x5a; 128]);
    t
}

/// App info as text KeyValues with a trailing NUL (`pics.md` §3).
fn app_buffer(id: u32, app: &App) -> Vec<u8> {
    let mut common = Object::new();
    common.push("name", app.name.as_str());
    common.push("type", app.app_type.as_str());
    if let Some(parent) = app.parent {
        common.push("parent", parent.to_string());
    }
    // A `common` section adds to the one made from the name and type.
    for (_, v) in app.sections.iter().filter(|(k, _)| k == "common") {
        for (k, v) in v.iter() {
            common.set(k, v.clone());
        }
    }
    let mut info = Object::new();
    info.push("appid", id.to_string());
    info.push("common", common);
    for (k, v) in app.sections.iter().filter(|(k, _)| k != "common") {
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
