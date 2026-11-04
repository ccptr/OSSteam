// SPDX-License-Identifier: GPL-2.0-or-later
//! Settings (SPEC D-9, `docs/spec/ipc.md` §5.4): `ossteamd.toml` in the
//! config directory, read at start, plus per-key overrides that clients set
//! over IPC and the state database keeps (D-6). An override wins over the
//! file, and the file over the default. Unknown keys are warned about,
//! never fatal.

use std::collections::BTreeMap;
use std::path::Path;

use ossteam_ipc::v1;
use ossteam_types::AppId;

pub const FILE_NAME: &str = "ossteamd.toml";

pub const COMPAT_DEFAULT_TOOL: &str = "compat.default_tool";
/// The family `compat.tool.<app id>` (`compat-tools.md` §8).
pub const COMPAT_TOOL: &str = "compat.tool.<app id>";
const COMPAT_TOOL_PREFIX: &str = "compat.tool.";
pub const SECRETS_FILE_FALLBACK: &str = "secrets.file_fallback";
pub const DISCOVERY_TAKE_OVER_OFFICIAL: &str = "discovery.take_over_official";
pub const CONTENT_AUTO_UPDATE: &str = "content.auto_update";
pub const CONTENT_BANDWIDTH_LIMIT: &str = "content.bandwidth_limit";
pub const CONTENT_CONCURRENT_JOBS: &str = "content.concurrent_jobs";
pub const CONTENT_DOWNLOAD_THREADS: &str = "content.download_threads";
pub const CONTENT_LANGUAGE: &str = "content.language";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// A whole number in `min..=max`.
    Integer {
        min: i64,
        max: i64,
    },
    /// One of a fixed list of words.
    Choice(&'static [&'static str]),
    /// Any text, trimmed; what it may name is checked where it is used.
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    /// One of a [`Kind::Choice`]'s words.
    Word(&'static str),
    Text(String),
}

/// The languages Steam names depots by (`content.md` §6, §13).
const LANGUAGES: &[&str] = &[
    "arabic",
    "brazilian",
    "bulgarian",
    "czech",
    "danish",
    "dutch",
    "english",
    "finnish",
    "french",
    "german",
    "greek",
    "hungarian",
    "indonesian",
    "italian",
    "japanese",
    "koreana",
    "latam",
    "norwegian",
    "polish",
    "portuguese",
    "romanian",
    "russian",
    "schinese",
    "spanish",
    "swedish",
    "tchinese",
    "thai",
    "turkish",
    "ukrainian",
    "vietnamese",
];

impl Kind {
    /// Parses a value the way `SetSetting` and the database write it.
    fn parse(self, text: &str) -> Result<Value, String> {
        match self {
            Self::Bool => match text.trim() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(self.expected()),
            },
            Self::Integer { .. } => text
                .trim()
                .parse()
                .map_err(|_| self.expected())
                .and_then(|n| self.int(n)),
            Self::Choice(words) => words
                .iter()
                .find(|w| **w == text.trim())
                .map(|w| Value::Word(w))
                .ok_or_else(|| self.expected()),
            Self::Text => Ok(Value::Text(text.trim().to_owned())),
        }
    }

    fn int(self, n: i64) -> Result<Value, String> {
        match self {
            Self::Integer { min, max } if (min..=max).contains(&n) => Ok(Value::Int(n)),
            _ => Err(self.expected()),
        }
    }

    fn expected(self) -> String {
        match self {
            Self::Bool => "must be true or false".into(),
            Self::Integer { min, max } => format!("must be a whole number from {min} to {max}"),
            Self::Choice(words) => format!("must be one of {}", words.join(", ")),
            Self::Text => "must be text".into(),
        }
    }

    fn read_toml(self, value: &toml::Value) -> Result<Value, String> {
        match (self, value) {
            (Self::Bool, toml::Value::Boolean(b)) => Ok(Value::Bool(*b)),
            (Self::Integer { .. }, toml::Value::Integer(n)) => self.int(*n),
            (Self::Choice(_) | Self::Text, toml::Value::String(s)) => self.parse(s),
            _ => Err(self.expected()),
        }
    }

    fn proto(self) -> v1::SettingKind {
        match self {
            Self::Bool => v1::SettingKind::Bool,
            Self::Integer { .. } => v1::SettingKind::Integer,
            Self::Choice(_) | Self::Text => v1::SettingKind::String,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Word(w) => f.write_str(w),
            Self::Text(t) => f.write_str(t),
        }
    }
}

pub struct Def {
    /// For a family, the key with `<app id>` where the app id goes.
    pub key: &'static str,
    pub kind: Kind,
    pub default: Value,
    pub description: &'static str,
}

/// `discovery.take_over_official`'s description, which states its risks
/// (`steamworks-abi.md` §1.1.5, §1.1.6). The CLI prints it and the GUI asks
/// to confirm with it when the setting is turned on.
pub const TAKE_OVER_RISKS: &str = "Linux: take over an official Steam client's ~/.steam \
    entries while it is not running, hand them back afterwards, and yield them when it starts. \
    Risks: when games get no private ~/.steam, what a game ossteam started starts after the \
    official client has started (a launcher's real game, a game restarting itself) finds the \
    official client and plays under its account; and if the official client starts just as \
    ossteam takes over, the games it starts may find ossteam and play under ossteam's account.";

/// The risks alone, one sentence each, for the daemon's warning line.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const TAKE_OVER_RISK_LINE: &str = "when games get no private ~/.steam, what a game ossteam \
    started starts after the official Steam client has started finds that client and its \
    account. If the official client starts just as ossteam takes over, the games it starts may \
    find ossteam and play under ossteam's account.";

/// Every setting this build knows, sorted by key.
pub const DEFS: &[Def] = &[
    Def {
        key: COMPAT_DEFAULT_TOOL,
        kind: Kind::Text,
        default: Value::Word("proton_experimental"),
        description: "Linux: the compatibility tool for apps without a Linux version that no \
                      other mapping covers, by name; empty for none.",
    },
    Def {
        key: COMPAT_TOOL,
        kind: Kind::Text,
        default: Value::Word(""),
        description: "Linux: the compatibility tool for one app, by name or selectable alias; \
                      empty for Valve's mapping.",
    },
    Def {
        key: CONTENT_AUTO_UPDATE,
        kind: Kind::Choice(&["always", "never"]),
        default: Value::Word("always"),
        description: "Whether an update job is queued when an installed app has a new build \
                      (always), or the app is only marked (never).",
    },
    Def {
        key: CONTENT_BANDWIDTH_LIMIT,
        kind: Kind::Integer {
            min: 0,
            max: 1 << 30,
        },
        default: Value::Int(0),
        description: "KiB/s for all downloads together; 0 is no limit.",
    },
    Def {
        key: CONTENT_CONCURRENT_JOBS,
        kind: Kind::Integer { min: 1, max: 8 },
        default: Value::Int(1),
        description: "Content jobs that run at once.",
    },
    Def {
        key: CONTENT_DOWNLOAD_THREADS,
        kind: Kind::Integer { min: 1, max: 64 },
        default: Value::Int(8),
        description: "Chunk downloads at once for each job; applies to jobs started after a change.",
    },
    Def {
        key: CONTENT_LANGUAGE,
        kind: Kind::Choice(LANGUAGES),
        default: Value::Word("english"),
        description: "The language whose depots are installed, as Steam names it.",
    },
    Def {
        key: DISCOVERY_TAKE_OVER_OFFICIAL,
        kind: Kind::Bool,
        default: Value::Bool(false),
        description: TAKE_OVER_RISKS,
    },
    Def {
        key: SECRETS_FILE_FALLBACK,
        kind: Kind::Bool,
        default: Value::Bool(false),
        description: "When no OS keyring is available, store refresh tokens in a \
                      mode-0600 file in the data directory instead (SPEC D-8).",
    },
];

/// The setting `key` names: one of [`DEFS`], or a member of a family
/// such as `compat.tool.570`.
pub fn def(key: &str) -> Option<&'static Def> {
    if family_app(key).is_some() {
        return DEFS.iter().find(|d| d.key == COMPAT_TOOL);
    }
    DEFS.iter().find(|d| d.key == key && !d.is_family())
}

/// The app of a `compat.tool.<app id>` key.
fn family_app(key: &str) -> Option<AppId> {
    let id = key.strip_prefix(COMPAT_TOOL_PREFIX)?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .then(|| id.parse().ok().map(AppId))
        .flatten()
}

/// The key of `app`'s `compat.tool` setting.
pub fn compat_tool_key(app: AppId) -> String {
    format!("{COMPAT_TOOL_PREFIX}{}", app.0)
}

impl Def {
    /// Parses a value given over IPC or stored in the database.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        self.kind
            .parse(text)
            .map_err(|e| format!("{} {e}", self.key))
    }

    pub fn is_family(&self) -> bool {
        self.key.contains('<')
    }
}

/// A key as stored: a family member's own key (`compat.tool.570`) or a
/// plain one. `key` must be one [`def`] accepts.
pub fn canonical(key: &str) -> String {
    match family_app(key) {
        Some(app) => compat_tool_key(app),
        None => key.to_owned(),
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
    /// Keyed as [`canonical`] writes them.
    file: BTreeMap<String, Value>,
    overrides: BTreeMap<String, Value>,
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
    fn parse(text: &str) -> Result<(BTreeMap<String, Value>, Vec<String>), String> {
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
            match def(&key).map(|d| d.parse(&text)) {
                Some(Ok(v)) => {
                    self.overrides.insert(canonical(&key), v);
                }
                Some(Err(e)) => {
                    tracing::warn!("stored override {text:?} ignored: {e}");
                }
                None => tracing::warn!("stored override for unknown setting {key:?} ignored"),
            }
        }
    }

    /// Sets or removes `key`'s override; `key` is one [`def`] accepts.
    pub fn set_override(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(v) => self.overrides.insert(canonical(key), v),
            None => self.overrides.remove(&canonical(key)),
        };
    }

    fn get(&self, key: &str) -> (&Value, Source) {
        let d = def(key).expect("a known key");
        let key = canonical(key);
        if let Some(v) = self.overrides.get(&key) {
            (v, Source::Override)
        } else if let Some(v) = self.file.get(&key) {
            (v, Source::File)
        } else {
            (&d.default, Source::Default)
        }
    }

    fn value(&self, key: &str) -> &Value {
        self.get(key).0
    }

    fn bool(&self, key: &str) -> bool {
        match self.value(key) {
            Value::Bool(b) => *b,
            v => unreachable!("{key} is {v:?}"),
        }
    }

    fn int(&self, key: &str) -> i64 {
        match self.value(key) {
            Value::Int(n) => *n,
            v => unreachable!("{key} is {v:?}"),
        }
    }

    fn word(&self, key: &str) -> &'static str {
        match self.value(key) {
            Value::Word(w) => w,
            v => unreachable!("{key} is {v:?}"),
        }
    }

    fn text(&self, key: &str) -> &str {
        match self.value(key) {
            Value::Text(t) => t,
            Value::Word(w) => w,
            v => unreachable!("{key} is {v:?}"),
        }
    }

    /// `compat.default_tool`; empty for none.
    pub fn default_tool(&self) -> &str {
        self.text(COMPAT_DEFAULT_TOOL)
    }

    /// `compat.tool.<app>`; empty for Valve's mapping.
    pub fn app_tool(&self, app: AppId) -> &str {
        self.text(&compat_tool_key(app))
    }

    /// `content.auto_update` is `always`.
    pub fn auto_update(&self) -> bool {
        self.word(CONTENT_AUTO_UPDATE) == "always"
    }

    /// `content.bandwidth_limit` in bytes per second; 0 is no limit.
    pub fn bandwidth_limit(&self) -> u64 {
        (self.int(CONTENT_BANDWIDTH_LIMIT) as u64).saturating_mul(1024)
    }

    pub fn concurrent_jobs(&self) -> usize {
        self.int(CONTENT_CONCURRENT_JOBS) as usize
    }

    pub fn download_threads(&self) -> usize {
        self.int(CONTENT_DOWNLOAD_THREADS) as usize
    }

    pub fn language(&self) -> &'static str {
        self.word(CONTENT_LANGUAGE)
    }

    /// `discovery.take_over_official` (DISC-5).
    pub fn take_over_official(&self) -> bool {
        self.bool(DISCOVERY_TAKE_OVER_OFFICIAL)
    }

    /// `secrets.file_fallback` (SPEC D-8).
    pub fn file_fallback(&self) -> bool {
        self.bool(SECRETS_FILE_FALLBACK)
    }

    /// One setting as clients see it; `key` is one [`def`] accepts.
    pub fn proto(&self, key: &str) -> v1::Setting {
        let d = def(key).expect("a known key");
        let key = canonical(key);
        let (value, source) = self.get(&key);
        v1::Setting {
            key: key.clone(),
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
                .get(&key)
                .map(Value::to_string)
                .unwrap_or_default(),
            description: d.description.into(),
        }
    }

    /// Every plain setting, and the members of families that the file
    /// or an override sets, sorted by key.
    pub fn all(&self) -> Vec<v1::Setting> {
        let mut keys: Vec<String> = DEFS
            .iter()
            .filter(|d| !d.is_family())
            .map(|d| d.key.to_owned())
            .collect();
        for k in self.file.keys().chain(self.overrides.keys()) {
            if family_app(k).is_some() && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        keys.sort();
        keys.iter().map(|k| self.proto(k)).collect()
    }
}

/// Flattens nested tables into dotted keys and reads the known ones.
fn walk(
    table: &toml::Table,
    prefix: &str,
    values: &mut BTreeMap<String, Value>,
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
                values.insert(canonical(&key), v);
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

    #[test]
    fn integer_and_choice_settings() {
        let (file, _) = Config::parse(
            "[content]\nbandwidth_limit = 512\nlanguage = \"german\"\nauto_update = \"never\"\n",
        )
        .unwrap();
        let c = Config {
            file,
            overrides: BTreeMap::new(),
        };
        assert_eq!(c.bandwidth_limit(), 512 * 1024);
        assert_eq!(c.language(), "german");
        assert!(!c.auto_update());
        assert_eq!(c.download_threads(), 8);
        assert!(Config::parse("[content]\ndownload_threads = 65\n").is_err());
        assert!(Config::parse("[content]\nlanguage = \"klingon\"\n").is_err());
        assert!(Config::parse("[content]\nconcurrent_jobs = \"2\"\n").is_err());
        let d = def(CONTENT_CONCURRENT_JOBS).unwrap();
        assert_eq!(d.parse(" 3 "), Ok(Value::Int(3)));
        assert!(d.parse("0").is_err());
    }

    fn fallback() -> &'static Def {
        def(SECRETS_FILE_FALLBACK).unwrap()
    }

    #[test]
    fn defs_are_sorted_and_unique() {
        assert!(DEFS.windows(2).all(|w| w[0].key < w[1].key));
        // Each family member's key sorts after the plain keys before it.
        assert!(def(COMPAT_DEFAULT_TOOL).is_some_and(|d| !d.is_family()));
    }

    #[test]
    fn empty_is_default() {
        let (values, unknown) = Config::parse("").unwrap();
        assert!(values.is_empty() && unknown.is_empty());
        let c = Config::default();
        assert!(!c.file_fallback());
        let s = c.proto(SECRETS_FILE_FALLBACK);
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
        assert_eq!(
            c.proto(SECRETS_FILE_FALLBACK).source,
            v1::SettingSource::File as i32
        );

        c.apply_overrides(vec![
            (SECRETS_FILE_FALLBACK.into(), "false".into()),
            ("no.such".into(), "1".into()),
        ]);
        assert!(!c.file_fallback());
        let s = c.proto(SECRETS_FILE_FALLBACK);
        assert_eq!(s.source, v1::SettingSource::Override as i32);
        assert_eq!(s.file_value, "true");

        c.set_override(SECRETS_FILE_FALLBACK, None);
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
    fn compat_tool_family() {
        let (file, unknown) =
            Config::parse("[compat]\ndefault_tool = \"\"\n[compat.tool]\n570 = \"proton_9\"\n")
                .unwrap();
        assert!(unknown.is_empty());
        let mut c = Config {
            file,
            overrides: BTreeMap::new(),
        };
        assert_eq!(c.default_tool(), "");
        assert_eq!(c.app_tool(AppId(570)), "proton_9");
        assert_eq!(c.app_tool(AppId(730)), "");
        assert_eq!(Config::default().default_tool(), "proton_experimental");

        c.apply_overrides(vec![("compat.tool.730".into(), " sniper ".into())]);
        assert_eq!(c.app_tool(AppId(730)), "sniper");
        let keys: Vec<String> = c.all().into_iter().map(|s| s.key).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert!(keys.contains(&"compat.tool.570".to_owned()));
        assert!(keys.contains(&"compat.tool.730".to_owned()));
        assert!(!keys.iter().any(|k| k.contains('<')));
        let s = c.proto("compat.tool.730");
        assert_eq!(s.source, v1::SettingSource::Override as i32);

        // Leading zeros name the same app.
        c.set_override("compat.tool.0730", None);
        assert_eq!(c.app_tool(AppId(730)), "");
        for bad in [
            "compat.tool.",
            "compat.tool.x",
            "compat.tool.99999999999",
            COMPAT_TOOL,
        ] {
            assert!(def(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn bad_stored_override_is_skipped() {
        let mut c = Config::default();
        c.apply_overrides(vec![(SECRETS_FILE_FALLBACK.into(), "maybe".into())]);
        assert_eq!(c, Config::default());
    }
}
