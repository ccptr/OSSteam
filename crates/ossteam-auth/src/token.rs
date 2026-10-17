// SPDX-License-Identifier: GPL-2.0-or-later
//! Refresh and access tokens (`auth.md` §4, AUTH-3).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ossteam_net::Connection;
use ossteam_proto::steam::{
    CAuthenticationAccessTokenGenerateForAppRequest, ETokenRenewalType, authentication,
};
use ossteam_types::{CancelToken, Secret, SteamId};
use serde::Deserialize;

use crate::{AuthError, CALL_TIMEOUT};

/// What ossteam reads from a token's payload: enough to schedule renewal
/// and to show the account. The signature is the server's business and is
/// never checked or relied on here (FAITH-3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenInfo {
    pub steam_id: SteamId,
    /// Expiry, in seconds since the Unix epoch.
    pub expires_at: Option<u64>,
    pub audience: Vec<String>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(default)]
    aud: Vec<String>,
}

/// Reads the payload of a JWT without verifying it.
pub fn inspect(token: &Secret) -> Result<TokenInfo, AuthError> {
    let payload = token
        .expose()
        .split('.')
        .nth(1)
        .ok_or_else(|| AuthError::BadToken("no payload segment".into()))?;
    let json = zeroize::Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .map_err(|e| AuthError::BadToken(e.to_string()))?,
    );
    let claims: Claims =
        serde_json::from_slice(&json).map_err(|e| AuthError::BadToken(e.to_string()))?;
    let steam_id = claims
        .sub
        .parse()
        .map_err(|_| AuthError::BadToken("subject is not a Steam id".into()))?;
    Ok(TokenInfo {
        steam_id,
        expires_at: claims.exp,
        audience: claims.aud,
    })
}

/// A fresh access token, and a renewed refresh token if the server chose
/// to issue one.
#[derive(Debug)]
pub struct Renewed {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
}

/// `GenerateAccessTokenForApp` (`auth.md` §4). With `allow_renewal` the
/// server may also replace the refresh token.
pub fn generate_access_token(
    conn: &Connection,
    refresh_token: &Secret,
    steam_id: SteamId,
    allow_renewal: bool,
    cancel: &CancelToken,
) -> Result<Renewed, AuthError> {
    let mut req = CAuthenticationAccessTokenGenerateForAppRequest {
        refresh_token: Some(refresh_token.expose().to_owned()),
        steamid: Some(steam_id.0),
        ..Default::default()
    };
    if allow_renewal {
        req.set_renewal_type(ETokenRenewalType::KETokenRenewalTypeAllow);
    }
    let result = conn.call::<authentication::GenerateAccessTokenForApp>(&req, CALL_TIMEOUT, cancel);
    zeroize::Zeroize::zeroize(req.refresh_token.get_or_insert_default());
    let resp = result?;
    Ok(Renewed {
        access_token: Secret::new(
            resp.access_token
                .clone()
                .filter(|t| !t.is_empty())
                .ok_or(AuthError::Incomplete("access_token"))?,
        ),
        refresh_token: resp
            .refresh_token
            .clone()
            .filter(|t| !t.is_empty())
            .map(Secret::new),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: &str) -> Secret {
        Secret::new(format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"typ":"JWT","alg":"EdDSA"}"#),
            URL_SAFE_NO_PAD.encode(payload)
        ))
    }

    #[test]
    fn reads_claims() {
        let info = inspect(&jwt(
            r#"{"iss":"steam","sub":"76561197960287930","aud":["client","web"],"exp":1800000000}"#,
        ))
        .unwrap();
        assert_eq!(info.steam_id, SteamId(76561197960287930));
        assert_eq!(info.expires_at, Some(1_800_000_000));
        assert_eq!(info.audience, ["client", "web"]);
    }

    #[test]
    fn rejects_non_jwts() {
        assert!(inspect(&Secret::new("opaque".into())).is_err());
        assert!(inspect(&jwt(r#"{"sub":"bob"}"#)).is_err());
    }
}
