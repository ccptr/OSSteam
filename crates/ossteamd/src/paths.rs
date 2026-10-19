// SPDX-License-Identifier: GPL-2.0-or-later
//! The daemon's platform-standard directories (SPEC §17, DIRS-1).

use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;

/// The reverse-DNS application identifier used for macOS paths.
///
/// Provisional: SPEC §24 Q2 leaves the identifier open. Change it here
/// only, before the first release that writes macOS paths.
#[cfg(target_os = "macos")]
const APP_ID: (&str, &str, &str) = ("dev", "ccptr", "ossteam");

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    /// Logs.
    pub state: PathBuf,
}

impl Paths {
    pub fn resolve() -> io::Result<Self> {
        let not_found = || io::Error::new(io::ErrorKind::NotFound, "no home directory");

        #[cfg(target_os = "macos")]
        {
            let (q, o, a) = APP_ID;
            let dirs = ProjectDirs::from(q, o, a).ok_or_else(not_found)?;
            let home = directories::BaseDirs::new()
                .ok_or_else(not_found)?
                .home_dir()
                .to_owned();
            let id = format!("{q}.{o}.{a}");
            Ok(Self {
                config: dirs.config_dir().to_owned(),
                data: dirs.data_dir().to_owned(),
                cache: dirs.cache_dir().to_owned(),
                state: home.join("Library/Logs").join(id),
            })
        }

        #[cfg(windows)]
        {
            if let Some(root) = ossteam_ipc::endpoint::test_root() {
                return Ok(Self {
                    config: root.join("config"),
                    data: root.join("data"),
                    cache: root.join("cache"),
                    state: root.join("state"),
                });
            }
            let dirs = ProjectDirs::from("", "", "ossteam").ok_or_else(not_found)?;
            let local = dirs.data_local_dir().to_owned();
            let state = local
                .parent()
                .map(|p| p.join("state"))
                .ok_or_else(not_found)?;
            Ok(Self {
                config: dirs.config_dir().to_owned(),
                data: local,
                cache: dirs.cache_dir().to_owned(),
                state,
            })
        }

        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let dirs = ProjectDirs::from("", "", "ossteam").ok_or_else(not_found)?;
            let state = match dirs.state_dir() {
                Some(s) => s.to_owned(),
                None => directories::BaseDirs::new()
                    .ok_or_else(not_found)?
                    .home_dir()
                    .join(".local/state/ossteam"),
            };
            Ok(Self {
                config: dirs.config_dir().to_owned(),
                data: dirs.data_dir().to_owned(),
                cache: dirs.cache_dir().to_owned(),
                state,
            })
        }
    }
}
