// SPDX-License-Identifier: GPL-2.0-or-later
//! `HKEY_CURRENT_USER\Software\Valve\Steam\ActiveProcess`
//! (`steamworks-abi.md` §1.2). Run under Wine only; not yet on Windows.

use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW,
    RegOpenKeyExW, RegSetValueExW,
};

use super::{Error, LIBRARY, Libraries};

const KEY: &str = r"Software\Valve\Steam\ActiveProcess";
const VALUES: [&str; 4] = ["pid", "SteamClientDll", "SteamClientDll64", "ActiveUser"];

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn os_error(what: &str, code: u32) -> Error {
    Error::io(what, std::io::Error::from_raw_os_error(code as i32))
}

pub struct Entries {
    /// The key under `HKEY_CURRENT_USER`: [`KEY`], or one of a test's own.
    key: String,
}

impl Entries {
    pub fn new(_home: &Path, _data: &Path) -> Self {
        Self { key: KEY.into() }
    }

    /// A string value of the key, or `None` when it is not there.
    fn get(&self, name: &str) -> Result<Option<String>, Error> {
        let key = wide(&self.key);
        let name_w = wide(name);
        let mut len = 0u32;
        // SAFETY: the strings are NUL-terminated; a null buffer asks for
        // the size.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name_w.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if r != ERROR_SUCCESS {
            return Err(os_error(&format!("reading {}\\{name}", self.key), r));
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        // SAFETY: the buffer holds `len` bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name_w.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if r != ERROR_SUCCESS {
            return Err(os_error(&format!("reading {}\\{name}", self.key), r));
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(Some(
            std::ffi::OsString::from_wide(&buf[..end])
                .to_string_lossy()
                .into_owned(),
        ))
    }

    /// Whether a library path names ossteam's library.
    fn is_ours(path: &str) -> bool {
        PathBuf::from(path)
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n.eq_ignore_ascii_case(LIBRARY))
    }

    fn ours(&self) -> Result<bool, Error> {
        Ok(self
            .get("SteamClientDll64")?
            .is_some_and(|p| Self::is_ours(&p)))
    }

    pub fn check(&self) -> Result<(), Error> {
        for name in ["SteamClientDll", "SteamClientDll64"] {
            if let Some(p) = self.get(name)?
                && !Self::is_ours(&p)
            {
                return Err(Error::Conflict(format!(
                    r"HKEY_CURRENT_USER\{}\{name} (naming {p})",
                    self.key
                )));
            }
        }
        Ok(())
    }

    pub fn create(&self, libs: &Libraries, pid: u32, account_id: u32) -> Result<(), Error> {
        self.check()?;
        let key = open(&self.key, true)?;
        let set = || -> Result<(), Error> {
            match &libs.lib32 {
                Some(l) => set_string(key, &self.key, "SteamClientDll", l)?,
                None => delete(key, &self.key, "SteamClientDll")?,
            }
            set_string(key, &self.key, "SteamClientDll64", &libs.lib64)?;
            set_dword(key, &self.key, "ActiveUser", account_id)?;
            // Last, as on Linux.
            set_dword(key, &self.key, "pid", pid)
        };
        let r = set();
        // SAFETY: opened above, closed once.
        unsafe { RegCloseKey(key) };
        r
    }

    pub fn remove(&self) -> Result<(), Error> {
        if !self.ours()? {
            return Ok(());
        }
        let key = open(&self.key, false)?;
        let r = VALUES.iter().try_for_each(|n| delete(key, &self.key, n));
        // SAFETY: as above.
        unsafe { RegCloseKey(key) };
        r
    }

    pub fn remove_stale(&self, own_pid: u32) -> Result<(), Error> {
        if !self.ours()? {
            return Ok(());
        }
        let key = open(&self.key, false)?;
        let mut pid = 0u32;
        let mut len = 4u32;
        let name = wide("pid");
        // SAFETY: a 4-byte buffer for a DWORD.
        let r = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut pid).cast(),
                &mut len,
            )
        };
        // SAFETY: as above.
        unsafe { RegCloseKey(key) };
        if r == ERROR_SUCCESS && pid == own_pid {
            return Ok(());
        }
        tracing::info!("removing discovery entries left by an earlier daemon");
        self.remove()
    }
}

fn open(subkey: &str, create: bool) -> Result<HKEY, Error> {
    let path = wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the string is NUL-terminated; `key` is an out parameter.
    let r = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                &mut key,
            )
        }
    };
    if r != ERROR_SUCCESS {
        return Err(os_error(&format!("opening {subkey}"), r));
    }
    Ok(key)
}

fn set_string(key: HKEY, subkey: &str, name: &str, value: &Path) -> Result<(), Error> {
    let name_w = wide(name);
    let data = wide(value);
    // SAFETY: `data` is NUL-terminated UTF-16 of the length given in bytes.
    let r = unsafe {
        RegSetValueExW(
            key,
            name_w.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(os_error(&format!("writing {subkey}\\{name}"), r));
    }
    Ok(())
}

fn set_dword(key: HKEY, subkey: &str, name: &str, value: u32) -> Result<(), Error> {
    let name_w = wide(name);
    let bytes = value.to_le_bytes();
    // SAFETY: four bytes for a DWORD.
    let r = unsafe { RegSetValueExW(key, name_w.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4) };
    if r != ERROR_SUCCESS {
        return Err(os_error(&format!("writing {subkey}\\{name}"), r));
    }
    Ok(())
}

fn delete(key: HKEY, subkey: &str, name: &str) -> Result<(), Error> {
    let name_w = wide(name);
    // SAFETY: a NUL-terminated name.
    let r = unsafe { RegDeleteValueW(key, name_w.as_ptr()) };
    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
        return Err(os_error(&format!("removing {subkey}\\{name}"), r));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW};

    use super::*;

    /// A key of the test's own under `Software\ossteam-test`, deleted when
    /// the test ends, so that a real client's entries are never touched.
    struct Dirs {
        root: PathBuf,
        /// `Software\ossteam-test\<id>`, holding [`Self::key`].
        own: String,
        key: String,
        libs: Libraries,
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let own = wide(&self.own);
            let parent = wide(PARENT);
            // SAFETY: NUL-terminated key names. The parent goes only when
            // no other test still has a key in it.
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, own.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, parent.as_ptr());
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const PARENT: &str = r"Software\ossteam-test";

    fn dirs(name: &str) -> Dirs {
        let id = format!("{}-{name}", std::process::id());
        let root = std::env::temp_dir().join(format!("ossteam-disc-{id}"));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(bin.join("lib32")).unwrap();
        std::fs::write(bin.join(LIBRARY), b"64").unwrap();
        std::fs::write(bin.join("lib32").join(LIBRARY), b"32").unwrap();
        let libs = super::super::find_libraries(&bin).unwrap();
        let own = format!(r"{PARENT}\{id}");
        Dirs {
            root,
            key: format!(r"{own}\ActiveProcess"),
            own,
            libs,
        }
    }

    impl Dirs {
        fn entries(&self) -> Entries {
            Entries {
                key: self.key.clone(),
            }
        }

        fn dword(&self, name: &str) -> Option<u32> {
            let key = wide(&self.key);
            let name = wide(name);
            let mut value = 0u32;
            let mut len = 4u32;
            // SAFETY: a 4-byte buffer for a DWORD.
            let r = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD,
                    std::ptr::null_mut(),
                    (&raw mut value).cast(),
                    &mut len,
                )
            };
            (r == ERROR_SUCCESS).then_some(value)
        }
    }

    #[test]
    fn create_and_remove() {
        let d = dirs("create");
        let e = d.entries();
        e.create(&d.libs, 1234, 77).unwrap();
        assert_eq!(d.dword("pid"), Some(1234));
        assert_eq!(d.dword("ActiveUser"), Some(77));
        let path = |p: &Path| Some(p.to_string_lossy().into_owned());
        assert_eq!(e.get("SteamClientDll64").unwrap(), path(&d.libs.lib64));
        assert_eq!(
            e.get("SteamClientDll").unwrap(),
            path(d.libs.lib32.as_deref().unwrap())
        );
        // Again, with only the 64-bit library: the 32-bit value goes.
        let only64 = Libraries {
            lib64: d.libs.lib64.clone(),
            lib32: None,
        };
        e.create(&only64, 1234, 0).unwrap();
        assert_eq!(e.get("SteamClientDll").unwrap(), None);
        assert_eq!(d.dword("ActiveUser"), Some(0));
        e.remove().unwrap();
        for n in VALUES {
            assert_eq!(e.get(n).unwrap(), None, "{n}");
        }
        assert_eq!(d.dword("pid"), None);
        // Removing what is not there is fine.
        e.remove().unwrap();
    }

    #[test]
    fn an_official_client_is_left_alone() {
        let d = dirs("official");
        let valve = d.root.join(r"Steam\steamclient64.dll");
        let key = open(&d.key, true).unwrap();
        set_string(key, &d.key, "SteamClientDll64", &valve).unwrap();
        set_dword(key, &d.key, "pid", 99).unwrap();
        // SAFETY: opened above, closed once.
        unsafe { RegCloseKey(key) };
        let e = d.entries();
        assert!(matches!(e.check(), Err(Error::Conflict(m)) if m.contains("SteamClientDll64")));
        assert!(matches!(e.create(&d.libs, 1, 0), Err(Error::Conflict(_))));
        e.remove().unwrap();
        e.remove_stale(1).unwrap();
        assert_eq!(
            e.get("SteamClientDll64").unwrap(),
            Some(valve.to_string_lossy().into_owned())
        );
        assert_eq!(d.dword("pid"), Some(99));
        assert_eq!(e.get("SteamClientDll").unwrap(), None);
    }

    #[test]
    fn stale_entries_are_removed() {
        let d = dirs("stale");
        let e = d.entries();
        e.create(&d.libs, 1234, 0).unwrap();
        e.remove_stale(1234).unwrap();
        assert_eq!(d.dword("pid"), Some(1234));
        e.remove_stale(5678).unwrap();
        assert_eq!(d.dword("pid"), None);
        assert_eq!(e.get("SteamClientDll64").unwrap(), None);
    }

    #[test]
    fn no_key_is_no_entries() {
        let d = dirs("none");
        let e = d.entries();
        e.check().unwrap();
        e.remove().unwrap();
        e.remove_stale(1).unwrap();
        assert_eq!(e.get("pid").unwrap(), None);
    }
}
