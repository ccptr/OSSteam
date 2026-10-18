// SPDX-License-Identifier: GPL-2.0-or-later
//! Compatibility tools on Linux (`compat-tools.md`, COMPAT-1 to
//! COMPAT-3): Valve's tool list, which tool an app gets, the tools'
//! manifests, and the command line and environment of a launch through
//! them.
//!
//! This is plain data and compiles everywhere; only the Linux daemon
//! uses it (PLAT-2).

pub mod chain;
pub mod manifest;
pub mod tools;

pub use chain::{ChainTool, CompatEnv, Verb};
pub use manifest::ToolManifest;
pub use tools::{Mapping, MappingInput, Source, Tool, ToolList};

use ossteam_types::AppId;

/// "SteamPlay 2.0 Manifests", whose info holds the tool list (§1).
pub const MANIFESTS_APP: AppId = AppId(891390);
