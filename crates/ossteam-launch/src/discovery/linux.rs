// SPDX-License-Identifier: GPL-2.0-or-later
//! `~/.steam/sdk64`, `~/.steam/sdk32` and `~/.steam/steam.pid`
//! (`steamworks-abi.md` §1.1).

use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::files::{make_dir, remove, replace_copy};

/// Writes `<dir>/steam.pid` by renaming a new file over it.
fn write_pid(dir: &Path, pid: u32) -> Result<(), Error> {
    let file = dir.join("steam.pid");
    let tmp = dir.join(".steam.pid.ossteam");
    std::fs::write(&tmp, pid.to_string())
        .and_then(|()| std::fs::rename(&tmp, &file))
        .map_err(|e| Error::io(format!("writing {}", file.display()), e))
}

fn remove_dir(dir: &Path) -> Result<(), Error> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {}", dir.display()), e)),
    }
}
use super::{Error, Libraries};

pub mod takeover;

const SDKS: [&str; 2] = ["sdk64", "sdk32"];

pub struct Entries {
    /// `~/.steam`.
    steam: PathBuf,
    /// `<data>/steam-sdk`.
    sdk: PathBuf,
}

/// What is at one of the entries' paths.
enum Found {
    Missing,
    Ours,
    /// Something else, described for the conflict message.
    Foreign(String),
}

impl Entries {
    /// `home` is the user's home directory, `data` the daemon's data
    /// directory.
    pub fn new(home: &Path, data: &Path) -> Self {
        Self {
            steam: home.join(".steam"),
            sdk: data.join("steam-sdk"),
        }
    }

    fn link(&self, sdk: &str) -> PathBuf {
        self.steam.join(sdk)
    }

    fn pid_file(&self) -> PathBuf {
        self.steam.join("steam.pid")
    }

    fn find(&self, sdk: &str) -> Result<Found, Error> {
        let link = self.link(sdk);
        match std::fs::read_link(&link) {
            Ok(target) if target == self.sdk.join(sdk) => Ok(Found::Ours),
            Ok(target) => Ok(Found::Foreign(format!(
                "{} (a link to {})",
                link.display(),
                target.display()
            ))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Found::Missing),
            // Not a link: a directory or a file.
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                Ok(Found::Foreign(link.display().to_string()))
            }
            Err(e) => Err(Error::io(format!("reading {}", link.display()), e)),
        }
    }

    fn ours(&self) -> Result<bool, Error> {
        Ok(matches!(self.find("sdk64")?, Found::Ours))
    }

    /// Fails with `Conflict` when an entry exists that is not ossteam's
    /// (DISC-3).
    pub fn check(&self) -> Result<(), Error> {
        for sdk in SDKS {
            if let Found::Foreign(what) = self.find(sdk)? {
                return Err(Error::Conflict(what));
            }
        }
        let pid = self.pid_file();
        if !self.ours()? && pid.symlink_metadata().is_ok() {
            return Err(Error::Conflict(pid.display().to_string()));
        }
        Ok(())
    }

    /// Creates or refreshes the entries for the daemon `pid`: first those
    /// of the private `~/.steam` in `<data>/steam-sdk` (§1.1.2), whoever
    /// owns the real ones (§1.5), then the real ones. Fails with
    /// `Conflict`, having made only the private ones, when a real entry is
    /// not ossteam's.
    pub fn create(&self, libs: &Libraries, pid: u32, _account_id: u32) -> Result<(), Error> {
        for (sdk, lib) in [("sdk64", Some(&libs.lib64)), ("sdk32", libs.lib32.as_ref())] {
            let dir = self.sdk.join(sdk);
            match lib {
                Some(lib) => {
                    make_dir(&dir, 0o700)?;
                    replace_copy(lib, &dir.join("steamclient.so"))?;
                }
                // No library of this width: no entry either.
                None => remove_dir(&dir)?,
            }
        }
        // Last, so that it never names a client whose library is missing.
        write_pid(&self.sdk, pid)?;

        self.check()?;
        make_dir(&self.steam, 0o755)?;
        for sdk in SDKS {
            let link = self.link(sdk);
            match (self.sdk.join(sdk).is_dir(), self.find(sdk)?) {
                (true, Found::Missing) => symlink(self.sdk.join(sdk), &link)
                    .map_err(|e| Error::io(format!("creating {}", link.display()), e))?,
                (false, Found::Ours) => remove(&link)?,
                _ => {}
            }
        }
        write_pid(&self.steam, pid)
    }

    /// Removes ossteam's entries, the private `steam.pid` included:
    /// `steam.pid` first, then the links.
    pub fn remove(&self) -> Result<(), Error> {
        remove(&self.sdk.join("steam.pid"))?;
        if self.ours()? {
            remove(&self.pid_file())?;
        }
        for sdk in SDKS {
            if matches!(self.find(sdk)?, Found::Ours) {
                remove(&self.link(sdk))?;
            }
        }
        Ok(())
    }

    /// Removes ossteam's entries when their pid is not `own_pid`, as a
    /// crash leaves them (DISC-2).
    pub fn remove_stale(&self, own_pid: u32) -> Result<(), Error> {
        let private = self.sdk.join("steam.pid");
        let own = own_pid.to_string();
        if std::fs::read_to_string(&private).is_ok_and(|p| p.trim() != own) {
            remove(&private)?;
        }
        if !self.ours()? && !SDKS.iter().any(|s| matches!(self.find(s), Ok(Found::Ours))) {
            return Ok(());
        }
        let recorded = std::fs::read_to_string(self.pid_file()).ok();
        if recorded.as_deref().map(str::trim) == Some(own.as_str()) {
            return Ok(());
        }
        tracing::info!("removing discovery entries left by an earlier daemon");
        self.remove()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    struct Dirs {
        root: PathBuf,
        home: PathBuf,
        data: PathBuf,
        libs: Libraries,
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dirs(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ossteam-disc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let data = root.join("data");
        let bin = root.join("bin");
        std::fs::create_dir_all(bin.join("lib32")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(bin.join("libossteam_steamclient.so"), b"64").unwrap();
        std::fs::write(bin.join("lib32/libossteam_steamclient.so"), b"32").unwrap();
        let libs = super::super::find_libraries(&bin).unwrap();
        Dirs {
            root,
            home,
            data,
            libs,
        }
    }

    #[test]
    fn create_and_remove() {
        let d = dirs("create");
        let e = Entries::new(&d.home, &d.data);
        e.create(&d.libs, 1234, 0).unwrap();
        let steam = d.home.join(".steam");
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "1234"
        );
        assert_eq!(
            std::fs::read(steam.join("sdk64/steamclient.so")).unwrap(),
            b"64"
        );
        assert_eq!(
            std::fs::read(steam.join("sdk32/steamclient.so")).unwrap(),
            b"32"
        );
        // Copies, not links, and replaced by a new file when the library
        // changes.
        let copy = d.data.join("steam-sdk/sdk64/steamclient.so");
        assert!(copy.symlink_metadata().unwrap().file_type().is_file());
        let before = copy.metadata().unwrap().ino();
        e.create(&d.libs, 1234, 0).unwrap();
        assert_eq!(copy.metadata().unwrap().ino(), before, "unchanged");
        std::fs::write(&d.libs.lib64, b"64 v2").unwrap();
        e.create(&d.libs, 1234, 0).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"64 v2");
        assert_ne!(copy.metadata().unwrap().ino(), before);
        // Again, with only the 64-bit library: the 32-bit entry goes.
        let only64 = Libraries {
            lib64: d.libs.lib64.clone(),
            lib32: None,
        };
        e.create(&only64, 1234, 0).unwrap();
        assert!(steam.join("sdk32").symlink_metadata().is_err());
        assert!(!d.data.join("steam-sdk/sdk32").exists());
        let private = d.data.join("steam-sdk/steam.pid");
        assert_eq!(std::fs::read_to_string(&private).unwrap(), "1234");
        e.remove().unwrap();
        for n in ["sdk64", "sdk32", "steam.pid"] {
            assert!(steam.join(n).symlink_metadata().is_err(), "{n}");
        }
        assert!(!private.exists());
    }

    #[test]
    fn an_official_client_is_left_alone() {
        let d = dirs("official");
        let steam = d.home.join(".steam");
        let valve = d.root.join("Steam/linux64");
        std::fs::create_dir_all(&valve).unwrap();
        std::fs::create_dir_all(&steam).unwrap();
        symlink(&valve, steam.join("sdk64")).unwrap();
        std::fs::write(steam.join("steam.pid"), "99").unwrap();
        let e = Entries::new(&d.home, &d.data);
        assert!(matches!(e.check(), Err(Error::Conflict(m)) if m.contains("sdk64")));
        assert!(matches!(e.create(&d.libs, 1, 0), Err(Error::Conflict(_))));
        // The private ~/.steam is made all the same.
        let sdk = d.data.join("steam-sdk");
        assert_eq!(std::fs::read_to_string(sdk.join("steam.pid")).unwrap(), "1");
        assert_eq!(
            std::fs::read(sdk.join("sdk64/steamclient.so")).unwrap(),
            b"64"
        );
        e.remove().unwrap();
        assert!(!sdk.join("steam.pid").exists());
        e.remove_stale(1).unwrap();
        assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), valve);
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "99"
        );
        assert!(steam.join("sdk32").symlink_metadata().is_err());

        // A pid file alone is the official client's too.
        std::fs::remove_file(steam.join("sdk64")).unwrap();
        assert!(matches!(e.check(), Err(Error::Conflict(m)) if m.contains("steam.pid")));
    }

    #[test]
    fn stale_entries_are_removed() {
        let d = dirs("stale");
        let e = Entries::new(&d.home, &d.data);
        e.create(&d.libs, 1234, 0).unwrap();
        e.remove_stale(1234).unwrap();
        assert!(d.home.join(".steam/steam.pid").exists());
        e.remove_stale(5678).unwrap();
        assert!(!d.home.join(".steam/steam.pid").exists());
        assert!(!d.data.join("steam-sdk/steam.pid").exists());
        assert!(d.home.join(".steam/sdk64").symlink_metadata().is_err());
    }
}
