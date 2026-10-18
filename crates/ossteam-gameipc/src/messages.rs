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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseBody {
    AppState(AppState),
    Error(ErrorReply),
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
            Self::AppState(r) => {
                out.push(1);
                r.put(out);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::get(r)? {
            1 => Ok(Self::AppState(Wire::get(r)?)),
            t => Err(unknown("request", t)),
        }
    }
}

impl Wire for ResponseBody {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::AppState(a) => {
                out.push(1);
                a.put(out);
            }
            Self::Error(e) => {
                out.push(255);
                e.put(out);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::get(r)? {
            1 => Ok(Self::AppState(Wire::get(r)?)),
            255 => Ok(Self::Error(Wire::get(r)?)),
            t => Err(unknown("response", t)),
        }
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
            t => return Err(unknown("message", t)),
        })
    }
}
