// SPDX-License-Identifier: GPL-2.0-or-later
//! Content requests over a CM connection (`content.md` §1 to §3, §7).

use std::time::Duration;

use ossteam_net::{CallError, Packet, Requester};
use ossteam_proto::steam::{
    CContentServerDirectoryGetCdnAuthTokenRequest,
    CContentServerDirectoryGetManifestRequestCodeRequest,
    CContentServerDirectoryGetServersForSteamPipeRequest, CContentServerDirectoryServerInfo,
    CMsgClientGetDepotDecryptionKey, CMsgClientGetDepotDecryptionKeyResponse, EMsg,
    content_server_directory as csd,
};
use ossteam_types::{AppId, CancelToken, DepotId, EResult, ManifestId};

use crate::key::DepotKey;

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

/// The content servers for this client's cell (`content.md` §1).
pub fn servers(
    conn: &Requester,
    cell_id: u32,
    cancel: &CancelToken,
) -> Result<Vec<CContentServerDirectoryServerInfo>, RequestError> {
    let req = CContentServerDirectoryGetServersForSteamPipeRequest {
        cell_id: Some(cell_id),
        ..Default::default()
    };
    let resp = conn.call::<csd::GetServersForSteamPipe>(&req, REPLY_TIMEOUT, cancel)?;
    Ok(resp.servers)
}

/// A depot's key, or the result Steam refused it with (`content.md` §2).
pub fn depot_key(
    conn: &Requester,
    app: AppId,
    depot: DepotId,
    cancel: &CancelToken,
) -> Result<Result<DepotKey, EResult>, RequestError> {
    let body = CMsgClientGetDepotDecryptionKey {
        depot_id: Some(depot.0),
        app_id: Some(app.0),
    };
    let packet = Packet::proto(EMsg::KEMsgClientGetDepotDecryptionKey, &body);
    let reply = conn.request(packet, REPLY_TIMEOUT, cancel)?;
    if !reply.is(EMsg::KEMsgClientGetDepotDecryptionKeyResponse) {
        return Err(RequestError::UnexpectedReply(reply.emsg));
    }
    let r: CMsgClientGetDepotDecryptionKeyResponse = reply.decode_body()?;
    let eresult = EResult(r.eresult());
    if !eresult.is_ok() {
        return Ok(Err(eresult));
    }
    if r.depot_id() != depot.0 {
        tracing::warn!(
            asked = depot.0,
            got = r.depot_id(),
            "depot key for another depot"
        );
        return Ok(Err(EResult::FAIL));
    }
    Ok(DepotKey::new(r.depot_encryption_key()).ok_or(EResult::FAIL))
}

/// A manifest request code; 0 means refused (`content.md` §3). `branch`
/// is `None` for the public branch.
pub fn manifest_request_code(
    conn: &Requester,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
    branch: Option<&str>,
    cancel: &CancelToken,
) -> Result<u64, RequestError> {
    let req = CContentServerDirectoryGetManifestRequestCodeRequest {
        app_id: Some(app.0),
        depot_id: Some(depot.0),
        manifest_id: Some(manifest.0),
        app_branch: branch.filter(|b| *b != "public").map(str::to_owned),
        branch_password_hash: None,
    };
    let resp = conn.call::<csd::GetManifestRequestCode>(&req, REPLY_TIMEOUT, cancel)?;
    Ok(resp.manifest_request_code())
}

/// A CDN auth token for one server (`content.md` §7): the token and the
/// Unix time it expires.
pub fn cdn_auth_token(
    conn: &Requester,
    app: AppId,
    depot: DepotId,
    host: &str,
    cancel: &CancelToken,
) -> Result<(String, u32), RequestError> {
    let req = CContentServerDirectoryGetCdnAuthTokenRequest {
        depot_id: Some(depot.0),
        host_name: Some(host.to_owned()),
        app_id: Some(app.0),
    };
    let resp = conn.call::<csd::GetCdnAuthToken>(&req, REPLY_TIMEOUT, cancel)?;
    Ok((resp.token().to_owned(), resp.expiration_time()))
}
