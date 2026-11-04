// SPDX-License-Identifier: GPL-2.0-or-later
//! Launching games (`docs/spec/launch.md`, `launch-env.md`,
//! `compat-tools.md`, `steamworks-abi.md` §1): which launch option to
//! use, the compatibility tools, the command line and environment,
//! SteamStub detection, the game's process tree, and the entries and the
//! private `~/.steam` that let `steam_api` find a client.
//!
//! Everything here is synchronous (CONC-1). The daemon owns the policy:
//! the checks of `launch.md` §2, the play session and the IPC.

pub mod command;
pub mod compat;
pub mod discovery;
pub mod env;
pub mod isolation;
pub mod options;
pub mod process;
pub mod steamstub;

pub use options::{LaunchOption, Platform};
