// SPDX-License-Identifier: GPL-2.0-or-later
//! `HKEY_CURRENT_USER\Software\Valve\Steam\ActiveProcess`
//! (`steamworks-abi.md` §1.2). Type-checked only; not yet run on Windows.

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

pub struct Entries;

impl Entries {
    pub fn new(_home: &Path, _data: &Path) -> Self {
        Self
    }

    /// A string value of the key, or `None` when it is not there.
    fn get(&self, name: &str) -> Result<Option<String>, Error> {
        let key = wide(KEY);
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
            return Err(os_error(&format!("reading {KEY}\\{name}"), r));
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
            return Err(os_error(&format!("reading {KEY}\\{name}"), r));
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
                    r"HKEY_CURRENT_USER\{KEY}\{name} (naming {p})"
                )));
            }
        }
        Ok(())
    }

    pub fn create(&self, libs: &Libraries, pid: u32, account_id: u32) -> Result<(), Error> {
        self.check()?;
        let key = open(true)?;
        let set = || -> Result<(), Error> {
            match &libs.lib32 {
                Some(l) => set_string(key, "SteamClientDll", l)?,
                None => delete(key, "SteamClientDll")?,
            }
            set_string(key, "SteamClientDll64", &libs.lib64)?;
            set_dword(key, "ActiveUser", account_id)?;
            // Last, as on Linux.
            set_dword(key, "pid", pid)
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
        let key = open(false)?;
        let r = VALUES.iter().try_for_each(|n| delete(key, n));
        // SAFETY: as above.
        unsafe { RegCloseKey(key) };
        r
    }

    pub fn remove_stale(&self, own_pid: u32) -> Result<(), Error> {
        if !self.ours()? {
            return Ok(());
        }
        let key = open(false)?;
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

fn open(create: bool) -> Result<HKEY, Error> {
    let path = wide(KEY);
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
        return Err(os_error(&format!("opening {KEY}"), r));
    }
    Ok(key)
}

fn set_string(key: HKEY, name: &str, value: &Path) -> Result<(), Error> {
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
        return Err(os_error(&format!("writing {KEY}\\{name}"), r));
    }
    Ok(())
}

fn set_dword(key: HKEY, name: &str, value: u32) -> Result<(), Error> {
    let name_w = wide(name);
    let bytes = value.to_le_bytes();
    // SAFETY: four bytes for a DWORD.
    let r = unsafe { RegSetValueExW(key, name_w.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4) };
    if r != ERROR_SUCCESS {
        return Err(os_error(&format!("writing {KEY}\\{name}"), r));
    }
    Ok(())
}

fn delete(key: HKEY, name: &str) -> Result<(), Error> {
    let name_w = wide(name);
    // SAFETY: a NUL-terminated name.
    let r = unsafe { RegDeleteValueW(key, name_w.as_ptr()) };
    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
        return Err(os_error(&format!("removing {KEY}\\{name}"), r));
    }
    Ok(())
}
