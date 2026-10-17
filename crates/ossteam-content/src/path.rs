// SPDX-License-Identifier: GPL-2.0-or-later
//! Paths named by manifests and app info, checked before anything is
//! written under them (`content.md` §4 "Paths", §8).
//!
//! The data comes from the network, so a name that could leave the install
//! directory, or that the file system would reinterpret, is refused rather
//! than repaired.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{name:?} is not a safe relative path: {why}")]
pub struct PathError {
    pub name: String,
    pub why: &'static str,
}

/// Splits a manifest name on `\` and `/` and checks every component.
/// `windows` adds the rules for names Windows cannot hold; callers pass
/// `cfg!(windows)`.
pub fn components(name: &str, windows: bool) -> Result<Vec<&str>, PathError> {
    let err = |why| PathError {
        name: name.to_owned(),
        why,
    };
    if name.is_empty() {
        return Err(err("it is empty"));
    }
    if name.starts_with(['/', '\\']) {
        return Err(err("it is absolute"));
    }
    if name.len() >= 2 && name.as_bytes()[1] == b':' {
        return Err(err("it has a drive prefix"));
    }
    let parts: Vec<&str> = name.split(['/', '\\']).collect();
    for part in &parts {
        check_component(part, windows).map_err(err)?;
    }
    Ok(parts)
}

/// Checks one component, such as an app's `installdir`.
pub fn check_component(c: &str, windows: bool) -> Result<(), &'static str> {
    if c.is_empty() {
        return Err("it has an empty component");
    }
    if c == "." || c == ".." {
        return Err("it has a . or .. component");
    }
    if c.contains('\0') {
        return Err("it contains NUL");
    }
    if c.contains(['/', '\\']) {
        return Err("it contains a separator");
    }
    if windows {
        if c.chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err("it has a character Windows does not allow in names");
        }
        if c.ends_with([' ', '.']) {
            return Err("it ends in a space or dot, which Windows drops");
        }
        let stem = c.split('.').next().unwrap_or(c).trim_end();
        let reserved = ["CON", "PRN", "AUX", "NUL"];
        let numbered = ["COM", "LPT"];
        let upper = stem.to_ascii_uppercase();
        if reserved.contains(&upper.as_str())
            || (upper.len() == 4
                && numbered.contains(&&upper[..3])
                && upper.as_bytes()[3].is_ascii_digit()
                && upper.as_bytes()[3] != b'0')
        {
            return Err("it is a reserved device name on Windows");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_both_separators() {
        assert_eq!(
            components("bin\\x64/game.exe", false).unwrap(),
            ["bin", "x64", "game.exe"]
        );
    }

    #[test]
    fn refuses_escapes() {
        for bad in [
            "",
            "/etc/passwd",
            "\\x",
            "C:\\x",
            "a\\..\\..\\b",
            "a//b",
            "./a",
            "a\0b",
        ] {
            assert!(components(bad, false).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn windows_rules_apply_only_when_asked() {
        for bad in [
            "con",
            "Con.txt",
            "lpt1",
            "a:b",
            "x?",
            "trailing.",
            "trailing ",
        ] {
            assert!(components(bad, true).is_err(), "{bad:?}");
            assert!(
                components(bad, false).is_ok() || bad.contains(':'),
                "{bad:?}"
            );
        }
        assert!(components("lpt0", true).is_ok());
        assert!(components("console.log", true).is_ok());
    }
}
