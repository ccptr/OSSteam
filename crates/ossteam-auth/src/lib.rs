// SPDX-License-Identifier: GPL-2.0-or-later
//! Signing in to Steam (SPEC §9.2, AUTH-1 to AUTH-4; `docs/spec/auth.md`).
//!
//! - [`session`]: credential and QR sign-in through the `Authentication`
//!   service, Steam Guard codes and polling.
//! - [`token`]: reading refresh tokens and renewing access tokens.
//! - [`logon`]: the CM logon and logoff messages.
//!
//! The daemon drives these: it owns the connection, the timing of polls,
//! the IPC prompts and token storage. Nothing here stores anything.

pub mod logon;
pub mod password;
pub mod session;
pub mod token;

use ossteam_net::CallError;

/// How long one service call may take before it is abandoned.
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(transparent)]
    Call(#[from] CallError),
    #[error("the server's RSA key is unusable: {0}")]
    BadKey(String),
    #[error("password encryption failed: {0}")]
    Encrypt(String),
    #[error("the server's reply lacks {0}")]
    Incomplete(&'static str),
    #[error("token is not a readable JWT: {0}")]
    BadToken(String),
}

pub use session::{CodeKind, DeviceIdentity, Guard, PendingSession, Persistence, Poll, Tokens};
