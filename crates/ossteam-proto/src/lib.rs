// SPDX-License-Identifier: GPL-2.0-or-later
//! Steam message types generated from the vendored community protobuf
//! definitions (SPEC LIC-5 to LIC-7; see `PROVENANCE.md`).
//!
//! The upstream files declare no protobuf package, so every message, enum
//! and service lives in [`steam`]. Enum values keep upstream's names, so
//! `k_EMsgClientLogon` becomes `EMsg::KEMsgClientLogon`.

/// Generated messages and enums, plus one module of method markers per
/// unified service (for example [`steam::authentication`]).
#[allow(clippy::all, clippy::pedantic, missing_docs, non_camel_case_types)]
pub mod steam {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}

/// A unified service method, as named in the service definitions.
///
/// The wire form of the name and the EMsgs that carry the call are the
/// network layer's business (`docs/spec/cm.md` §3.5); this trait only ties
/// a method to its schema.
pub trait ServiceMethod {
    /// The service name as declared, for example `Authentication`.
    const SERVICE: &'static str;
    /// The method name as declared, for example `GetPasswordRSAPublicKey`.
    const METHOD: &'static str;
    type Request: prost::Message + Default;
    type Response: prost::Message + Default;
}

pub use prost::Message;
