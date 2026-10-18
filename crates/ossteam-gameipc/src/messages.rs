// SPDX-License-Identifier: GPL-2.0-or-later
//! The messages of `steamworks-abi.md` §3.4.

use crate::wire::{DecodeError, Reader, Wire, record};

record! {
    /// The shim's first message.
    pub struct Hello {
        pub protocol: u32,
        pub pid: u32,
        pub app_id: u32,
        /// `x86_64`, `x86` or `aarch64`.
        pub arch: String,
        pub version: String,
    }
}

record! {
    /// The session as a game sees it.
    pub struct Session {
        /// An account is active, though it may be disconnected.
        pub active: bool,
        /// The account is logged on to Steam right now.
        pub logged_on: bool,
        pub steam_id: u64,
        pub persona_name: String,
        /// `EPersonaState`.
        pub persona_state: u32,
        pub ip_country: String,
        /// Steam's clock in Unix seconds when the message was sent; 0
        /// before the first logon.
        pub server_time: u32,
        pub universe: u32,
        pub ui_language: String,
    }
}

record! {
    pub struct Dlc {
        pub app_id: u32,
        pub owned: bool,
        pub installed: bool,
        /// Its info says it is released.
        pub available: bool,
        /// Empty when its info is not known.
        pub name: String,
    }
}

record! {
    /// The app the process claims to be.
    pub struct App {
        pub app_id: u32,
        pub owned: bool,
        /// The process belongs to a game the daemon launched for this app.
        pub launched: bool,
        pub installed: bool,
        pub install_dir: String,
        pub build_id: u32,
        /// Empty for `public`.
        pub branch: String,
        pub language: String,
        pub languages: Vec<String>,
        pub depots: Vec<u32>,
        pub dlc: Vec<Dlc>,
        /// `<data>/userdata/<account id>/<app id>`; empty without an
        /// account.
        pub user_dir: String,
        pub owner: u64,
        pub low_violence: bool,
        /// `ufs/quota` of the app's info: Steam Cloud bytes, 0 for none.
        pub cloud_quota: u64,
        /// `ufs/maxnumfiles`: 0 for no limit.
        pub cloud_max_files: u32,
    }
}

record! {
    pub struct Welcome {
        pub session: Session,
        pub app: App,
    }
}

record! {
    pub struct Refused {
        pub reason: String,
    }
}

record! {
    pub struct AppStateRequest {
        pub app_id: u32,
    }
}

record! {
    /// What `ISteamApps` tells a game about any app.
    pub struct AppState {
        pub owned: bool,
        pub installed: bool,
        pub install_dir: String,
        pub build_id: u32,
        pub depots: Vec<u32>,
    }
}

record! {
    /// A stat's 32 bits: an `int32`, a `float`'s bits, or an achievement
    /// block's bit field (`isteamuserstats.md` §1.1).
    pub struct StatValue {
        pub id: u32,
        pub value: u32,
    }
}

record! {
    pub struct Unlock {
        pub stat: u32,
        pub bit: u32,
        /// Unix seconds.
        pub time: u32,
    }
}

record! {
    pub struct AchievementDef {
        pub bit: u32,
        pub name: String,
        pub display_name: String,
        pub description: String,
        pub hidden: bool,
        pub icon: String,
        pub icon_gray: String,
        /// The stat whose value is the progress; empty for none.
        pub progress_stat: String,
        pub progress_min: u32,
        pub progress_max: u32,
    }
}

record! {
    pub struct StatDef {
        pub id: u32,
        /// Empty for an achievement block.
        pub name: String,
        /// 1 integer, 2 float, 3 average rate, 4 achievements, 5 group
        /// achievements.
        pub kind: u32,
        pub default: u32,
        pub achievements: Vec<AchievementDef>,
    }
}

record! {
    pub struct StatsSchema {
        pub version: u32,
        pub stats: Vec<StatDef>,
    }
}

record! {
    pub struct UserStatsRequest {
        /// 0 for the account.
        pub steam_id: u64,
    }
}

record! {
    pub struct UserStats {
        /// An `EResult`; the other fields are empty unless it is 1.
        pub result: u32,
        pub steam_id: u64,
        pub crc: u32,
        pub schema: StatsSchema,
        pub values: Vec<StatValue>,
        pub unlocks: Vec<Unlock>,
    }
}

record! {
    pub struct StoreStatsRequest {
        pub crc: u32,
        pub values: Vec<StatValue>,
        pub reset: bool,
    }
}

record! {
    pub struct StoreStats {
        pub result: u32,
        pub crc: u32,
        pub reverted: Vec<StatValue>,
        pub out_of_date: bool,
    }
}

record! {
    pub struct GlobalAchievementsRequest {}
}

record! {
    pub struct AchievementPercent {
        pub name: String,
        /// The bits of a `float`.
        pub percent: u32,
    }
}

record! {
    pub struct GlobalAchievements {
        pub result: u32,
        pub achievements: Vec<AchievementPercent>,
    }
}

record! {
    pub struct AchievementIconRequest {
        pub file: String,
    }
}

record! {
    /// RGBA, 8 bits each, rows top down.
    pub struct Image {
        pub result: u32,
        pub width: u32,
        pub height: u32,
        pub rgba: Vec<u8>,
    }
}

record! {
    /// `isteamuser.md` §3.
    pub struct AuthTicketRequest {
        /// 2 for a session ticket, 5 for a Web API ticket.
        pub kind: u32,
        /// The Web API identity; empty otherwise.
        pub identity: String,
        /// The game's buffer, 0 for none.
        pub max: u32,
    }
}

record! {
    pub struct AuthTicket {
        pub result: u32,
        pub crc: u32,
        pub ticket: Vec<u8>,
    }
}

record! {
    pub struct CancelAuthTicketRequest {
        pub crc: u32,
    }
}

record! {
    /// The answer to a request that has nothing to say.
    pub struct Done {}
}

record! {
    pub struct BeginAuthSessionRequest {
        pub steam_id: u64,
        pub ticket: Vec<u8>,
    }
}

record! {
    pub struct BeginAuthSession {
        /// An `EBeginAuthSessionResult`.
        pub result: u32,
    }
}

record! {
    pub struct EndAuthSessionRequest {
        pub steam_id: u64,
    }
}

record! {
    pub struct UserLicenseRequest {
        pub steam_id: u64,
        pub app_id: u32,
    }
}

record! {
    pub struct UserLicense {
        /// An `EUserHasLicenseForAppResult`.
        pub result: u32,
    }
}

record! {
    pub struct EncryptedAppTicketRequest {
        pub userdata: Vec<u8>,
    }
}

record! {
    pub struct EncryptedAppTicket {
        pub result: u32,
        /// The protobuf `EncryptedAppTicket`, encoded.
        pub ticket: Vec<u8>,
    }
}

record! {
    pub struct TicketActive {
        pub crc: u32,
        /// 1, or 16 when Steam did not confirm the ticket in time.
        pub result: u32,
    }
}

record! {
    pub struct TicketValidated {
        pub steam_id: u64,
        pub owner: u64,
        /// An `EAuthSessionResponse`.
        pub response: u32,
    }
}

record! {
    pub struct ErrorReply {
        pub message: String,
    }
}

record! {
    /// Calls to one unimplemented slot or export.
    pub struct Call {
        /// The interface version string, or `exports`.
        pub interface: String,
        pub method: String,
        pub count: u32,
    }
}

record! {
    /// What the shim recorded since its previous report
    /// (`steamworks-abi.md` §4).
    pub struct Report {
        pub interfaces: Vec<String>,
        pub unknown: Vec<String>,
        pub calls: Vec<Call>,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestBody {
    AppState(AppStateRequest),
    UserStats(UserStatsRequest),
    StoreStats(StoreStatsRequest),
    GlobalAchievements(GlobalAchievementsRequest),
    AchievementIcon(AchievementIconRequest),
    AuthTicket(AuthTicketRequest),
    CancelAuthTicket(CancelAuthTicketRequest),
    BeginAuthSession(BeginAuthSessionRequest),
    EndAuthSession(EndAuthSessionRequest),
    UserLicense(UserLicenseRequest),
    EncryptedAppTicket(EncryptedAppTicketRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseBody {
    AppState(AppState),
    UserStats(UserStats),
    StoreStats(StoreStats),
    GlobalAchievements(GlobalAchievements),
    Image(Image),
    AuthTicket(AuthTicket),
    Done(Done),
    BeginAuthSession(BeginAuthSession),
    UserLicense(UserLicense),
    EncryptedAppTicket(EncryptedAppTicket),
    Error(ErrorReply),
}

/// What Steam said later about a request (`isteamuser.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeBody {
    TicketActive(TicketActive),
    TicketValidated(TicketValidated),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub body: RequestBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub id: u32,
    pub body: ResponseBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Welcome(Welcome),
    Refused(Refused),
    Request(Request),
    Response(Response),
    SessionUpdate(Session),
    AppUpdate(App),
    Report(Report),
    Notice(NoticeBody),
}

fn unknown(what: &str, tag: u8) -> DecodeError {
    DecodeError(format!("unknown {what} tag {tag}"))
}

fn tagged(out: &mut Vec<u8>, tag: u8, body: &impl Wire) {
    out.push(tag);
    body.put(out);
}

impl Wire for RequestBody {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::AppState(r) => tagged(out, 1, r),
            Self::UserStats(r) => tagged(out, 2, r),
            Self::StoreStats(r) => tagged(out, 3, r),
            Self::GlobalAchievements(r) => tagged(out, 4, r),
            Self::AchievementIcon(r) => tagged(out, 5, r),
            Self::AuthTicket(r) => tagged(out, 6, r),
            Self::CancelAuthTicket(r) => tagged(out, 7, r),
            Self::BeginAuthSession(r) => tagged(out, 8, r),
            Self::EndAuthSession(r) => tagged(out, 9, r),
            Self::UserLicense(r) => tagged(out, 10, r),
            Self::EncryptedAppTicket(r) => tagged(out, 11, r),
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(match u8::get(r)? {
            1 => Self::AppState(Wire::get(r)?),
            2 => Self::UserStats(Wire::get(r)?),
            3 => Self::StoreStats(Wire::get(r)?),
            4 => Self::GlobalAchievements(Wire::get(r)?),
            5 => Self::AchievementIcon(Wire::get(r)?),
            6 => Self::AuthTicket(Wire::get(r)?),
            7 => Self::CancelAuthTicket(Wire::get(r)?),
            8 => Self::BeginAuthSession(Wire::get(r)?),
            9 => Self::EndAuthSession(Wire::get(r)?),
            10 => Self::UserLicense(Wire::get(r)?),
            11 => Self::EncryptedAppTicket(Wire::get(r)?),
            t => return Err(unknown("request", t)),
        })
    }
}

impl Wire for ResponseBody {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::AppState(a) => tagged(out, 1, a),
            Self::UserStats(a) => tagged(out, 2, a),
            Self::StoreStats(a) => tagged(out, 3, a),
            Self::GlobalAchievements(a) => tagged(out, 4, a),
            Self::Image(a) => tagged(out, 5, a),
            Self::AuthTicket(a) => tagged(out, 6, a),
            Self::Done(a) => tagged(out, 7, a),
            Self::BeginAuthSession(a) => tagged(out, 8, a),
            Self::UserLicense(a) => tagged(out, 10, a),
            Self::EncryptedAppTicket(a) => tagged(out, 11, a),
            Self::Error(e) => tagged(out, 255, e),
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(match u8::get(r)? {
            1 => Self::AppState(Wire::get(r)?),
            2 => Self::UserStats(Wire::get(r)?),
            3 => Self::StoreStats(Wire::get(r)?),
            4 => Self::GlobalAchievements(Wire::get(r)?),
            5 => Self::Image(Wire::get(r)?),
            6 => Self::AuthTicket(Wire::get(r)?),
            7 => Self::Done(Wire::get(r)?),
            8 => Self::BeginAuthSession(Wire::get(r)?),
            10 => Self::UserLicense(Wire::get(r)?),
            11 => Self::EncryptedAppTicket(Wire::get(r)?),
            255 => Self::Error(Wire::get(r)?),
            t => return Err(unknown("response", t)),
        })
    }
}

impl Wire for NoticeBody {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::TicketActive(n) => tagged(out, 1, n),
            Self::TicketValidated(n) => tagged(out, 2, n),
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(match u8::get(r)? {
            1 => Self::TicketActive(Wire::get(r)?),
            2 => Self::TicketValidated(Wire::get(r)?),
            t => return Err(unknown("notice", t)),
        })
    }
}

impl Wire for Request {
    fn put(&self, out: &mut Vec<u8>) {
        self.id.put(out);
        self.body.put(out);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            id: Wire::get(r)?,
            body: Wire::get(r)?,
        })
    }
}

impl Wire for Response {
    fn put(&self, out: &mut Vec<u8>) {
        self.id.put(out);
        self.body.put(out);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            id: Wire::get(r)?,
            body: Wire::get(r)?,
        })
    }
}

impl Wire for Message {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hello(m) => tagged(out, 1, m),
            Self::Welcome(m) => tagged(out, 2, m),
            Self::Refused(m) => tagged(out, 3, m),
            Self::Request(m) => tagged(out, 4, m),
            Self::Response(m) => tagged(out, 5, m),
            Self::SessionUpdate(m) => tagged(out, 6, m),
            Self::AppUpdate(m) => tagged(out, 7, m),
            Self::Report(m) => tagged(out, 8, m),
            Self::Notice(m) => tagged(out, 9, m),
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(match u8::get(r)? {
            1 => Self::Hello(Wire::get(r)?),
            2 => Self::Welcome(Wire::get(r)?),
            3 => Self::Refused(Wire::get(r)?),
            4 => Self::Request(Wire::get(r)?),
            5 => Self::Response(Wire::get(r)?),
            6 => Self::SessionUpdate(Wire::get(r)?),
            7 => Self::AppUpdate(Wire::get(r)?),
            8 => Self::Report(Wire::get(r)?),
            9 => Self::Notice(Wire::get(r)?),
            t => return Err(unknown("message", t)),
        })
    }
}
