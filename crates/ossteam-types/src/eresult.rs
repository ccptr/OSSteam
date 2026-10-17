// SPDX-License-Identifier: GPL-2.0-or-later
//! Steam's result code.

use std::fmt;

/// A Steam result code, as carried in message headers and bodies.
///
/// This is a newtype over the wire value, not a closed enum. Steam adds
/// codes over time, and a value this build has no name for must still be
/// passed on and shown truthfully (FAITH-1).
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct EResult(pub i32);

macro_rules! codes {
    ($($name:ident = $v:literal,)*) => {
        impl EResult {
            $(pub const $name: Self = Self($v);)*

            /// The name of a known code.
            pub fn name(self) -> Option<&'static str> {
                match self.0 {
                    $($v => Some(stringify!($name)),)*
                    _ => None,
                }
            }
        }
    };
}

// The values used by the Steam protocol and the public Steamworks API.
// Only codes ossteam handles or reports are named; others print as numbers.
codes! {
    INVALID = 0,
    OK = 1,
    FAIL = 2,
    NO_CONNECTION = 3,
    INVALID_PASSWORD = 5,
    LOGGED_IN_ELSEWHERE = 6,
    INVALID_PROTOCOL_VER = 7,
    INVALID_PARAM = 8,
    FILE_NOT_FOUND = 9,
    BUSY = 10,
    INVALID_STATE = 11,
    ACCESS_DENIED = 15,
    TIMEOUT = 16,
    BANNED = 17,
    ACCOUNT_NOT_FOUND = 18,
    SERVICE_UNAVAILABLE = 20,
    NOT_LOGGED_ON = 21,
    PENDING = 22,
    LIMIT_EXCEEDED = 25,
    REVOKED = 26,
    EXPIRED = 27,
    LOGON_SESSION_REPLACED = 34,
    TRY_ANOTHER_CM = 48,
    ACCOUNT_DISABLED = 43,
    SERVICE_READ_ONLY = 44,
    ALREADY_LOGGED_IN_ELSEWHERE = 50,
    CANCELLED = 52,
    ACCOUNT_LOGON_DENIED = 63,
    INVALID_LOGIN_AUTH_CODE = 65,
    ACCOUNT_LOGON_DENIED_NO_MAIL = 66,
    EXPIRED_LOGIN_AUTH_CODE = 71,
    ACCOUNT_LOCKED_DOWN = 73,
    RATE_LIMIT_EXCEEDED = 84,
    ACCOUNT_LOGIN_DENIED_NEED_TWO_FACTOR = 85,
    ACCOUNT_LOGIN_DENIED_THROTTLE = 87,
    TWO_FACTOR_CODE_MISMATCH = 88,
    NOT_SUPPORTED = 128,
}

impl EResult {
    pub fn is_ok(self) -> bool {
        self == Self::OK
    }
}

impl fmt::Debug for EResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "EResult::{n}({})", self.0),
            None => write!(f, "EResult({})", self.0),
        }
    }
}

impl fmt::Display for EResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "{n} ({})", self.0),
            None => write!(f, "EResult {}", self.0),
        }
    }
}

impl From<i32> for EResult {
    fn from(v: i32) -> Self {
        Self(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_codes_display() {
        assert_eq!(EResult::OK.to_string(), "OK (1)");
        assert_eq!(EResult(9999).to_string(), "EResult 9999");
        assert!(EResult(1).is_ok());
        assert!(!EResult::FAIL.is_ok());
    }
}
