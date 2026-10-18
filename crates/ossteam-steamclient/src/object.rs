// SPDX-License-Identifier: GPL-2.0-or-later
//! Interface objects (`steamworks-abi.md` §2.3).

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::Mutex;

use crate::vtables::VERSIONS;

/// What a game holds a pointer to. The vtable comes first, as C++
/// objects have it.
#[repr(C)]
pub struct Object {
    vtable: *const c_void,
    /// Index into `VERSIONS`.
    pub version: usize,
    pub pipe: i32,
    pub user: i32,
}

struct Cache(BTreeMap<(usize, i32, i32), usize>);

static OBJECTS: Mutex<Cache> = Mutex::new(Cache(BTreeMap::new()));

/// The object of `version` for `pipe` and `user`: the same pointer for
/// the same three every time. Objects are never freed.
pub fn get(version: usize, pipe: i32, user: i32) -> *mut Object {
    let mut cache = OBJECTS.lock().unwrap_or_else(|e| e.into_inner());
    let addr = *cache.0.entry((version, pipe, user)).or_insert_with(|| {
        let obj = Box::new(Object {
            vtable: VERSIONS[version].vtable.as_ptr().cast(),
            version,
            pipe,
            user,
        });
        let first = !crate::report::handed_out(version);
        if first {
            crate::log::log!(
                Debug,
                "handing out {}",
                VERSIONS[version].name.to_string_lossy()
            );
        }
        Box::into_raw(obj) as usize
    });
    addr as *mut Object
}
