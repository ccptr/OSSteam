// SPDX-License-Identifier: GPL-2.0-or-later
//! Settings (SPEC D-9, `docs/spec/ipc.md` §5.4): `ossteamd.toml` in the
//! config directory, read at start, plus per-key overrides that clients set
//! over IPC and the state database keeps (D-6). An override wins over the
//! file, and the file over the default. Unknown keys are warned about,
//! never fatal.

use std::collections::BTreeMap;
use std::path::Path;

use ossteam_ipc::v1;

pub const FILE_NAME: &str = "ossteamd.toml";

pub const SECRETS_FILE_FALLBACK: &str = "secrets.file_fallback";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
}

impl Kind {
    /// Parses a value the way `SetSetting` and the database write it.
    fn parse(self, text: &str) -> Result<Value, String> {
        match self {
            Self::Bool => match text.trim() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err("must be true or false".into()),
            },
        }
    }

    fn read_toml(self, value: &toml::Value) -> Result<Value, String> {
        match (self, value) {
            (Self::Bool, toml::Value::Boolean(b)) => Ok(Value::Bool(*b)),
            (Self::Bool, _) => Err("must be true or false".into()),
        }
    }

    fn proto(self) -> v1::SettingKind {
        match self {
            Self::Bool => v1::SettingKind::Bool,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}

pub struct Def {
    pub key: &'static str,
    pub kind: Kind,
    pub default: Value,
    pub description: &'static str,
}

/// Every setting this build knows, sorted by key.
pub const DEFS: &[Def] = &[Def {
    key: SECRETS_FILE_FALLBACK,
    kind: Kind::Bool,
    default: Value::Bool(false),
    description: "When no OS keyring is available, store refresh tokens in a \
                  mode-0600 file in the data directory instead (SPEC D-8).",
}];

pub fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

impl Def {
    /// Parses a value given over IPC or stored in the database.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        self.kind
            .parse(text)
            .map_err(|e| format!("{} {e}", self.key))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    Override,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    file: BTreeMap<&'static str, Value>,
    overrides: BTreeMap<&'static str, Value>,
}

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
        let (file, unknown) =
            Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        for key in unknown {
            tracing::warn!(file = %path.display(), "unknown setting {key:?} ignored");
        }
        Ok(Self {
            file,
            overrides: BTreeMap::new(),
        })
    }

    /// Parses the file's text. Returns its values and the dotted names of
    /// keys this build does not know.
    fn parse(text: &str) -> Result<(BTreeMap<&'static str, Value>, Vec<String>), String> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut values = BTreeMap::new();
        let mut unknown = Vec::new();
        walk(&table, "", &mut values, &mut unknown)?;
        Ok((values, unknown))
    }

    /// Applies the overrides stored in the database. A row this build does
    /// not understand is warned about and skipped; it stays stored.
    pub fn apply_overrides(&mut self, rows: Vec<(String, String)>) {
        for (key, text) in rows {
            match def(&key).map(|d| (d, d.parse(&text))) {
                Some((d, Ok(v))) => {
                    self.overrides.insert(d.key, v);
                }
                Some((_, Err(e))) => {
                    tracing::warn!("stored override {text:?} ignored: {e}");
                }
                None => tracing::warn!("stored override for unknown setting {key:?} ignored"),
            }
        }
    }

    pub fn set_override(&mut self, d: &'static Def, value: Option<Value>) {
        match value {
            Some(v) => self.overrides.insert(d.key, v),
            None => self.overrides.remove(d.key),
        };
    }

    fn get(&self, d: &'static Def) -> (Value, Source) {
        if let Some(v) = self.overrides.get(d.key) {
            (*v, Source::Override)
        } else if let Some(v) = self.file.get(d.key) {
            (*v, Source::File)
        } else {
            (d.default, Source::Default)
        }
    }

    fn bool(&self, key: &str) -> bool {
        let d = def(key).expect("a key from DEFS");
        match self.get(d).0 {
            Value::Bool(b) => b,
        }
    }

    /// `secrets.file_fallback` (SPEC D-8).
    pub fn file_fallback(&self) -> bool {
        self.bool(SECRETS_FILE_FALLBACK)
    }

    /// One setting as clients see it.
    pub fn proto(&self, d: &'static Def) -> v1::Setting {
        let (value, source) = self.get(d);
        v1::Setting {
            key: d.key.into(),
            kind: d.kind.proto() as i32,
            value: value.to_string(),
            source: match source {
                Source::Default => v1::SettingSource::Default,
                Source::File => v1::SettingSource::File,
                Source::Override => v1::SettingSource::Override,
            } as i32,
            default_value: d.default.to_string(),
            file_value: self
                .file
                .get(d.key)
                .map(Value::to_string)
                .unwrap_or_default(),
            description: d.description.into(),
        }
    }

    pub fn all(&self) -> Vec<v1::Setting> {
        DEFS.iter().map(|d| self.proto(d)).collect()
    }
}

/// Flattens nested tables into dotted keys and reads the known ones.
fn walk(
    table: &toml::Table,
    prefix: &str,
    values: &mut BTreeMap<&'static str, Value>,
    unknown: &mut Vec<String>,
) -> Result<(), String> {
    for (name, value) in table {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match (def(&key), value) {
            (Some(d), v) => {
                let v = d.kind.read_toml(v).map_err(|e| format!("{key} {e}"))?;
                values.insert(d.key, v);
            }
            (None, toml::Value::Table(t)) => walk(t, &key, values, unknown)?,
            (None, _) => unknown.push(key),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback() -> &'static Def {
        def(SECRETS_FILE_FALLBACK).unwrap()
    }

    #[test]
    fn defs_are_sorted_and_unique() {
        assert!(DEFS.windows(2).all(|w| w[0].key < w[1].key));
    }

    #[test]
    fn empty_is_default() {
        let (values, unknown) = Config::parse("").unwrap();
        assert!(values.is_empty() && unknown.is_empty());
        let c = Config::default();
        assert!(!c.file_fallback());
        let s = c.proto(fallback());
        assert_eq!(s.source, v1::SettingSource::Default as i32);
        assert_eq!((s.value.as_str(), s.file_value.as_str()), ("false", ""));
    }

    #[test]
    fn reads_known_keys_and_reports_unknown_ones() {
        let (values, unknown) =
            Config::parse("colour = 1\n[secrets]\nfile_fallback = true\nshred = 3\n").unwrap();
        assert_eq!(values.get(SECRETS_FILE_FALLBACK), Some(&Value::Bool(true)));
        assert_eq!(unknown, ["colour", "secrets.shred"]);
        // Dotted keys and nested tables mean the same.
        let (dotted, _) = Config::parse("secrets.file_fallback = true\n").unwrap();
        assert_eq!(dotted, values);
    }

    #[test]
    fn rejects_wrong_types() {
        assert!(Config::parse("[secrets]\nfile_fallback = \"yes\"\n").is_err());
        assert!(Config::parse("secrets = 1\n").is_ok());
        assert!(Config::parse("not toml").is_err());
    }

    #[test]
    fn override_wins_over_file_over_default() {
        let (file, _) = Config::parse("secrets.file_fallback = true\n").unwrap();
        let mut c = Config {
            file,
            overrides: BTreeMap::new(),
        };
        assert!(c.file_fallback());
        assert_eq!(c.proto(fallback()).source, v1::SettingSource::File as i32);

        c.apply_overrides(vec![
            (SECRETS_FILE_FALLBACK.into(), "false".into()),
            ("no.such".into(), "1".into()),
        ]);
        assert!(!c.file_fallback());
        let s = c.proto(fallback());
        assert_eq!(s.source, v1::SettingSource::Override as i32);
        assert_eq!(s.file_value, "true");

        c.set_override(fallback(), None);
        assert!(c.file_fallback());
    }

    #[test]
    fn parses_override_values() {
        assert_eq!(fallback().parse(" true "), Ok(Value::Bool(true)));
        assert_eq!(
            fallback().parse("yes"),
            Err("secrets.file_fallback must be true or false".into())
        );
        assert!(def("secrets").is_none());
    }

    #[test]
    fn bad_stored_override_is_skipped() {
        let mut c = Config::default();
        c.apply_overrides(vec![(SECRETS_FILE_FALLBACK.into(), "maybe".into())]);
        assert_eq!(c, Config::default());
    }
}
