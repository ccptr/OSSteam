// SPDX-License-Identifier: GPL-2.0-or-later
//! The Steam connection manager (CM) client (SPEC §9.1, NET-1 to NET-4;
//! `docs/spec/cm.md`).
//!
//! - [`directory`]: finding servers and backing off failed ones.
//! - [`transport`]: the WebSocket transport and connection setup.
//! - [`msg`] and [`multi`]: Steam message framing.
//! - [`conn`]: a live connection with jobs, service calls and heartbeats.
//!
//! Logon and the session state machine live above this crate.

pub mod conn;
pub mod directory;
pub mod msg;
pub mod multi;
pub mod tls;
pub mod transport;

pub use conn::{CallError, CloseReason, Connection, Incoming, SessionIds};
pub use directory::{CmKind, CmServer, ServerPicker};
pub use msg::{Header, JOB_NONE, Packet};
pub use transport::ConnectError;
