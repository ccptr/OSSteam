// SPDX-License-Identifier: GPL-2.0-or-later
//! Licenses, product info (PICS) and the PICS cache (SPEC §9.3, PICS-1 to
//! PICS-3, D-7; `docs/spec/pics.md`).
//!
//! - [`license`]: the license list and which licenses grant something.
//! - [`info`]: app and package info buffers and typed views of them.
//! - [`request`]: the PICS requests over a CM connection.
//! - [`cache`]: the on-disk cache, keyed by change number.
//! - [`library`]: the apps an account's licenses grant.
//!
//! The daemon decides when to call what; this crate holds no session
//! state.

pub mod cache;
pub mod info;
pub mod library;
pub mod license;
pub mod request;

pub use cache::Cache;
pub use info::{AppInfo, Info, PackageInfo};
pub use library::{Library, LibraryApp};
pub use license::License;
