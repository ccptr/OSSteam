// SPDX-License-Identifier: GPL-2.0-or-later
//! A string that must never be logged (SEC-2, D-10).

use std::fmt;

use zeroize::Zeroizing;

/// A password, token, Steam Guard code or similar.
///
/// The buffer is zeroed when dropped. `Debug` shows only the length, there
/// is no `Display`, and the content is reachable only through
/// [`Secret::expose`], so it cannot reach a log by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(s: String) -> Self {
        Self(Zeroizing::new(s))
    }

    /// The secret itself. Call this only where the value leaves the
    /// process (a request body, a keyring write).
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({} bytes)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_content() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "Secret(7 bytes)");
        assert_eq!(s.expose(), "hunter2");
    }
}
