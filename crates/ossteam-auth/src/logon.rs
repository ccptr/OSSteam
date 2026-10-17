// SPDX-License-Identifier: GPL-2.0-or-later
//! CM logon and logoff (`cm.md` §4 steps 4 and 6, `auth.md` §4).
//!
//! These build and read messages only. The daemon sends them on its
//! connection and waits for the replies, which arrive unsolicited.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use ossteam_net::Packet;
use ossteam_net::conn::PROTOCOL_VERSION;
use ossteam_proto::steam::{
    CMsgClientLogOff, CMsgClientLoggedOff, CMsgClientLogon, CMsgClientLogonResponse, CMsgIpAddress,
    EMsg, c_msg_ip_address,
};
use ossteam_types::{AccountType, EResult, Secret, SteamId, Universe};

/// The `ClientLogon` message for a refresh-token logon.
///
/// Only the fields `cm.md` and `auth.md` establish are set: the protocol
/// version, the account name, the cell id, and the refresh token in the
/// `access_token` field (`auth.md` §4, unverified). The rest of what the
/// official client sends waits for the capture in `cm.md` §7.
pub fn logon_packet(account_name: &str, refresh_token: &Secret, cell_id: u32) -> Packet {
    let mut body = CMsgClientLogon {
        protocol_version: Some(PROTOCOL_VERSION),
        cell_id: Some(cell_id),
        account_name: Some(account_name.to_owned()),
        access_token: Some(refresh_token.expose().to_owned()),
        ..Default::default()
    };
    let mut packet = Packet::proto(EMsg::KEMsgClientLogon, &body);
    zeroize::Zeroize::zeroize(body.access_token.get_or_insert_default());
    // Before logon the connection has no Steam id; the header names an
    // individual desktop account with account id 0 (`cm.md` §4, unverified).
    if let Some(h) = packet.proto_header_mut() {
        h.steamid = Some(
            SteamId::new(
                Universe::Public,
                AccountType::Individual,
                SteamId::DESKTOP_INSTANCE,
                0,
            )
            .0,
        );
        h.client_sessionid = Some(0);
    }
    packet
}

/// A successful logon.
#[derive(Clone, Debug, PartialEq)]
pub struct LogonOk {
    pub steam_id: SteamId,
    pub session_id: i32,
    /// The heartbeat interval the server set, if it set one.
    pub heartbeat: Option<Duration>,
    pub cell_id: Option<u32>,
    pub public_ip: Option<IpAddr>,
    pub country: Option<String>,
    pub vanity_url: Option<String>,
    pub email_domain: Option<String>,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LogonError {
    #[error("logon refused: {eresult}")]
    Refused {
        eresult: EResult,
        extended: Option<EResult>,
    },
    #[error("logon response is malformed: {0}")]
    Malformed(String),
}

impl LogonError {
    /// Whether retrying later, with the same token, can help: the server
    /// was busy or redirected us, rather than rejecting the account.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Refused { eresult, .. } => [
                EResult::TRY_ANOTHER_CM,
                EResult::SERVICE_UNAVAILABLE,
                EResult::BUSY,
                EResult::TIMEOUT,
                EResult::NO_CONNECTION,
                EResult::SERVICE_READ_ONLY,
                EResult::PENDING,
            ]
            .contains(eresult),
            Self::Malformed(_) => true,
        }
    }

    /// Whether the token itself was rejected, so it must be deleted and the
    /// user asked to sign in again (`auth.md` §4). Which EResults Steam
    /// uses here is unverified; this list is the best current reading.
    pub fn token_rejected(&self) -> bool {
        match self {
            Self::Refused { eresult, .. } => [
                EResult::INVALID_PASSWORD,
                EResult::ACCESS_DENIED,
                EResult::EXPIRED,
                EResult::REVOKED,
                EResult::ACCOUNT_LOGON_DENIED,
                EResult::INVALID_LOGIN_AUTH_CODE,
                EResult::EXPIRED_LOGIN_AUTH_CODE,
            ]
            .contains(eresult),
            Self::Malformed(_) => false,
        }
    }
}

/// Reads `ClientLogOnResponse`.
pub fn parse_logon_response(packet: &Packet) -> Result<LogonOk, LogonError> {
    if !packet.is(EMsg::KEMsgClientLogOnResponse) {
        return Err(LogonError::Malformed(format!(
            "message {} is not a logon response",
            packet.emsg
        )));
    }
    let header = packet
        .proto_header()
        .ok_or_else(|| LogonError::Malformed("no protobuf header".into()))?;
    let body: CMsgClientLogonResponse = packet
        .decode_body()
        .map_err(|e| LogonError::Malformed(e.to_string()))?;
    let eresult = EResult(body.eresult());
    if !eresult.is_ok() {
        return Err(LogonError::Refused {
            eresult,
            extended: body.eresult_extended.map(EResult),
        });
    }
    let seconds = [
        body.heartbeat_seconds,
        body.legacy_out_of_game_heartbeat_seconds,
    ]
    .into_iter()
    .flatten()
    .find(|&s| s > 0);
    Ok(LogonOk {
        steam_id: SteamId(
            header
                .steamid
                .ok_or_else(|| LogonError::Malformed("no Steam id in header".into()))?,
        ),
        session_id: header.client_sessionid(),
        heartbeat: seconds.map(|s| Duration::from_secs(s as u64)),
        cell_id: body.cell_id,
        public_ip: body.public_ip.as_ref().and_then(ip),
        country: body
            .user_country
            .clone()
            .or(body.ip_country_code.clone())
            .filter(|c| !c.is_empty()),
        vanity_url: body.vanity_url.clone().filter(|v| !v.is_empty()),
        email_domain: body.email_domain.clone().filter(|d| !d.is_empty()),
    })
}

fn ip(addr: &CMsgIpAddress) -> Option<IpAddr> {
    match addr.ip.as_ref()? {
        c_msg_ip_address::Ip::V4(v) => Some(IpAddr::V4(Ipv4Addr::from(*v))),
        c_msg_ip_address::Ip::V6(b) => <[u8; 16]>::try_from(b.as_slice())
            .ok()
            .map(|b| IpAddr::V6(Ipv6Addr::from(b))),
    }
}

/// `ClientLogOff` (`cm.md` §4 step 6).
pub fn logoff_packet() -> Packet {
    Packet::proto(EMsg::KEMsgClientLogOff, &CMsgClientLogOff::default())
}

/// The reason in `ClientLoggedOff`, if `packet` is one.
pub fn parse_logged_off(packet: &Packet) -> Option<EResult> {
    if !packet.is(EMsg::KEMsgClientLoggedOff) {
        return None;
    }
    let body: CMsgClientLoggedOff = packet.decode_body().ok()?;
    Some(EResult(body.eresult()))
}

#[cfg(test)]
mod tests {
    use ossteam_proto::steam::CMsgProtoBufHeader;

    use super::*;

    fn response(eresult: i32, steam_id: u64) -> Packet {
        let body = CMsgClientLogonResponse {
            eresult: Some(eresult),
            heartbeat_seconds: Some(9),
            cell_id: Some(4),
            public_ip: Some(CMsgIpAddress {
                ip: Some(c_msg_ip_address::Ip::V4(0x0102_0304)),
            }),
            ..Default::default()
        };
        let mut p = Packet::proto(EMsg::KEMsgClientLogOnResponse, &body);
        *p.proto_header_mut().unwrap() = CMsgProtoBufHeader {
            steamid: Some(steam_id),
            client_sessionid: Some(77),
            ..Default::default()
        };
        p
    }

    #[test]
    fn logon_packet_sets_only_established_fields() {
        let p = logon_packet("alice", &Secret::new("tok".into()), 3);
        let body: CMsgClientLogon = p.decode_body().unwrap();
        assert_eq!(body.account_name(), "alice");
        assert_eq!(body.access_token(), "tok");
        assert_eq!(body.protocol_version(), PROTOCOL_VERSION);
        assert_eq!(body.client_os_type, None);
        assert_eq!(body.machine_id, None);
        assert_eq!(
            p.proto_header().unwrap().steamid,
            Some(0x0110_0001_0000_0000)
        );
    }

    #[test]
    fn parses_success() {
        let ok = parse_logon_response(&response(1, 76561197960287930)).unwrap();
        assert_eq!(ok.steam_id, SteamId(76561197960287930));
        assert_eq!(ok.session_id, 77);
        assert_eq!(ok.heartbeat, Some(Duration::from_secs(9)));
        assert_eq!(ok.public_ip, Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
    }

    #[test]
    fn classifies_failures() {
        let e = parse_logon_response(&response(EResult::TRY_ANOTHER_CM.0, 0)).unwrap_err();
        assert!(e.is_transient() && !e.token_rejected());
        let e = parse_logon_response(&response(EResult::ACCESS_DENIED.0, 0)).unwrap_err();
        assert!(!e.is_transient() && e.token_rejected());
    }
}
