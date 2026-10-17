// SPDX-License-Identifier: GPL-2.0-or-later
//! Refresh tokens and guard data (`auth.md` §4 and §5, SPEC D-8).
//!
//! Secrets go in the OS keyring: Secret Service on Linux, the Keychain on
//! macOS, Credential Manager on Windows. Only when no keyring is reachable
//! and the user set `secrets.file_fallback` do they go in a mode-0600 file
//! in the data directory instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use keyring_core::{CredentialStore, Error as KeyringError};
use ossteam_types::Secret;
use zeroize::{Zeroize, Zeroizing};

/// The keyring service for refresh tokens, keyed by account name.
pub const TOKENS: &str = "ossteam";
/// The keyring service for guard data (machine tokens).
pub const GUARD: &str = "ossteam-guard";

pub const FALLBACK_FILE: &str = "secrets.json";

#[derive(Debug, Clone, thiserror::Error)]
pub enum SecretsError {
    /// Nowhere to store secrets: no keyring, and the file fallback is off.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
}

enum Backend {
    Keyring(Arc<CredentialStore>),
    File(PathBuf),
}

pub struct Secrets {
    backend: Result<Backend, String>,
}

impl Secrets {
    /// Opens the OS keyring, or the fallback file if the keyring is out of
    /// reach and `file_fallback` allows it.
    pub fn open(file_fallback: bool, data_dir: &Path) -> Self {
        let backend = match platform_store() {
            Ok(store) => {
                tracing::debug!(store = ?store, "using the OS keyring");
                Ok(Backend::Keyring(store))
            }
            Err(e) if file_fallback => {
                let path = data_dir.join(FALLBACK_FILE);
                tracing::warn!(
                    "no OS keyring ({e}); storing tokens in {} as secrets.file_fallback allows",
                    path.display()
                );
                Ok(Backend::File(path))
            }
            Err(e) => {
                tracing::warn!("no OS keyring: {e}");
                Err(format!(
                    "no OS keyring is available ({e}). Log in with --no-remember, or run \
                     'ossteam config set secrets.file_fallback true' to store tokens in a \
                     private file instead"
                ))
            }
        };
        Self { backend }
    }

    #[cfg(test)]
    pub fn file(path: PathBuf) -> Self {
        Self {
            backend: Ok(Backend::File(path)),
        }
    }

    fn backend(&self) -> Result<&Backend, SecretsError> {
        self.backend
            .as_ref()
            .map_err(|e| SecretsError::Unavailable(e.clone()))
    }

    /// Whether secrets can be stored at all.
    pub fn check(&self) -> Result<(), SecretsError> {
        self.backend().map(|_| ())
    }

    pub fn get(&self, service: &str, user: &str) -> Result<Option<Secret>, SecretsError> {
        match self.backend()? {
            Backend::Keyring(store) => {
                match store
                    .build(service, user, None)
                    .and_then(|e| e.get_password())
                {
                    Ok(p) => Ok(Some(Secret::new(p))),
                    Err(KeyringError::NoEntry) => Ok(None),
                    Err(e) => Err(keyring_failure("reading", e)),
                }
            }
            Backend::File(path) => {
                let map = FileMap::read(path)?;
                Ok(map
                    .0
                    .get(service)
                    .and_then(|m| m.get(user))
                    .map(|s| Secret::new(s.clone())))
            }
        }
    }

    pub fn set(&self, service: &str, user: &str, secret: &Secret) -> Result<(), SecretsError> {
        match self.backend()? {
            Backend::Keyring(store) => store
                .build(service, user, None)
                .and_then(|e| e.set_password(secret.expose()))
                .map_err(|e| keyring_failure("storing", e)),
            Backend::File(path) => {
                let mut map = FileMap::read(path)?;
                map.0
                    .entry(service.to_owned())
                    .or_default()
                    .insert(user.to_owned(), secret.expose().to_owned());
                map.write(path)
            }
        }
    }

    /// Deletes a secret. Deleting one that does not exist succeeds.
    pub fn delete(&self, service: &str, user: &str) -> Result<(), SecretsError> {
        match self.backend()? {
            Backend::Keyring(store) => {
                match store
                    .build(service, user, None)
                    .and_then(|e| e.delete_credential())
                {
                    Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                    Err(e) => Err(keyring_failure("deleting", e)),
                }
            }
            Backend::File(path) => {
                let mut map = FileMap::read(path)?;
                let removed = map.0.get_mut(service).and_then(|m| m.remove(user));
                match removed {
                    Some(mut old) => {
                        old.zeroize();
                        map.write(path)
                    }
                    None => Ok(()),
                }
            }
        }
    }
}

fn keyring_failure(action: &str, e: KeyringError) -> SecretsError {
    // The error's Display never includes secret bytes; the bytes some
    // variants carry are wiped here.
    let message = format!("{action} a secret in the OS keyring failed: {e}");
    match e {
        KeyringError::BadEncoding(mut b) | KeyringError::BadDataFormat(mut b, _) => b.zeroize(),
        _ => {}
    }
    SecretsError::Failed(message)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

/// The fallback file's content: service → user → secret. Values are wiped
/// when the map is dropped.
#[derive(Default)]
struct FileMap(BTreeMap<String, BTreeMap<String, String>>);

impl Drop for FileMap {
    fn drop(&mut self) {
        for users in self.0.values_mut() {
            for v in users.values_mut() {
                v.zeroize();
            }
        }
    }
}

impl FileMap {
    fn read(path: &Path) -> Result<Self, SecretsError> {
        let bytes = match std::fs::read(path) {
            Ok(b) => Zeroizing::new(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(SecretsError::Failed(format!(
                    "reading {}: {e}",
                    path.display()
                )));
            }
        };
        serde_json::from_slice(&bytes).map(Self).map_err(|e| {
            SecretsError::Failed(format!(
                "{} is malformed (line {}, column {})",
                path.display(),
                e.line(),
                e.column()
            ))
        })
    }

    /// Writes the file through a temporary file created with mode 0600, so
    /// the secrets are never readable by others, even briefly.
    fn write(&self, path: &Path) -> Result<(), SecretsError> {
        use std::io::Write;
        let fail =
            |e: std::io::Error| SecretsError::Failed(format!("writing {}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = Zeroizing::new(serde_json::to_vec(&self.0).expect("maps of strings serialize"));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp).map_err(fail)?;
        f.write_all(&json)
            .and_then(|()| f.sync_all())
            .map_err(fail)?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(fail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_backend_round_trip() {
        let dir = std::env::temp_dir().join(format!("ossteamd-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FALLBACK_FILE);
        let s = Secrets::file(path.clone());

        assert!(s.get(TOKENS, "alice").unwrap().is_none());
        s.set(TOKENS, "alice", &Secret::new("t1".into())).unwrap();
        s.set(GUARD, "alice", &Secret::new("g1".into())).unwrap();
        assert_eq!(s.get(TOKENS, "alice").unwrap().unwrap().expose(), "t1");
        assert_eq!(s.get(GUARD, "alice").unwrap().unwrap().expose(), "g1");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        s.delete(TOKENS, "alice").unwrap();
        s.delete(TOKENS, "nobody").unwrap();
        assert!(s.get(TOKENS, "alice").unwrap().is_none());
        assert!(s.get(GUARD, "alice").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unavailable_store_says_how_to_proceed() {
        let s = Secrets {
            backend: Err("no keyring".into()),
        };
        assert!(matches!(s.check(), Err(SecretsError::Unavailable(_))));
        assert!(matches!(
            s.get(TOKENS, "a"),
            Err(SecretsError::Unavailable(_))
        ));
    }
}
