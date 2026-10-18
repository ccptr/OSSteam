// SPDX-License-Identifier: GPL-2.0-or-later
//! The types the vtables and exports exchange with games
//! (`steamworks-abi.md` §2.2, §2.4).

use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Mutex;

/// The string a stub returns for `const char *` (§2.4).
pub static EMPTY: &CStr = c"";

/// `CSteamID` and `CGameID` returned by value: a 64-bit class, which
/// the C rules return like a one-field struct.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CSteamID(pub u64);

/// `SteamIPAddress_t`: 16 address bytes, then the type; packed to 1.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SteamIpAddress {
    pub address: [u8; 16],
    pub kind: u32,
}

/// `InputAnalogActionData_t` and `ControllerAnalogActionData_t`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct AnalogActionData {
    pub mode: i32,
    pub x: f32,
    pub y: f32,
    pub active: u8,
}

/// `InputDigitalActionData_t` and `ControllerDigitalActionData_t`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct DigitalActionData {
    pub state: u8,
    pub active: u8,
}

/// `InputMotionData_t` and `ControllerMotionData_t`: ten floats.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct MotionData {
    pub values: [f32; 10],
}

/// `CallbackMsg_t`, packed as callbacks are (§2.2).
#[cfg_attr(windows, repr(C, packed(8)))]
#[cfg_attr(not(windows), repr(C, packed(4)))]
pub struct CallbackMsg {
    pub user: i32,
    pub callback: i32,
    pub param: *mut u8,
    pub size: i32,
}

/// A vtable entry. Function pointers are plain addresses here so that
/// slots of every signature fit one array.
#[repr(transparent)]
pub struct Fp(pub *const c_void);

// SAFETY: the pointers are code addresses, never written.
unsafe impl Sync for Fp {}

/// One slot of the table.
pub struct SlotInfo {
    /// Index into `VERSIONS`.
    pub version: usize,
    pub method: &'static str,
}

/// One interface version of the table.
pub struct VersionInfo {
    pub name: &'static CStr,
    /// Index into `INTERFACES`.
    pub interface: usize,
    /// The index of its first slot in `SLOTS`.
    pub first_slot: usize,
    pub vtable: &'static [Fp],
}

impl VersionInfo {
    pub fn interface_name(&self) -> &'static str {
        crate::vtables::INTERFACES[self.interface]
    }
}

/// The version with this name.
pub fn find_version(name: &[u8]) -> Option<usize> {
    crate::vtables::VERSIONS
        .iter()
        .position(|v| v.name.to_bytes() == name)
}

/// A `const char *` for a game that stays valid for as long as the
/// process runs. Each distinct string is kept once, so a value that
/// changes (a persona name) costs memory only for the values it takes.
/// A string with a NUL in it is cut at the NUL.
pub fn intern(s: &str) -> *const c_char {
    static STRINGS: Mutex<Option<HashSet<&'static CStr>>> = Mutex::new(None);
    let s = s.split('\0').next().unwrap_or("");
    if s.is_empty() {
        return EMPTY.as_ptr();
    }
    let mut set = STRINGS.lock().unwrap_or_else(|e| e.into_inner());
    let set = set.get_or_insert_with(HashSet::new);
    let c = CString::new(s).expect("cut at the first NUL");
    if let Some(found) = set.get(c.as_c_str()) {
        return found.as_ptr();
    }
    let leaked: &'static CStr = Box::leak(c.into_boxed_c_str());
    set.insert(leaked);
    leaked.as_ptr()
}

/// Copies `s` into a game's buffer of `size` bytes with a NUL, if it
/// fits. Returns whether it did; the buffer is left empty when not.
///
/// # Safety
///
/// `buf` is null or holds `size` writable bytes.
pub unsafe fn write_str(buf: *mut c_void, size: i64, s: &str) -> bool {
    if buf.is_null() || size <= 0 {
        return false;
    }
    let buf = buf.cast::<u8>();
    if s.len() as i64 >= size {
        // SAFETY: size > 0, so the first byte is writable.
        unsafe { buf.write(0) };
        return false;
    }
    // SAFETY: s.len() + 1 <= size bytes are writable.
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
        buf.add(s.len()).write(0);
    }
    true
}

/// Copies as much of `s` as fits with a NUL into a game's buffer.
/// Returns the bytes written, the NUL included.
///
/// # Safety
///
/// `buf` is null or holds `size` writable bytes.
pub unsafe fn write_str_truncated(buf: *mut c_void, size: i64, s: &str) -> usize {
    if buf.is_null() || size <= 0 {
        return 0;
    }
    let mut n = s.len().min(size as usize - 1);
    // Never cut a UTF-8 sequence in half.
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    let buf = buf.cast::<u8>();
    // SAFETY: n + 1 <= size bytes are writable.
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n);
        buf.add(n).write(0);
    }
    n + 1
}

/// Reads a game's NUL-terminated string.
///
/// # Safety
///
/// `p` is null or points to a NUL-terminated string.
pub unsafe fn read_str<'a>(p: *const c_void) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(p.cast()) }.to_str().ok()
}

/// Writes a value through a game's out-pointer, if it is not null.
///
/// # Safety
///
/// `p` is null or valid for a write of `T`, with any alignment.
pub unsafe fn put<T>(p: *mut c_void, v: T) {
    if !p.is_null() {
        // SAFETY: as the caller promises.
        unsafe { p.cast::<T>().write_unaligned(v) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_sdk() {
        assert_eq!(size_of::<SteamIpAddress>(), 20);
        assert_eq!(size_of::<AnalogActionData>(), 13);
        assert_eq!(size_of::<DigitalActionData>(), 2);
        assert_eq!(size_of::<MotionData>(), 40);
        assert_eq!(std::mem::offset_of!(CallbackMsg, param), 8);
        let size = if cfg!(all(windows, target_pointer_width = "64")) {
            24
        } else if cfg!(target_pointer_width = "64") {
            20
        } else {
            16
        };
        assert_eq!(size_of::<CallbackMsg>(), size);
    }

    #[test]
    fn interned_strings_are_kept_once() {
        let a = intern("english");
        assert_eq!(a, intern("english"));
        assert_ne!(a, intern("german"));
        assert_eq!(intern(""), EMPTY.as_ptr());
        // SAFETY: interned strings live for the whole process.
        assert_eq!(unsafe { CStr::from_ptr(intern("a\0b")) }, c"a");
    }

    #[test]
    fn strings_fit_or_fail() {
        let mut buf = [0xffu8; 4];
        // SAFETY: buf holds 4 bytes.
        unsafe {
            assert!(write_str(buf.as_mut_ptr().cast(), 4, "abc"));
            assert_eq!(&buf, b"abc\0");
            assert!(!write_str(buf.as_mut_ptr().cast(), 4, "abcd"));
            assert_eq!(buf[0], 0);
            assert_eq!(write_str_truncated(buf.as_mut_ptr().cast(), 4, "aé€"), 4);
            assert_eq!(&buf, b"a\xc3\xa9\0");
        }
    }
}
