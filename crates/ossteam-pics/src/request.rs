// SPDX-License-Identifier: GPL-2.0-or-later
//! PICS requests over a CM connection (`pics.md` §2 to §4).

use std::collections::HashMap;
use std::time::Duration;

use ossteam_net::{CallError, Packet, Requester};
use ossteam_proto::steam::{
    CMsgClientPicsAccessTokenRequest, CMsgClientPicsAccessTokenResponse,
    CMsgClientPicsChangesSinceRequest, CMsgClientPicsChangesSinceResponse,
    CMsgClientPicsProductInfoRequest, CMsgClientPicsProductInfoResponse, EMsg,
    c_msg_client_pics_product_info_request as req,
};
use ossteam_types::{AppId, CancelToken, PackageId};

use crate::info::{self, Info};

/// The most apps or packages one request carries (`pics.md` §2, §3).
pub const BATCH: usize = 256;
/// How long to wait for each reply.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error(transparent)]
    Call(#[from] CallError),
    #[error("the reply does not decode: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("unexpected reply message {0}")]
    UnexpectedReply(u32),
}

fn decode<M: prost::Message + Default>(reply: &Packet, emsg: EMsg) -> Result<M, RequestError> {
    if !reply.is(emsg) {
        return Err(RequestError::UnexpectedReply(reply.emsg));
    }
    Ok(reply.decode_body()?)
}

/// App access tokens (`pics.md` §2).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AppTokens {
    /// Every app the server answered for; 0 means no token is needed.
    pub tokens: HashMap<AppId, u64>,
    pub denied: Vec<AppId>,
}

pub fn app_tokens(
    conn: &Requester,
    apps: &[AppId],
    cancel: &CancelToken,
) -> Result<AppTokens, RequestError> {
    let mut out = AppTokens::default();
    for batch in apps.chunks(BATCH) {
        let body = CMsgClientPicsAccessTokenRequest {
            packageids: Vec::new(),
            appids: batch.iter().map(|a| a.0).collect(),
        };
        let packet = Packet::proto(EMsg::KEMsgClientPicsAccessTokenRequest, &body);
        let reply = conn.request(packet, REPLY_TIMEOUT, cancel)?;
        let r: CMsgClientPicsAccessTokenResponse =
            decode(&reply, EMsg::KEMsgClientPicsAccessTokenResponse)?;
        for t in r.app_access_tokens {
            out.tokens.insert(AppId(t.appid()), t.access_token());
        }
        out.denied
            .extend(r.app_denied_tokens.into_iter().map(AppId));
    }
    Ok(out)
}

/// What changed since a change number (`pics.md` §4).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    pub current: u32,
    /// Every app must be checked: the gap was too large to list.
    pub full_apps: bool,
    pub full_packages: bool,
    pub apps: Vec<(AppId, u32)>,
    pub packages: Vec<(PackageId, u32)>,
}

pub fn changes_since(
    conn: &Requester,
    since: u32,
    cancel: &CancelToken,
) -> Result<Changes, RequestError> {
    let body = CMsgClientPicsChangesSinceRequest {
        since_change_number: Some(since),
        send_app_info_changes: Some(true),
        send_package_info_changes: Some(true),
        ..Default::default()
    };
    let packet = Packet::proto(EMsg::KEMsgClientPicsChangesSinceRequest, &body);
    let reply = conn.request(packet, REPLY_TIMEOUT, cancel)?;
    let r: CMsgClientPicsChangesSinceResponse =
        decode(&reply, EMsg::KEMsgClientPicsChangesSinceResponse)?;
    let full = r.force_full_update();
    Ok(Changes {
        current: r.current_change_number(),
        full_apps: full || r.force_full_app_update(),
        full_packages: full || r.force_full_package_update(),
        apps: r
            .app_changes
            .iter()
            .map(|c| (AppId(c.appid()), c.change_number()))
            .collect(),
        packages: r
            .package_changes
            .iter()
            .map(|c| (PackageId(c.packageid()), c.change_number()))
            .collect(),
    })
}

/// One id to ask about, with its access token (0 for none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    pub id: u32,
    pub token: u64,
}

/// The answer to a product info request.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fetched {
    pub apps: Vec<Info>,
    pub packages: Vec<Info>,
    /// Ids the server does not know.
    pub unknown_apps: Vec<u32>,
    pub unknown_packages: Vec<u32>,
    /// Ids that came back without usable info: an empty buffer, or one
    /// that does not parse (`pics.md` §3).
    pub unavailable_apps: Vec<u32>,
    pub unavailable_packages: Vec<u32>,
}

/// Asks for app and package info. With `meta_only`, the entries carry
/// change numbers and hashes and their `data` is empty (`pics.md` §3).
pub fn product_info(
    conn: &Requester,
    apps: &[Want],
    packages: &[Want],
    meta_only: bool,
    cancel: &CancelToken,
) -> Result<Fetched, RequestError> {
    let mut out = Fetched::default();
    for batch in packages.chunks(BATCH) {
        one_request(conn, &[], batch, meta_only, cancel, &mut out)?;
    }
    for batch in apps.chunks(BATCH) {
        one_request(conn, batch, &[], meta_only, cancel, &mut out)?;
    }
    Ok(out)
}

fn one_request(
    conn: &Requester,
    apps: &[Want],
    packages: &[Want],
    meta_only: bool,
    cancel: &CancelToken,
    out: &mut Fetched,
) -> Result<(), RequestError> {
    let body = CMsgClientPicsProductInfoRequest {
        packages: packages
            .iter()
            .map(|w| req::PackageInfo {
                packageid: Some(w.id),
                access_token: Some(w.token),
            })
            .collect(),
        apps: apps
            .iter()
            .map(|w| req::AppInfo {
                appid: Some(w.id),
                access_token: Some(w.token),
                only_public_obsolete: None,
            })
            .collect(),
        meta_data_only: Some(meta_only),
        ..Default::default()
    };
    let packet = Packet::proto(EMsg::KEMsgClientPicsProductInfoRequest, &body);
    let mut failure = None;
    conn.request_each(packet, REPLY_TIMEOUT, cancel, |reply| {
        match decode::<CMsgClientPicsProductInfoResponse>(
            &reply,
            EMsg::KEMsgClientPicsProductInfoResponse,
        ) {
            Ok(r) => {
                let pending = r.response_pending();
                take_response(r, meta_only, out);
                pending
            }
            Err(e) => {
                failure = Some(e);
                false
            }
        }
    })?;
    failure.map_or(Ok(()), Err)
}

fn take_response(r: CMsgClientPicsProductInfoResponse, meta_only: bool, out: &mut Fetched) {
    for a in r.apps {
        let id = a.appid();
        let data = if meta_only {
            Ok(Default::default())
        } else {
            info::parse_app_buffer(a.buffer())
        };
        match data {
            Ok(data) => out.apps.push(Info {
                id,
                change_number: a.change_number(),
                sha: a.sha().to_vec(),
                missing_token: a.missing_token(),
                data,
            }),
            Err(e) => {
                tracing::warn!(app = id, size = a.size(), "app info unavailable: {e}");
                out.unavailable_apps.push(id);
            }
        }
    }
    for p in r.packages {
        let id = p.packageid();
        let data = if meta_only {
            Ok(Default::default())
        } else {
            info::parse_package_buffer(p.buffer())
        };
        match data {
            Ok(data) => out.packages.push(Info {
                id,
                change_number: p.change_number(),
                sha: p.sha().to_vec(),
                missing_token: p.missing_token(),
                data,
            }),
            Err(e) => {
                tracing::warn!(
                    package = id,
                    size = p.size(),
                    "package info unavailable: {e}"
                );
                out.unavailable_packages.push(id);
            }
        }
    }
    out.unknown_apps.extend(r.unknown_appids);
    out.unknown_packages.extend(r.unknown_packageids);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossteam_proto::steam::c_msg_client_pics_product_info_response as resp;

    #[test]
    fn responses_are_collected_and_bad_buffers_set_aside() {
        let r = CMsgClientPicsProductInfoResponse {
            apps: vec![
                resp::AppInfo {
                    appid: Some(480),
                    change_number: Some(9),
                    buffer: Some(b"\"appinfo\" { \"appid\" \"480\" }\0".to_vec()),
                    ..Default::default()
                },
                resp::AppInfo {
                    appid: Some(481),
                    size: Some(5000),
                    ..Default::default()
                },
            ],
            unknown_appids: vec![7],
            ..Default::default()
        };
        let mut out = Fetched::default();
        take_response(r, false, &mut out);
        assert_eq!(out.apps.len(), 1);
        assert_eq!(out.apps[0].change_number, 9);
        assert_eq!(out.apps[0].data.get_str("appid"), Some("480"));
        assert_eq!(out.unavailable_apps, [481]);
        assert_eq!(out.unknown_apps, [7]);
    }

    #[test]
    fn metadata_entries_need_no_buffer() {
        let r = CMsgClientPicsProductInfoResponse {
            packages: vec![resp::PackageInfo {
                packageid: Some(5),
                change_number: Some(3),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut out = Fetched::default();
        take_response(r, true, &mut out);
        assert_eq!(out.packages[0].change_number, 3);
        assert!(out.unavailable_packages.is_empty());
    }
}
