// SPDX-License-Identifier: GPL-2.0-or-later
//! Steam content (`docs/spec/content.md`): content servers, depot keys,
//! manifests, chunks, and the install engine built on them.

pub mod cdn;
pub mod chunk;
pub mod depots;
pub mod download;
pub mod engine;
pub mod key;
pub mod manifest;
pub mod path;
pub mod request;
pub mod script;
pub mod throttle;
pub mod zip;

#[cfg(any(test, feature = "testing"))]
pub mod fake;

pub use key::DepotKey;
pub use manifest::{Chunk, ChunkId, File, Manifest};
