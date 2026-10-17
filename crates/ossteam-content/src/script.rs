// SPDX-License-Identifier: GPL-2.0-or-later
//! Install scripts: parsing, variables and which steps are due
//! (`install-scripts.md` §2). Running them is the daemon's, on Windows.

use ossteam_vdf::{Object, Value};

/// The root key of a script.
const ROOT: &str = "InstallScript";

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("not KeyValues: {0}")]
    Syntax(String),
    #[error("no InstallScript section")]
    NoRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

/// A registry key: its hive and the path below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegKey {
    pub hive: Hive,
    pub path: String,
}

impl RegKey {
    /// `HKEY_LOCAL_MACHINE\Software\...`; `None` for another hive.
    pub fn parse(full: &str) -> Option<Self> {
        let (hive, path) = full.split_once('\\').unwrap_or((full, ""));
        let hive = if hive.eq_ignore_ascii_case("HKEY_LOCAL_MACHINE") {
            Hive::LocalMachine
        } else if hive.eq_ignore_ascii_case("HKEY_CURRENT_USER") {
            Hive::CurrentUser
        } else {
            return None;
        };
        Some(Self {
            hive,
            path: path.trim_matches('\\').to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    String(String),
    Dword(u32),
}

/// One value a script writes (§2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub key: RegKey,
    /// `None` for the key's default value, `(Default)` in the script.
    pub name: Option<String>,
    pub data: Data,
}

/// Conditions on Windows for a Run Process entry (§2.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirement {
    pub is_64bit: Option<bool>,
    pub os_type: Option<String>,
}

impl Requirement {
    /// Whether the entry runs on this Windows. `os_type` is matched
    /// exactly, ignoring case (*unverified*, §2.2).
    pub fn holds(&self, is_64bit: bool, os_type: &str) -> bool {
        self.is_64bit.is_none_or(|want| want == is_64bit)
            && self
                .os_type
                .as_deref()
                .is_none_or(|want| want.eq_ignore_ascii_case(os_type))
    }
}

/// A Run Process or Run Process On Uninstall entry (§2.2, §2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    /// The entry's key, which also names its value under `has_run`.
    pub name: String,
    pub program: String,
    pub arguments: String,
    pub has_run: Option<RegKey>,
    pub minimum_has_run: u32,
    pub no_cleanup: bool,
    pub as_current_user: bool,
    pub requirement: Requirement,
}

impl Process {
    /// Whether the entry is due, given the value recorded under its
    /// `HasRunKey` (`None` when there is none).
    pub fn due(&self, recorded: Option<u32>) -> bool {
        self.has_run.is_none() || recorded.is_none_or(|v| v < self.minimum_has_run)
    }
}

/// A parsed script, with values picked for one language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    pub registry: Vec<RegValue>,
    pub run: Vec<Process>,
    pub run_on_uninstall: Vec<Process>,
    /// Program name and path.
    pub firewall: Vec<(String, String)>,
    /// The script carries a `kvsignatures` block (§2.6).
    pub signed: bool,
    /// What was skipped, for the log: unknown sections, unusable entries.
    pub problems: Vec<String>,
}

/// A scalar value as text; numbers from the binary form included.
fn text(v: &Value) -> Option<String> {
    v.to_text_scalar()
}

fn flag(o: &Object, key: &str) -> bool {
    o.get(key).and_then(text).is_some_and(|v| v.trim() == "1")
}

/// A value that may be an object of per-language values (§2.1).
fn localized(v: &Value, language: &str) -> Option<String> {
    match v {
        Value::Object(o) => o.get(language).or_else(|| o.get("english")).and_then(text),
        v => text(v),
    }
}

impl Script {
    /// Parses a script's text, choosing per-language values for
    /// `language`, falling back to English.
    pub fn parse(source: &str, language: &str) -> Result<Self, ScriptError> {
        let doc =
            ossteam_vdf::text::parse(source).map_err(|e| ScriptError::Syntax(e.to_string()))?;
        let root = doc.get_object(ROOT).ok_or(ScriptError::NoRoot)?;
        let mut s = Script {
            signed: doc
                .get_object("kvsignatures")
                .is_some_and(|k| k.get(ROOT).is_some()),
            ..Default::default()
        };
        for (section, v) in root.iter() {
            let Some(o) = v.as_object() else {
                s.problems.push(format!("{section:?} is not a section"));
                continue;
            };
            match section.to_ascii_lowercase().as_str() {
                "registry" => s.registry(o, language),
                "run process" => {
                    let list = s.processes(o);
                    s.run.extend(list);
                }
                "run process on uninstall" => {
                    let list = s.processes(o);
                    s.run_on_uninstall.extend(list);
                }
                "firewall" => {
                    for (name, path) in o.iter() {
                        match text(path) {
                            Some(p) => s.firewall.push((name.to_owned(), p)),
                            None => s
                                .problems
                                .push(format!("firewall entry {name:?} has no path")),
                        }
                    }
                }
                _ => s.problems.push(format!("unknown section {section:?}")),
            }
        }
        Ok(s)
    }

    fn registry(&mut self, section: &Object, language: &str) {
        for (path, v) in section.iter() {
            let Some(key) = RegKey::parse(path) else {
                self.problems
                    .push(format!("registry key {path:?} is in an unsupported hive"));
                continue;
            };
            let Some(kinds) = v.as_object() else {
                self.problems
                    .push(format!("registry key {path:?} has no values"));
                continue;
            };
            for (kind, values) in kinds.iter() {
                let is_dword = kind.eq_ignore_ascii_case("dword");
                if !is_dword && !kind.eq_ignore_ascii_case("string") {
                    self.problems
                        .push(format!("registry key {path:?}: unknown type {kind:?}"));
                    continue;
                }
                let Some(values) = values.as_object() else {
                    continue;
                };
                for (name, v) in values.iter() {
                    let Some(raw) = localized(v, language) else {
                        self.problems.push(format!(
                            "{path}\\{name}: no value for {language} or english"
                        ));
                        continue;
                    };
                    let data = if is_dword {
                        match parse_dword(&raw) {
                            Some(n) => Data::Dword(n),
                            None => {
                                self.problems
                                    .push(format!("{path}\\{name}: {raw:?} is not a dword"));
                                continue;
                            }
                        }
                    } else {
                        Data::String(raw)
                    };
                    let name = (!name.eq_ignore_ascii_case("(Default)")).then(|| name.to_owned());
                    self.registry.push(RegValue {
                        key: key.clone(),
                        name,
                        data,
                    });
                }
            }
        }
    }

    fn processes(&mut self, section: &Object) -> Vec<Process> {
        let mut out = Vec::new();
        for (name, v) in section.iter() {
            let Some(o) = v.as_object() else {
                self.problems
                    .push(format!("process entry {name:?} is not a section"));
                continue;
            };
            let Some(program) = o
                .get("process 1")
                .and_then(text)
                .filter(|p| !p.trim().is_empty())
            else {
                self.problems
                    .push(format!("process entry {name:?} has no \"process 1\""));
                continue;
            };
            if o.get("process 2").is_some() {
                // Unverified whether the client runs these (§2.2).
                self.problems.push(format!(
                    "process entry {name:?}: \"process 2\" and later are not run"
                ));
            }
            let has_run = match o.get("HasRunKey").and_then(text) {
                None => None,
                Some(k) => match RegKey::parse(&k) {
                    Some(k) => Some(k),
                    None => {
                        self.problems.push(format!(
                            "process entry {name:?}: HasRunKey {k:?} is in an unsupported hive"
                        ));
                        continue;
                    }
                },
            };
            let minimum_has_run = o
                .get("MinimumHasRunValue")
                .and_then(text)
                .and_then(|v| parse_dword(&v))
                .unwrap_or(1);
            let requirement = o
                .get_object("Requirement_OS")
                .map(|r| Requirement {
                    is_64bit: r
                        .get("Is64BitWindows")
                        .and_then(text)
                        .map(|v| v.trim() == "1"),
                    os_type: r.get("OSType").and_then(text).map(|v| v.trim().to_owned()),
                })
                .unwrap_or_default();
            out.push(Process {
                name: name.to_owned(),
                program,
                arguments: o.get("command 1").and_then(text).unwrap_or_default(),
                has_run,
                minimum_has_run,
                no_cleanup: flag(o, "NoCleanUp"),
                as_current_user: flag(o, "AsCurrentUser"),
                requirement,
            });
        }
        out
    }

    /// The script with `%NAME%` variables replaced by `vars` (§2.5).
    /// Unknown variables are left as they are and returned.
    pub fn expand(&self, vars: &dyn Fn(&str) -> Option<String>) -> (Self, Vec<String>) {
        let mut unknown = Vec::new();
        let mut x = |s: &str| expand(s, vars, &mut unknown);
        let process = |p: &Process, x: &mut dyn FnMut(&str) -> String| Process {
            program: x(&p.program),
            arguments: x(&p.arguments),
            ..p.clone()
        };
        let out = Self {
            registry: self
                .registry
                .iter()
                .map(|r| RegValue {
                    data: match &r.data {
                        Data::String(s) => Data::String(x(s)),
                        d => d.clone(),
                    },
                    ..r.clone()
                })
                .collect(),
            run: self.run.iter().map(|p| process(p, &mut x)).collect(),
            run_on_uninstall: self
                .run_on_uninstall
                .iter()
                .map(|p| process(p, &mut x))
                .collect(),
            firewall: self
                .firewall
                .iter()
                .map(|(n, p)| (n.clone(), x(p)))
                .collect(),
            signed: self.signed,
            problems: self.problems.clone(),
        };
        unknown.sort();
        unknown.dedup();
        (out, unknown)
    }

    /// Whether any value is written to `HKEY_LOCAL_MACHINE`.
    pub fn writes_local_machine(&self) -> bool {
        self.registry
            .iter()
            .any(|r| r.key.hive == Hive::LocalMachine)
    }
}

/// A decimal number, or hex with `0x`.
fn parse_dword(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s
            .parse::<u32>()
            .ok()
            .or_else(|| s.parse::<i32>().ok().map(|n| n as u32)),
    }
}

/// Replaces each `%NAME%` that `vars` knows, ignoring case of the name.
fn expand(s: &str, vars: &dyn Fn(&str) -> Option<String>, unknown: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end)
                if end > 0
                    && after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                let name = &after[..end];
                match vars(&name.to_ascii_uppercase()) {
                    Some(v) => out.push_str(&v),
                    None => {
                        unknown.push(name.to_owned());
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
"InstallScript"
{
    "Registry"
    {
        "HKEY_LOCAL_MACHINE\\Software\\Example\\Game"
        {
            "string"
            {
                "InstallPath" "%INSTALLDIR%"
                "(Default)" "Example"
            }
            "dword"
            {
                "Language" { "english" "1" "french" "2" }
                "Installed" "1"
            }
        }
        "HKEY_CLASSES_ROOT\\.example" { "string" { "(Default)" "x" } }
    }
    "Run Process"
    {
        "DirectX"
        {
            "HasRunKey" "HKEY_LOCAL_MACHINE\\Software\\Valve\\Steam\\Apps\\CommonRedist\\DirectX\\Jun2010"
            "process 1" "%INSTALLDIR%\\_CommonRedist\\DirectX\\Jun2010\\DXSETUP.exe"
            "command 1" "/silent"
            "NoCleanUp" "1"
            "MinimumHasRunValue" "2"
            "Requirement_OS" { "Is64BitWindows" "1" }
        }
        "Broken" { "command 1" "/q" }
    }
    "Firewall" { "Example" "%INSTALLDIR%\\game.exe" }
    "Run Process On Uninstall" { "Cleanup" { "process 1" "%INSTALLDIR%\\cleanup.exe" "AsCurrentUser" "1" } }
    "Shortcuts" { }
}
"kvsignatures" { "InstallScript" "abcdef" }
"#;

    #[test]
    fn parses_every_section() {
        let s = Script::parse(SCRIPT, "french").unwrap();
        assert!(s.signed);
        let key = RegKey::parse("HKEY_LOCAL_MACHINE\\Software\\Example\\Game").unwrap();
        assert_eq!(key.path, "Software\\Example\\Game");
        assert_eq!(
            s.registry,
            [
                RegValue {
                    key: key.clone(),
                    name: Some("InstallPath".into()),
                    data: Data::String("%INSTALLDIR%".into())
                },
                RegValue {
                    key: key.clone(),
                    name: None,
                    data: Data::String("Example".into())
                },
                RegValue {
                    key: key.clone(),
                    name: Some("Language".into()),
                    data: Data::Dword(2)
                },
                RegValue {
                    key,
                    name: Some("Installed".into()),
                    data: Data::Dword(1)
                },
            ]
        );
        assert!(s.writes_local_machine());
        assert_eq!(s.run.len(), 1);
        let dx = &s.run[0];
        assert_eq!(dx.name, "DirectX");
        assert_eq!(dx.arguments, "/silent");
        assert!(dx.no_cleanup && !dx.as_current_user);
        assert_eq!(dx.minimum_has_run, 2);
        assert_eq!(dx.requirement.is_64bit, Some(true));
        assert_eq!(
            s.firewall,
            [("Example".into(), "%INSTALLDIR%\\game.exe".into())]
        );
        assert_eq!(s.run_on_uninstall.len(), 1);
        assert!(s.run_on_uninstall[0].as_current_user);
        // The other hive, the entry without a program, the unknown section.
        assert_eq!(s.problems.len(), 3, "{:?}", s.problems);
    }

    #[test]
    fn languages_fall_back_to_english() {
        let s = Script::parse(SCRIPT, "german").unwrap();
        assert!(
            s.registry
                .iter()
                .any(|r| r.name.as_deref() == Some("Language") && r.data == Data::Dword(1))
        );
    }

    #[test]
    fn unsigned_and_broken_scripts() {
        let s = Script::parse(r#""InstallScript" { }"#, "english").unwrap();
        assert!(!s.signed && s.run.is_empty());
        assert!(matches!(
            Script::parse(r#""Other" { }"#, "english"),
            Err(ScriptError::NoRoot)
        ));
        assert!(matches!(
            Script::parse("\"InstallScript\" {", "english"),
            Err(ScriptError::Syntax(_))
        ));
    }

    #[test]
    fn variables_are_replaced_and_unknown_ones_kept() {
        let s = Script::parse(SCRIPT, "english").unwrap();
        let vars = |name: &str| (name == "INSTALLDIR").then(|| "D:\\Games\\Example".to_owned());
        let (x, unknown) = s.expand(&vars);
        assert_eq!(
            x.run[0].program,
            "D:\\Games\\Example\\_CommonRedist\\DirectX\\Jun2010\\DXSETUP.exe"
        );
        assert_eq!(x.firewall[0].1, "D:\\Games\\Example\\game.exe");
        assert!(unknown.is_empty());
        let mut unknown = Vec::new();
        assert_eq!(
            expand("%a b% 100% %Missing%x", &vars, &mut unknown),
            "%a b% 100% %Missing%x"
        );
        assert_eq!(unknown, ["Missing"]);
        let mut unknown = Vec::new();
        assert_eq!(
            expand("%installdir%\\a", &vars, &mut unknown),
            "D:\\Games\\Example\\a"
        );
    }

    #[test]
    fn due_and_requirements() {
        let s = Script::parse(SCRIPT, "english").unwrap();
        let dx = &s.run[0];
        assert!(dx.due(None));
        assert!(dx.due(Some(1)));
        assert!(!dx.due(Some(2)));
        let mut free = dx.clone();
        free.has_run = None;
        assert!(free.due(Some(5)));
        assert!(dx.requirement.holds(true, "Win10"));
        assert!(!dx.requirement.holds(false, "Win10"));
        let xp = Requirement {
            is_64bit: None,
            os_type: Some("WinXP".into()),
        };
        assert!(xp.holds(true, "winxp") && !xp.holds(true, "Win7"));
        assert_eq!(parse_dword("0x10"), Some(16));
        assert_eq!(parse_dword("-1"), Some(u32::MAX));
    }
}
