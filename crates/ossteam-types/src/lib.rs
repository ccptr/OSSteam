// SPDX-License-Identifier: GPL-2.0-or-later
//! Steam identifiers and other plain types shared across ossteam crates.
//!
//! This crate does no I/O. Everything here is a value type.

use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod eresult;
mod secret;
pub use eresult::EResult;
pub use secret::Secret;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident($inner:ty)) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub $inner);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }

        impl From<$inner> for $name {
            fn from(v: $inner) -> Self {
                Self(v)
            }
        }
    };
}

id_type!(
    /// An application (game, tool, DLC, ...) id.
    AppId(u32)
);
id_type!(
    /// A depot id.
    DepotId(u32)
);
id_type!(
    /// A depot manifest id (gid).
    ManifestId(u64)
);
id_type!(
    /// A package (license bundle, "sub") id.
    PackageId(u32)
);

/// The universe field of a [`SteamId`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Universe {
    Invalid,
    Public,
    Beta,
    Internal,
    Dev,
    Other(u8),
}

impl From<u8> for Universe {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Invalid,
            1 => Self::Public,
            2 => Self::Beta,
            3 => Self::Internal,
            4 => Self::Dev,
            n => Self::Other(n),
        }
    }
}

impl From<Universe> for u8 {
    fn from(u: Universe) -> u8 {
        match u {
            Universe::Invalid => 0,
            Universe::Public => 1,
            Universe::Beta => 2,
            Universe::Internal => 3,
            Universe::Dev => 4,
            Universe::Other(n) => n,
        }
    }
}

/// The account type field of a [`SteamId`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AccountType {
    Invalid,
    Individual,
    Multiseat,
    GameServer,
    AnonGameServer,
    Pending,
    ContentServer,
    Clan,
    Chat,
    ConsoleUser,
    AnonUser,
    Other(u8),
}

impl From<u8> for AccountType {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Invalid,
            1 => Self::Individual,
            2 => Self::Multiseat,
            3 => Self::GameServer,
            4 => Self::AnonGameServer,
            5 => Self::Pending,
            6 => Self::ContentServer,
            7 => Self::Clan,
            8 => Self::Chat,
            9 => Self::ConsoleUser,
            10 => Self::AnonUser,
            n => Self::Other(n),
        }
    }
}

impl From<AccountType> for u8 {
    fn from(t: AccountType) -> u8 {
        match t {
            AccountType::Invalid => 0,
            AccountType::Individual => 1,
            AccountType::Multiseat => 2,
            AccountType::GameServer => 3,
            AccountType::AnonGameServer => 4,
            AccountType::Pending => 5,
            AccountType::ContentServer => 6,
            AccountType::Clan => 7,
            AccountType::Chat => 8,
            AccountType::ConsoleUser => 9,
            AccountType::AnonUser => 10,
            AccountType::Other(n) => n,
        }
    }
}

/// A 64-bit Steam id.
///
/// Bit layout, most significant first: universe (8), account type (4),
/// instance (20), account id (32).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SteamId(pub u64);

impl SteamId {
    /// The instance value used for individual desktop accounts.
    pub const DESKTOP_INSTANCE: u32 = 1;

    pub fn new(
        universe: Universe,
        account_type: AccountType,
        instance: u32,
        account_id: u32,
    ) -> Self {
        let u = u64::from(u8::from(universe));
        let t = u64::from(u8::from(account_type) & 0xf);
        let i = u64::from(instance & 0xf_ffff);
        Self((u << 56) | (t << 52) | (i << 32) | u64::from(account_id))
    }

    /// An individual account in the public universe.
    pub fn individual(account_id: u32) -> Self {
        Self::new(
            Universe::Public,
            AccountType::Individual,
            Self::DESKTOP_INSTANCE,
            account_id,
        )
    }

    pub fn account_id(self) -> u32 {
        self.0 as u32
    }

    pub fn instance(self) -> u32 {
        ((self.0 >> 32) & 0xf_ffff) as u32
    }

    pub fn account_type(self) -> AccountType {
        AccountType::from(((self.0 >> 52) & 0xf) as u8)
    }

    pub fn universe(self) -> Universe {
        Universe::from((self.0 >> 56) as u8)
    }

    /// The textual "Steam3" form, e.g. `[U:1:22202]`, for account types
    /// that have one. Returns `None` for the others.
    pub fn steam3(self) -> Option<String> {
        let letter = match self.account_type() {
            AccountType::Individual => 'U',
            AccountType::Multiseat => 'M',
            AccountType::GameServer => 'G',
            AccountType::AnonGameServer => 'A',
            AccountType::Pending => 'P',
            AccountType::ContentServer => 'C',
            AccountType::Clan => 'g',
            AccountType::AnonUser => 'a',
            _ => return None,
        };
        Some(format!(
            "[{letter}:{}:{}]",
            u8::from(self.universe()),
            self.account_id()
        ))
    }
}

impl fmt::Display for SteamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for SteamId {
    type Err = ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// Cooperative cancellation (SPEC CONC-2).
///
/// Clones share one flag. Blocking loops check it at least once a second.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_id_fields_round_trip() {
        let id = SteamId::individual(22202);
        assert_eq!(id.0, 76561197960287930);
        assert_eq!(id.account_id(), 22202);
        assert_eq!(id.instance(), 1);
        assert_eq!(id.account_type(), AccountType::Individual);
        assert_eq!(id.universe(), Universe::Public);
        assert_eq!(id.steam3().as_deref(), Some("[U:1:22202]"));
    }

    #[test]
    fn steam_id_masks_out_of_range_fields() {
        let id = SteamId::new(Universe::Public, AccountType::Other(0x1f), 0xfff_ffff, 7);
        assert_eq!(id.account_type(), AccountType::Other(0xf));
        assert_eq!(id.instance(), 0xf_ffff);
        assert_eq!(id.universe(), Universe::Public);
    }

    #[test]
    fn ids_parse_and_display() {
        assert_eq!("480".parse::<AppId>().unwrap(), AppId(480));
        assert_eq!(AppId(480).to_string(), "480");
        assert!("x".parse::<DepotId>().is_err());
    }
}
