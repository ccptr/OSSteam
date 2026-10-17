// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteamd.toml` in the config directory (SPEC D-9). Unknown keys are
//! warned about, never fatal.

use std::path::Path;

/// The daemon's settings. Every field has a default, so a missing file is
/// the same as an empty one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// `secrets.file_fallback`: when no OS keyring is available, store
    /// refresh tokens in a mode-0600 file instead (SPEC D-8). Off unless
    /// the user turns it on.
    pub secrets_file_fallback: bool,
}

pub const FILE_NAME: &str = "ossteamd.toml";

impl Config {
    /// Reads the config file in `dir`. A missing file gives the defaults.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
            }
        };
        let (config, unknown) =
            Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        for key in unknown {
            tracing::warn!(file = %path.display(), "unknown setting {key:?} ignored");
        }
        Ok(config)
    }

    /// Parses the file's text. Returns the config and the dotted names of
    /// keys this build does not know.
    fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut config = Self::default();
        let mut unknown = Vec::new();
        for (section, value) in &table {
            match (section.as_str(), value) {
                ("secrets", toml::Value::Table(t)) => {
                    for (key, value) in t {
                        match (key.as_str(), value) {
                            ("file_fallback", toml::Value::Boolean(b)) => {
                                config.secrets_file_fallback = *b;
                            }
                            ("file_fallback", _) => {
                                return Err("secrets.file_fallback must be true or false".into());
                            }
                            _ => unknown.push(format!("secrets.{key}")),
                        }
                    }
                }
                _ => unknown.push(section.clone()),
            }
        }
        Ok((config, unknown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), (Config::default(), vec![]));
    }

    #[test]
    fn reads_known_keys_and_reports_unknown_ones() {
        let (c, unknown) =
            Config::parse("colour = 1\n[secrets]\nfile_fallback = true\nshred = 3\n").unwrap();
        assert!(c.secrets_file_fallback);
        assert_eq!(unknown, ["colour", "secrets.shred"]);
    }

    #[test]
    fn rejects_wrong_types() {
        assert!(Config::parse("[secrets]\nfile_fallback = \"yes\"\n").is_err());
        assert!(Config::parse("not toml").is_err());
    }
}
