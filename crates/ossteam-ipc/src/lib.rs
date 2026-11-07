// SPDX-License-Identifier: GPL-2.0-or-later
//! The ossteam client IPC protocol (SPEC §8): the generated schema types,
//! frame encoding, endpoint conventions and a blocking client.
//!
//! Leaf clients depend on this crate and nothing Steam-related (LEAF-2).
//! The daemon uses the same crate for its side of the connection.

pub mod client;
pub mod endpoint;
pub mod frame;
pub mod local_time;
mod os;

/// Types generated from `proto/ossteam/ipc/v1/*.proto`.
#[allow(clippy::all)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/ossteam.ipc.v1.rs"));
}

pub use client::{Client, ClientError, ClientInfo, ConnectOptions};
pub use endpoint::Endpoint;

/// The protocol version this build speaks. Peers must share the major
/// version; minor versions only add (SPEC IPC-6).
pub const PROTOCOL_VERSION: v1::ProtocolVersion = v1::ProtocolVersion { major: 1, minor: 0 };

/// An `Error` frame from the daemon.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RemoteError {
    pub code: v1::ErrorCode,
    pub message: String,
    pub detail: String,
}

impl From<v1::Error> for RemoteError {
    fn from(e: v1::Error) -> Self {
        Self {
            code: v1::ErrorCode::try_from(e.code).unwrap_or(v1::ErrorCode::Unspecified),
            message: e.message,
            detail: e.detail,
        }
    }
}

impl From<RemoteError> for v1::Error {
    fn from(e: RemoteError) -> Self {
        Self {
            code: e.code as i32,
            message: e.message,
            detail: e.detail,
        }
    }
}

impl RemoteError {
    pub fn new(code: v1::ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: String::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
}
