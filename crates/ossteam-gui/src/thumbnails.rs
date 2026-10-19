// SPDX-License-Identifier: GPL-2.0-or-later
//! Library capsules, decoded at the size they are drawn (`gui.md` §4.2).
//!
//! gpui's own image loading keeps every image it has decoded, at full
//! size, until the app exits: scrolling a library of 10,000 apps held
//! gigabytes. Capsules here are scaled to their cell on a background
//! thread, and only the most recently drawn are kept; the others leave
//! the GPU's atlas too.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui::{AppContext, Context, RenderImage, Task, Window};
use image::imageops::FilterType;

/// Capsules kept: more than a 4K display shows at once.
pub const CAPACITY: usize = 256;

/// An image file at a size in device pixels.
type Key = (Arc<Path>, u32, u32);

enum State {
    /// Dropping the task cancels the load.
    Loading {
        _load: Task<()>,
    },
    Ready(Arc<RenderImage>),
    Failed,
}

struct Entry {
    state: State,
    used: u64,
}

pub enum Thumbnail {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

#[derive(Default)]
pub struct Thumbnails {
    entries: HashMap<Key, Entry>,
    clock: u64,
}

impl Thumbnails {
    /// The image at `path` scaled to fill `width` × `height` device
    /// pixels, loaded in the background on first use. Views call this
    /// while drawing; the entity notifies when a load ends.
    pub fn get(
        &mut self,
        path: &Arc<Path>,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Thumbnail {
        self.clock += 1;
        let key = (path.clone(), width, height);
        if let Some(e) = self.entries.get_mut(&key) {
            e.used = self.clock;
            return match &e.state {
                State::Loading { .. } => Thumbnail::Loading,
                State::Ready(image) => Thumbnail::Ready(image.clone()),
                State::Failed => Thumbnail::Failed,
            };
        }
        let task = {
            let key = key.clone();
            let (path, w, h) = key.clone();
            let load = cx.background_spawn(async move {
                let bytes = std::fs::read(&path).ok()?;
                decode(&bytes, w, h).ok()
            });
            cx.spawn(async move |this, cx| {
                let image = load.await;
                let _ = this.update(cx, |t, cx| {
                    if let Some(e) = t.entries.get_mut(&key) {
                        e.state = match image {
                            Some(i) => State::Ready(Arc::new(i)),
                            None => State::Failed,
                        };
                        cx.notify();
                    }
                });
            })
        };
        self.entries.insert(
            key,
            Entry {
                state: State::Loading { _load: task },
                used: self.clock,
            },
        );
        self.evict(window, cx);
        Thumbnail::Loading
    }

    /// Drops the least recently drawn beyond [`CAPACITY`]. A dropped load
    /// is cancelled.
    fn evict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.len() <= CAPACITY {
            return;
        }
        let mut by_use: Vec<_> = self
            .entries
            .iter()
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        by_use.sort_unstable_by_key(|(used, _)| *used);
        let excess = self.entries.len() - CAPACITY;
        for (_, key) in by_use.into_iter().take(excess) {
            if let Some(Entry {
                state: State::Ready(image),
                ..
            }) = self.entries.remove(&key)
            {
                cx.drop_image(image, Some(window));
            }
        }
    }
}

/// Decodes an image and scales it to cover `width` × `height`, cropping
/// what overflows, into gpui's BGRA.
pub fn decode(bytes: &[u8], width: u32, height: u32) -> image::ImageResult<RenderImage> {
    let scaled = image::load_from_memory(bytes)?.resize_to_fill(
        width.max(1),
        height.max(1),
        FilterType::Triangle,
    );
    let mut pixels = scaled.into_rgba8();
    for p in pixels.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Ok(RenderImage::new([image::Frame::new(pixels)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("encode");
        out.into_inner()
    }

    #[test]
    fn decoded_at_the_cell_size_in_bgra() {
        let image = decode(&png(600, 900, [10, 20, 30]), 146, 221).expect("decodes");
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (146, 221));
        assert_eq!(
            &image.as_bytes(0).expect("one frame")[..4],
            &[30, 20, 10, 255]
        );
    }

    #[test]
    fn a_wider_image_is_cropped_to_cover() {
        let image = decode(&png(920, 430, [0, 0, 0]), 146, 221).expect("decodes");
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (146, 221));
    }

    #[test]
    fn not_an_image_fails() {
        assert!(decode(b"<html>404</html>", 146, 221).is_err());
    }
}
