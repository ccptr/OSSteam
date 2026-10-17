// SPDX-License-Identifier: GPL-2.0-or-later
//! The PICS cache (`pics.md` §6, D-7): one binary KeyValues file per app
//! and package, keyed by change number. It can be deleted at any time.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use ossteam_vdf::{Object, Value, binary};

use crate::info::Info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Package,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Package => "package",
        }
    }
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// A cache rooted at `dir` (the daemon passes `<cache>/pics`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, kind: Kind, id: u32) -> PathBuf {
        self.dir.join(kind.dir()).join(format!("{id}.vdf"))
    }

    /// The cached entry, if there is a readable one. An unreadable file is
    /// deleted so that the entry is fetched again.
    pub fn load(&self, kind: Kind, id: u32) -> Option<Info> {
        let path = self.path(kind, id);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(path = %path.display(), "cannot read a PICS cache entry: {e}");
                return None;
            }
        };
        match decode(id, &bytes) {
            Some(info) => Some(info),
            None => {
                tracing::warn!(path = %path.display(), "dropping a damaged PICS cache entry");
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    pub fn store(&self, kind: Kind, info: &Info) -> io::Result<()> {
        write_atomic(&self.path(kind, info.id), &encode(info))
    }

    /// The last change number seen (`pics.md` §4), or 0.
    pub fn change_number(&self) -> u32 {
        std::fs::read(self.dir.join("state.vdf"))
            .ok()
            .and_then(|b| binary::parse(&b, binary::Keys::Inline).ok())
            .and_then(|(o, _)| o.get("change_number").and_then(Value::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    }

    pub fn set_change_number(&self, n: u32) -> io::Result<()> {
        let mut o = Object::new();
        o.push("change_number", Value::UInt64(n.into()));
        write_atomic(&self.dir.join("state.vdf"), &binary::to_vec(&o))
    }
}

fn encode(info: &Info) -> Vec<u8> {
    let mut sha = String::with_capacity(info.sha.len() * 2);
    for b in &info.sha {
        let _ = write!(sha, "{b:02x}");
    }
    let mut o = Object::new();
    o.push("change_number", Value::UInt64(info.change_number.into()));
    o.push("sha", sha);
    o.push("missing_token", Value::Int32(info.missing_token.into()));
    o.push("data", info.data.clone());
    binary::to_vec(&o)
}

fn decode(id: u32, bytes: &[u8]) -> Option<Info> {
    let o = binary::parse_all(bytes, binary::Keys::Inline).ok()?;
    let sha = o.get_str("sha")?;
    if sha.len() % 2 != 0 {
        return None;
    }
    let sha = (0..sha.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(sha.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(Info {
        id,
        change_number: u32::try_from(o.get("change_number")?.as_u64()?).ok()?,
        sha,
        missing_token: o.get("missing_token")?.as_bool()?,
        data: o.get_object("data")?.clone(),
    })
}

/// Writes to a temporary name in the same directory, then renames into
/// place, so a reader never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().expect("cache paths have a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ossteam-pics-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn round_trips_entries_and_the_change_number() {
        let d = dir("roundtrip");
        let cache = Cache::new(&d);
        assert!(cache.load(Kind::App, 480).is_none());
        assert_eq!(cache.change_number(), 0);

        let mut data = Object::new();
        data.push("common", Object::from_iter([("name", "Spacewar")]));
        let info = Info {
            id: 480,
            change_number: 77,
            sha: vec![0x00, 0xab, 0xff],
            missing_token: true,
            data,
        };
        cache.store(Kind::App, &info).unwrap();
        assert_eq!(cache.load(Kind::App, 480), Some(info));
        assert!(cache.load(Kind::Package, 480).is_none());

        cache.set_change_number(123).unwrap();
        assert_eq!(cache.change_number(), 123);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn damaged_entries_are_dropped() {
        let d = dir("damaged");
        let cache = Cache::new(&d);
        let path = cache.path(Kind::Package, 5);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\x00garbage").unwrap();
        assert!(cache.load(Kind::Package, 5).is_none());
        assert!(!path.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
