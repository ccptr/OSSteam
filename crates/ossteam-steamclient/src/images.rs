// SPDX-License-Identifier: GPL-2.0-or-later
//! Images that `ISteamUtils` hands out by handle: the achievement icons
//! (`isteamuserstats.md` §4).

use std::sync::{Arc, Mutex};

pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA, rows top down.
    pub rgba: Box<[u8]>,
}

/// Handle `n` is `IMAGES[n - 1]`. Handles are never reused.
static IMAGES: Mutex<Vec<Arc<Image>>> = Mutex::new(Vec::new());

/// Keeps an image and returns its handle.
pub fn add(image: Image) -> i32 {
    let mut all = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    all.push(Arc::new(image));
    all.len().min(i32::MAX as usize) as i32
}

pub fn get(handle: i32) -> Option<Arc<Image>> {
    let index = usize::try_from(handle).ok()?.checked_sub(1)?;
    IMAGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(index)
        .cloned()
}
