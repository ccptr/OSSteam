// SPDX-License-Identifier: GPL-2.0-or-later
//! Credential and QR sign-in sessions (`auth.md` §1 to §3).

use std::time::Duration;

use ossteam_net::Connection;
use ossteam_proto::steam::{
    CAuthenticationAllowedConfirmation, CAuthenticationBeginAuthSessionViaCredentialsRequest,
    CAuthenticationBeginAuthSessionViaQrRequest, CAuthenticationDeviceDetails,
    CAuthenticationGetPasswordRsaPublicKeyRequest, CAuthenticationPollAuthSessionStatusRequest,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest, EAuthSessionGuardType,
    EAuthTokenPlatformType, ESessionPersistence, authentication,
};
use ossteam_types::{CancelToken, Secret, SteamId};

use crate::{AuthError, CALL_TIMEOUT, password, token};

/// The website id of the Steam client platform (`auth.md` §1).
const WEBSITE_ID: &str = "Client";

/// What identifies this client in a begin-session request.
///
/// Only the fields `auth.md` §1 establishes are sent. OS type, gaming
/// device type and machine id wait for the capture in `auth.md` §8.
#[derive(Clone, Debug)]
pub struct DeviceIdentity {
    /// Shown to the user in the Steam mobile app; the host name.
    pub friendly_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persistence {
    /// A remembered sign-in, whose refresh token is stored.
    Persistent,
    /// `--no-remember`: the token lives only as long as the daemon.
    Ephemeral,
}

/// One way the user may satisfy Steam Guard (`auth.md` §2 step 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    None,
    /// A code sent by email. The hint is the address's domain.
    EmailCode {
        hint: Option<String>,
    },
    /// A code from the Steam mobile authenticator.
    DeviceCode,
    /// Approval in the Steam mobile app.
    DeviceConfirmation,
    /// Approval through a link in an email.
    EmailConfirmation,
    /// Satisfied silently by stored guard data.
    MachineToken,
    /// A type this build does not handle.
    Other(i32),
}

impl Guard {
    fn from_proto(c: &CAuthenticationAllowedConfirmation) -> Self {
        let message = c.associated_message.clone().filter(|m| !m.is_empty());
        match c.confirmation_type() {
            EAuthSessionGuardType::KEAuthSessionGuardTypeNone => Self::None,
            EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode => {
                Self::EmailCode { hint: message }
            }
            EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode => Self::DeviceCode,
            EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceConfirmation => {
                Self::DeviceConfirmation
            }
            EAuthSessionGuardType::KEAuthSessionGuardTypeEmailConfirmation => {
                Self::EmailConfirmation
            }
            EAuthSessionGuardType::KEAuthSessionGuardTypeMachineToken => Self::MachineToken,
            _ => Self::Other(c.confirmation_type.unwrap_or_default()),
        }
    }
}

/// Which kind of code [`PendingSession::submit_code`] sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeKind {
    Email,
    Device,
}

/// The result of a successful sign-in.
#[derive(Debug)]
pub struct Tokens {
    pub account_name: String,
    pub steam_id: SteamId,
    pub refresh_token: Secret,
    pub access_token: Secret,
    /// A machine token to send with later sign-ins (`auth.md` §5).
    pub new_guard_data: Option<Secret>,
}

/// What one poll found.
#[derive(Debug)]
pub enum Poll {
    /// Not approved yet. A QR session may have rotated its challenge.
    Pending {
        new_challenge_url: Option<String>,
    },
    Done(Tokens),
}

/// A sign-in session that has begun and waits for approval.
#[derive(Debug)]
pub struct PendingSession {
    client_id: u64,
    request_id: Vec<u8>,
    interval: Duration,
    steam_id: Option<SteamId>,
    /// The Steam Guard types the user may satisfy, any one of them.
    pub guards: Vec<Guard>,
    /// The QR challenge URL (QR sessions only).
    pub challenge_url: Option<String>,
}

impl PendingSession {
    /// How long to wait between polls, as the server asked. A missing or
    /// non-positive interval is treated as 1 s rather than polling flat out.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// The account's Steam id, known up front for credential sessions.
    pub fn steam_id(&self) -> Option<SteamId> {
        self.steam_id
    }

    /// Sends a Steam Guard code (`auth.md` §2 step 4).
    pub fn submit_code(
        &self,
        conn: &Connection,
        code: Secret,
        kind: CodeKind,
        cancel: &CancelToken,
    ) -> Result<(), AuthError> {
        let steam_id = self
            .steam_id
            .ok_or(AuthError::Incomplete("a Steam id for this session"))?;
        let code_type = match kind {
            CodeKind::Email => EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode,
            CodeKind::Device => EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode,
        };
        let mut req = CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest {
            client_id: Some(self.client_id),
            steamid: Some(steam_id.0),
            code: Some(code.expose().to_owned()),
            ..Default::default()
        };
        req.set_code_type(code_type);
        drop(code);
        let result = conn.call::<authentication::UpdateAuthSessionWithSteamGuardCode>(
            &req,
            CALL_TIMEOUT,
            cancel,
        );
        zeroize::Zeroize::zeroize(req.code.get_or_insert_default());
        result?;
        Ok(())
    }

    /// Polls once (`auth.md` §2 step 5). The caller waits
    /// [`interval`](Self::interval) between polls.
    pub fn poll(&mut self, conn: &Connection, cancel: &CancelToken) -> Result<Poll, AuthError> {
        let req = CAuthenticationPollAuthSessionStatusRequest {
            client_id: Some(self.client_id),
            request_id: Some(self.request_id.clone()),
            token_to_revoke: None,
        };
        let resp =
            conn.call::<authentication::PollAuthSessionStatus>(&req, CALL_TIMEOUT, cancel)?;
        if let Some(id) = resp.new_client_id.filter(|&id| id != 0) {
            self.client_id = id;
        }
        let new_challenge_url = resp.new_challenge_url.clone().filter(|u| !u.is_empty());
        if let Some(url) = &new_challenge_url {
            self.challenge_url = Some(url.clone());
        }
        let refresh = resp.refresh_token.clone().unwrap_or_default();
        if refresh.is_empty() {
            return Ok(Poll::Pending { new_challenge_url });
        }
        let refresh_token = Secret::new(refresh);
        let steam_id = match self.steam_id {
            Some(id) => id,
            None => token::inspect(&refresh_token)?.steam_id,
        };
        Ok(Poll::Done(Tokens {
            account_name: resp.account_name.clone().unwrap_or_default(),
            steam_id,
            refresh_token,
            access_token: Secret::new(resp.access_token.clone().unwrap_or_default()),
            new_guard_data: resp
                .new_guard_data
                .clone()
                .filter(|g| !g.is_empty())
                .map(Secret::new),
        }))
    }
}

fn device_details(identity: &DeviceIdentity) -> CAuthenticationDeviceDetails {
    let mut d = CAuthenticationDeviceDetails {
        device_friendly_name: Some(identity.friendly_name.clone()),
        ..Default::default()
    };
    d.set_platform_type(EAuthTokenPlatformType::KEAuthTokenPlatformTypeSteamClient);
    d
}

fn interval(seconds: Option<f32>) -> Duration {
    match seconds {
        Some(s) if s.is_finite() && s > 0.0 => {
            Duration::from_secs_f32(s).max(Duration::from_millis(100))
        }
        _ => Duration::from_secs(1),
    }
}

/// Begins a credential sign-in (`auth.md` §2 steps 1 to 3). `password` is
/// zeroed as soon as it has been encrypted.
pub fn begin_credentials(
    conn: &Connection,
    account_name: &str,
    password: Secret,
    persistence: Persistence,
    guard_data: Option<&Secret>,
    identity: &DeviceIdentity,
    cancel: &CancelToken,
) -> Result<PendingSession, AuthError> {
    let key = conn.call::<authentication::GetPasswordRsaPublicKey>(
        &CAuthenticationGetPasswordRsaPublicKeyRequest {
            account_name: Some(account_name.to_owned()),
        },
        CALL_TIMEOUT,
        cancel,
    )?;
    let encrypted = password::encrypt(key.publickey_mod(), key.publickey_exp(), password)?;

    let mut req = CAuthenticationBeginAuthSessionViaCredentialsRequest {
        account_name: Some(account_name.to_owned()),
        encrypted_password: Some(encrypted),
        encryption_timestamp: key.timestamp,
        website_id: Some(WEBSITE_ID.into()),
        device_details: Some(device_details(identity)),
        guard_data: guard_data.map(|g| g.expose().to_owned()),
        ..Default::default()
    };
    req.set_platform_type(EAuthTokenPlatformType::KEAuthTokenPlatformTypeSteamClient);
    req.set_persistence(match persistence {
        Persistence::Persistent => ESessionPersistence::KESessionPersistencePersistent,
        Persistence::Ephemeral => ESessionPersistence::KESessionPersistenceEphemeral,
    });
    let result =
        conn.call::<authentication::BeginAuthSessionViaCredentials>(&req, CALL_TIMEOUT, cancel);
    if let Some(g) = req.guard_data.as_mut() {
        zeroize::Zeroize::zeroize(g);
    }
    let resp = result?;
    Ok(PendingSession {
        client_id: resp.client_id.ok_or(AuthError::Incomplete("client_id"))?,
        request_id: resp
            .request_id
            .clone()
            .ok_or(AuthError::Incomplete("request_id"))?,
        interval: interval(resp.interval),
        steam_id: resp.steamid.filter(|&id| id != 0).map(SteamId),
        guards: resp
            .allowed_confirmations
            .iter()
            .map(Guard::from_proto)
            .collect(),
        challenge_url: None,
    })
}

/// Begins a QR sign-in (`auth.md` §3).
pub fn begin_qr(
    conn: &Connection,
    identity: &DeviceIdentity,
    cancel: &CancelToken,
) -> Result<PendingSession, AuthError> {
    let mut req = CAuthenticationBeginAuthSessionViaQrRequest {
        website_id: Some(WEBSITE_ID.into()),
        device_details: Some(device_details(identity)),
        ..Default::default()
    };
    req.set_platform_type(EAuthTokenPlatformType::KEAuthTokenPlatformTypeSteamClient);
    let resp = conn.call::<authentication::BeginAuthSessionViaQr>(&req, CALL_TIMEOUT, cancel)?;
    Ok(PendingSession {
        client_id: resp.client_id.ok_or(AuthError::Incomplete("client_id"))?,
        request_id: resp
            .request_id
            .clone()
            .ok_or(AuthError::Incomplete("request_id"))?,
        interval: interval(resp.interval),
        steam_id: None,
        guards: resp
            .allowed_confirmations
            .iter()
            .map(Guard::from_proto)
            .collect(),
        challenge_url: Some(
            resp.challenge_url
                .clone()
                .filter(|u| !u.is_empty())
                .ok_or(AuthError::Incomplete("challenge_url"))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_guard_types() {
        let mut c = CAuthenticationAllowedConfirmation {
            associated_message: Some("example.com".into()),
            ..Default::default()
        };
        c.set_confirmation_type(EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode);
        assert_eq!(
            Guard::from_proto(&c),
            Guard::EmailCode {
                hint: Some("example.com".into())
            }
        );
        c.confirmation_type = Some(4242);
        assert_eq!(Guard::from_proto(&c), Guard::Other(4242));
    }

    #[test]
    fn interval_defaults_and_floors() {
        assert_eq!(interval(None), Duration::from_secs(1));
        assert_eq!(interval(Some(0.0)), Duration::from_secs(1));
        assert_eq!(interval(Some(f32::NAN)), Duration::from_secs(1));
        assert_eq!(interval(Some(5.0)), Duration::from_secs(5));
        assert_eq!(interval(Some(0.01)), Duration::from_millis(100));
    }
}
