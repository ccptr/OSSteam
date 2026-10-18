// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamRemoteStorage` (`docs/spec/steamworks/isteamremotestorage.md`).

use crate::object::Object;

/// No Steam Cloud until M3, for the account or any app.
pub fn cloud_enabled(_: &Object) -> bool {
    false
}
