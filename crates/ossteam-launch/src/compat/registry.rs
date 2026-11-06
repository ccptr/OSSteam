// SPDX-License-Identifier: GPL-2.0-or-later
//! A Proton prefix's registry, for install scripts (`compat-tools.md`
//! §7): Wine's own files read, the 32-bit view of a 64-bit prefix, and
//! the files `regedit` imports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A value's type and data, as far as install scripts read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `REG_SZ`.
    String(String),
    /// `REG_DWORD`.
    Dword(u32),
    /// Any other type.
    Other,
}

/// One of Wine's registry files: keys by their path, lowercased, and in
/// each its values by name, lowercased (`None` for the default value).
#[derive(Debug, Default)]
pub struct Hive {
    /// `#arch=win64` in the file's header.
    pub win64: bool,
    keys: HashMap<String, HashMap<Option<String>, Value>>,
}

impl Hive {
    /// Parses Wine's `WINE REGISTRY Version 2` text.
    pub fn parse(text: &str) -> Self {
        let mut hive = Self::default();
        let mut current: Option<String> = None;
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if let Some(rest) = line.strip_prefix('[') {
                let (path, _) = unquote(rest, ']');
                let path = path.to_lowercase();
                hive.keys.entry(path.clone()).or_default();
                current = Some(path);
            } else if line.starts_with("#arch=") && current.is_none() {
                hive.win64 = line.trim() == "#arch=win64";
            } else if line.starts_with('"') || line.starts_with('@') {
                let mut full = line.to_owned();
                // Binary data goes on with a backslash at the end.
                while full.ends_with('\\') {
                    full.pop();
                    match lines.next() {
                        Some(next) => full.push_str(next.trim_start()),
                        None => break,
                    }
                }
                let Some(key) = &current else { continue };
                if let Some((name, value)) = parse_value(&full) {
                    let values = hive.keys.get_mut(key).expect("made above");
                    values.insert(name.map(|n| n.to_lowercase()), value);
                }
            }
        }
        hive
    }

    /// A value at `path` (relative to the hive, `\`-separated); `name`
    /// `None` for the key's default value. Case does not matter.
    pub fn get(&self, path: &str, name: Option<&str>) -> Option<&Value> {
        self.keys
            .get(&path.trim_matches('\\').to_lowercase())?
            .get(&name.map(str::to_lowercase))
    }
}

/// A `name=data` line; `None` for one that is not.
fn parse_value(line: &str) -> Option<(Option<String>, Value)> {
    let (name, rest) = match line.strip_prefix('@') {
        Some(rest) => (None, rest),
        None => {
            let (name, rest) = unquote(line.strip_prefix('"')?, '"');
            (Some(name), rest)
        }
    };
    let data = rest.trim_start().strip_prefix('=')?;
    let value = if let Some(s) = data.strip_prefix('"') {
        Value::String(unquote(s, '"').0)
    } else if let Some(hex) = data.strip_prefix("dword:") {
        u32::from_str_radix(hex.trim(), 16).map_or(Value::Other, Value::Dword)
    } else {
        Value::Other
    };
    Some((name, value))
}

/// The text up to `end`, with Wine's escapes undone, and what follows
/// `end`.
fn unquote(s: &str, end: char) -> (String, &str) {
    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == end {
            return (out, &s[i + c.len_utf8()..]);
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some((_, e)) = chars.next() else { break };
        let simple = match e {
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'e' => Some('\x1b'),
            'f' => Some('\x0c'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\x0b'),
            _ => None,
        };
        if let Some(c) = simple {
            out.push(c);
        } else if e == 'x' {
            let mut n = 0u32;
            for _ in 0..4 {
                match chars.peek().and_then(|(_, d)| d.to_digit(16)) {
                    Some(d) => {
                        n = n * 16 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
        } else if let Some(first) = e.to_digit(8) {
            let mut n = first;
            for _ in 0..2 {
                match chars.peek().and_then(|(_, d)| d.to_digit(8)) {
                    Some(d) => {
                        n = n * 8 + d;
                        chars.next();
                    }
                    None => break,
                }
            }
            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
        } else {
            out.push(e);
        }
    }
    (out, "")
}

/// A prefix's registry: `system.reg` and `user.reg`.
#[derive(Debug, Default)]
pub struct Registry {
    pub machine: Hive,
    pub user: Hive,
}

impl Registry {
    /// Reads `<pfx>/system.reg` and `<pfx>/user.reg`; a missing file is
    /// an empty hive.
    pub fn read(pfx: &Path) -> std::io::Result<Self> {
        let hive = |name: &str| match std::fs::read(pfx.join(name)) {
            Ok(bytes) => Ok(Hive::parse(&String::from_utf8_lossy(&bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Hive::default()),
            Err(e) => Err(e),
        };
        Ok(Self {
            machine: hive("system.reg")?,
            user: hive("user.reg")?,
        })
    }

    /// A value in `HKEY_LOCAL_MACHINE` (`machine`) or
    /// `HKEY_CURRENT_USER`, as a 32-bit program sees it ([`view`]).
    pub fn get(&self, machine: bool, path: &str, name: Option<&str>) -> Option<&Value> {
        if machine {
            self.machine
                .get(&view(true, path, self.machine.win64), name)
        } else {
            self.user.get(path, name)
        }
    }
}

/// Where a 32-bit program's `path` is in a prefix: in a 64-bit one, a key
/// under `HKEY_LOCAL_MACHINE\Software` is at `Software\Wow6432Node`
/// (`install-scripts.md` §2.1).
pub fn view(machine: bool, path: &str, win64: bool) -> String {
    let path = path.trim_matches('\\');
    let (first, rest) = path.split_once('\\').unwrap_or((path, ""));
    let redirected = machine
        && win64
        && first.eq_ignore_ascii_case("software")
        && !rest
            .split('\\')
            .next()
            .is_some_and(|k| k.eq_ignore_ascii_case("wow6432node"));
    if !redirected {
        return path.to_owned();
    }
    if rest.is_empty() {
        format!("{first}\\Wow6432Node")
    } else {
        format!("{first}\\Wow6432Node\\{rest}")
    }
}

/// A change to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub machine: bool,
    /// The key's path, as [`view`] gives it.
    pub path: String,
    pub name: Option<String>,
    /// `None` deletes the value.
    pub value: Option<Value>,
}

/// A file for `regedit /S`: `Windows Registry Editor Version 5.00`, in
/// UTF-16LE with a byte order mark (`compat-tools.md` §7). Values of
/// another type than string and dword are left out.
pub fn import_file(changes: &[Change]) -> Vec<u8> {
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    let mut key: Option<(bool, &str)> = None;
    for c in changes {
        let data = match &c.value {
            None => "-".to_owned(),
            Some(Value::String(s)) => format!("\"{}\"", escape(s)),
            Some(Value::Dword(n)) => format!("dword:{n:08x}"),
            Some(Value::Other) => continue,
        };
        if key != Some((c.machine, c.path.as_str())) {
            let hive = if c.machine {
                "HKEY_LOCAL_MACHINE"
            } else {
                "HKEY_CURRENT_USER"
            };
            text.push_str(&format!("\r\n[{hive}\\{}]\r\n", c.path));
            key = Some((c.machine, c.path.as_str()));
        }
        let name = match &c.name {
            None => "@".to_owned(),
            Some(n) => format!("\"{}\"", escape(n)),
        };
        text.push_str(&format!("{name}={data}\r\n"));
    }
    text.push_str("\r\n");
    [0xfeff_u16]
        .into_iter()
        .chain(text.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The drive letter of the prefix whose `dosdevices` entry links to `/`,
/// as `Z:`; `None` when there is none.
pub fn root_drive(pfx: &Path) -> Option<String> {
    let dir = std::fs::read_dir(pfx.join("dosdevices")).ok()?;
    let mut found: Vec<String> = dir
        .flatten()
        .filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t == Path::new("/")))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let letter = name.strip_suffix(':')?;
            (letter.len() == 1 && letter.chars().all(|c| c.is_ascii_lowercase()))
                .then(|| format!("{}:", letter.to_ascii_uppercase()))
        })
        .collect();
    found.sort();
    found.pop()
}

/// A host path as a program in the prefix names it, on `drive`, the
/// drive that links to `/`.
pub fn dos_path(drive: &str, host: &Path) -> String {
    let mut out = drive.to_owned();
    for c in host.components() {
        if let std::path::Component::Normal(c) = c {
            out.push('\\');
            out.push_str(&c.to_string_lossy());
        }
    }
    if out == drive {
        out.push('\\');
    }
    out
}

/// The user directory of the prefix (`steamuser` in Proton's): the one
/// under `drive_c/users` that is not `Public`.
pub fn user(pfx: &Path) -> Option<String> {
    let mut users: Vec<String> = std::fs::read_dir(pfx.join("drive_c").join("users"))
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.eq_ignore_ascii_case("public"))
        .collect();
    users.sort();
    (users.len() == 1).then(|| users.remove(0))
}

/// `<compatdata>/pfx`.
pub fn prefix_dir(compat_data: &Path) -> PathBuf {
    compat_data.join("pfx")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as they are in Spacewar's prefix, with a few more forms.
    const SYSTEM: &str = r#"WINE REGISTRY Version 2
;; All keys relative to REGISTRY\\Machine

#arch=win64

[Software\\Wow6432Node\\Valve\\Steam] 1793301783
#time=1dd67daead30250
"InstallPath"="C:\\Program Files (x86)\\Steam"

[Software\\Wow6432Node\\Valve\\Steam\\Apps\\CommonRedist\\.NET\\4.7.2] 1793301782
#time=1dd67daeaac61ae
"4.7.2"=dword:00000001

[Software\\Microsoft\\Windows NT\\CurrentVersion] 1793301782
@="default"
"CurrentBuild"="19045"
"Path"=str(2):"%SystemRoot%\\x"
"Bin"=hex:01,02,\
  03,04
"Quote \"q\""="caf\xe9 \x4e2d1 \101"
"After"=dword:0000002a
"#;

    #[test]
    fn wine_files_are_read() {
        let h = Hive::parse(SYSTEM);
        assert!(h.win64);
        assert_eq!(
            h.get("software\\WOW6432NODE\\valve\\steam", Some("installpath")),
            Some(&Value::String("C:\\Program Files (x86)\\Steam".into()))
        );
        let key = "Software\\Microsoft\\Windows NT\\CurrentVersion";
        assert_eq!(h.get(key, None), Some(&Value::String("default".into())));
        assert_eq!(h.get(key, Some("Path")), Some(&Value::Other));
        assert_eq!(h.get(key, Some("Bin")), Some(&Value::Other));
        // \xe9, then \x4e2d followed by a digit, then octal 101.
        assert_eq!(
            h.get(key, Some("Quote \"q\"")),
            Some(&Value::String("café 中1 A".into()))
        );
        assert_eq!(h.get(key, Some("After")), Some(&Value::Dword(42)));
        assert_eq!(h.get(key, Some("Missing")), None);
    }

    #[test]
    fn the_32_bit_view() {
        let r = Registry {
            machine: Hive::parse(SYSTEM),
            user: Hive::default(),
        };
        let key = "Software\\Valve\\Steam\\Apps\\CommonRedist\\.NET\\4.7.2";
        assert_eq!(r.get(true, key, Some("4.7.2")), Some(&Value::Dword(1)));
        assert_eq!(r.get(false, key, Some("4.7.2")), None);
        assert_eq!(
            view(true, "SOFTWARE\\Valve\\Spacewar", true),
            "SOFTWARE\\Wow6432Node\\Valve\\Spacewar"
        );
        assert_eq!(view(true, "Software", true), "Software\\Wow6432Node");
        assert_eq!(
            view(true, "Software\\Wow6432Node\\X", true),
            "Software\\Wow6432Node\\X"
        );
        assert_eq!(view(true, "System\\X", true), "System\\X");
        assert_eq!(view(true, "Software\\X", false), "Software\\X");
        assert_eq!(view(false, "Software\\X", true), "Software\\X");
    }

    #[test]
    fn import_files() {
        let changes = [
            Change {
                machine: true,
                path: "SOFTWARE\\Wow6432Node\\Valve\\Spacewar".into(),
                name: Some("Exe_Path".into()),
                value: Some(Value::String("Z:\\a \"b\"\\ä.exe".into())),
            },
            Change {
                machine: true,
                path: "SOFTWARE\\Wow6432Node\\Valve\\Spacewar".into(),
                name: Some("PatchVersion".into()),
                value: Some(Value::Dword(42)),
            },
            Change {
                machine: false,
                path: "Software\\Example".into(),
                name: None,
                value: None,
            },
        ];
        let bytes = import_file(&changes);
        assert_eq!(&bytes[..2], &[0xff, 0xfe]);
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        assert_eq!(
            text,
            "Windows Registry Editor Version 5.00\r\n\
             \r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\Valve\\Spacewar]\r\n\
             \"Exe_Path\"=\"Z:\\\\a \\\"b\\\"\\\\ä.exe\"\r\n\
             \"PatchVersion\"=dword:0000002a\r\n\
             \r\n[HKEY_CURRENT_USER\\Software\\Example]\r\n\
             @=-\r\n\r\n"
        );
    }

    #[test]
    fn paths_in_the_prefix() {
        assert_eq!(
            dos_path("Z:", Path::new("/home/u/lib/apps/Space war")),
            "Z:\\home\\u\\lib\\apps\\Space war"
        );
        assert_eq!(dos_path("Z:", Path::new("/")), "Z:\\");
    }

    #[cfg(unix)]
    #[test]
    fn drives_and_users_of_a_prefix() {
        let dir = std::env::temp_dir().join(format!("ossteam-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dos = dir.join("dosdevices");
        std::fs::create_dir_all(&dos).unwrap();
        std::fs::create_dir_all(dir.join("drive_c/users/steamuser")).unwrap();
        std::fs::create_dir_all(dir.join("drive_c/users/Public")).unwrap();
        std::os::unix::fs::symlink("../drive_c", dos.join("c:")).unwrap();
        std::os::unix::fs::symlink("/", dos.join("z:")).unwrap();
        std::os::unix::fs::symlink("/dev/sda1", dos.join("d::")).unwrap();
        assert_eq!(root_drive(&dir).as_deref(), Some("Z:"));
        assert_eq!(user(&dir).as_deref(), Some("steamuser"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
