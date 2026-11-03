// SPDX-License-Identifier: GPL-2.0-or-later
//! Library capsules and App page artwork, decoded at the size they are
//! drawn (`gui.md` §4.2, §4.3).
//!
//! Images are scaled to where they are drawn on worker threads, newest
//! request first (what was asked for last is what is on screen), and only
//! the most recently drawn are kept: an image dropped here leaves the
//! renderer too, which keeps nothing the GUI has not kept (ADR 0004).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use image::imageops::FilterType;
use ossteam_ui::image::Image;

/// Decoding threads, started as loads need them.
const WORKERS: usize = 4;

/// How an image is scaled to its box.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fit {
    /// Fills the box, cropping what overflows.
    Cover,
    /// Fits inside the box, keeping its shape; never enlarged.
    Contain,
}

/// An image file in a box in device pixels.
pub type Key = (Arc<Path>, u32, u32, Fit);

/// A finished load, for [`Thumbnails::loaded`] on the thread that owns the
/// cache.
pub struct Loaded(Key, Option<Arc<Image>>);

/// Hands a finished load to the thread that owns the cache.
pub type Post = Arc<dyn Fn(Loaded) + Send + Sync>;

enum State {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

struct Entry {
    state: State,
    used: u64,
}

pub enum Thumbnail {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

/// The loads not started yet, newest last, shared with the workers.
#[derive(Default)]
struct Queue {
    waiting: Mutex<VecDeque<Key>>,
    ready: Condvar,
    /// The cache is gone; the workers end.
    closed: AtomicBool,
}

pub struct Thumbnails {
    /// Filled as views build what is visible, so behind a cell: building
    /// takes the state by reference.
    inner: RefCell<Inner>,
    capacity: usize,
    queue: Arc<Queue>,
    post: Post,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Key, Entry>,
    clock: u64,
    workers: usize,
}

impl Thumbnails {
    /// A cache of the `capacity` images drawn last; finished loads go to
    /// `post`.
    pub fn new(capacity: usize, post: Post) -> Self {
        Self {
            inner: RefCell::default(),
            capacity,
            queue: Arc::default(),
            post,
        }
    }

    /// The image at `path` scaled to a box of `width` × `height` device
    /// pixels, loaded in the background on first use. Views call this for
    /// what they show, as they build it.
    pub fn get(&self, path: &Arc<Path>, (width, height): (u32, u32), fit: Fit) -> Thumbnail {
        let mut inner = self.inner.borrow_mut();
        inner.clock += 1;
        let clock = inner.clock;
        let key = (path.clone(), width, height, fit);
        if let Some(e) = inner.entries.get_mut(&key) {
            e.used = clock;
            return match &e.state {
                State::Loading => Thumbnail::Loading,
                State::Ready(image) => Thumbnail::Ready(image.clone()),
                State::Failed => Thumbnail::Failed,
            };
        }
        inner.entries.insert(
            key.clone(),
            Entry {
                state: State::Loading,
                used: clock,
            },
        );
        if let Ok(mut waiting) = self.queue.waiting.lock() {
            waiting.push_back(key);
        }
        self.queue.ready.notify_one();
        if inner.workers < WORKERS && self.start_worker() {
            inner.workers += 1;
        }
        self.evict(&mut inner);
        Thumbnail::Loading
    }

    /// Takes a finished load; dropped if its entry was evicted meanwhile.
    pub fn loaded(&mut self, Loaded(key, image): Loaded) {
        if let Some(e) = self.inner.get_mut().entries.get_mut(&key) {
            e.state = match image {
                Some(i) => State::Ready(i),
                None => State::Failed,
            };
        }
    }

    fn start_worker(&self) -> bool {
        let queue = self.queue.clone();
        let post = self.post.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || work(&queue, &*post))
            .is_ok()
    }

    /// Drops the least recently drawn beyond the capacity; a load not
    /// started yet is not made.
    fn evict(&self, inner: &mut Inner) {
        if inner.entries.len() <= self.capacity {
            return;
        }
        let mut by_use: Vec<_> = inner
            .entries
            .iter()
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        by_use.sort_unstable_by_key(|(used, _)| *used);
        let excess = inner.entries.len() - self.capacity;
        let gone: Vec<Key> = by_use.into_iter().take(excess).map(|(_, k)| k).collect();
        for key in &gone {
            inner.entries.remove(key);
        }
        if let Ok(mut waiting) = self.queue.waiting.lock() {
            waiting.retain(|k| !gone.contains(k));
        }
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        // Under the lock, so a worker cannot miss it between its check
        // and its wait.
        if let Ok(_waiting) = self.queue.waiting.lock() {
            self.queue.closed.store(true, Ordering::Relaxed);
        }
        self.queue.ready.notify_all();
    }
}

/// A worker: decodes the newest waiting load; it sleeps while nothing
/// waits, and ends when the cache is gone.
fn work(queue: &Queue, post: &(dyn Fn(Loaded) + Send + Sync)) {
    loop {
        let key = {
            let Ok(mut waiting) = queue.waiting.lock() else {
                return;
            };
            loop {
                if queue.closed.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(key) = waiting.pop_back() {
                    break key;
                }
                waiting = match queue.ready.wait(waiting) {
                    Ok(w) => w,
                    Err(_) => return,
                };
            }
        };
        let (path, w, h, fit) = key.clone();
        let image = std::fs::read(&path)
            .ok()
            .and_then(|bytes| decode(&bytes, w, h, fit).ok());
        post(Loaded(key, image));
    }
}

/// Decodes an image and scales it to a box of `width` × `height`.
pub fn decode(bytes: &[u8], width: u32, height: u32, fit: Fit) -> image::ImageResult<Arc<Image>> {
    let image = image::load_from_memory(bytes)?;
    let (width, height) = (width.max(1), height.max(1));
    let scaled = match fit {
        Fit::Cover => image.resize_to_fill(width, height, FilterType::Triangle),
        Fit::Contain if image.width() <= width && image.height() <= height => image,
        Fit::Contain => image.resize(width, height, FilterType::Triangle),
    };
    let (w, h) = (scaled.width(), scaled.height());
    Image::from_rgba(w, h, scaled.into_rgba8().into_raw()).map_err(|_| {
        image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ))
    })
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
    fn decoded_at_the_cell_size() {
        let image = decode(&png(600, 900, [10, 20, 30]), 146, 221, Fit::Cover).expect("decodes");
        assert_eq!((image.width(), image.height()), (146, 221));
        assert_eq!(&image.pixels()[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn a_wider_image_is_cropped_to_cover() {
        let image = decode(&png(920, 430, [0, 0, 0]), 146, 221, Fit::Cover).expect("decodes");
        assert_eq!((image.width(), image.height()), (146, 221));
    }

    #[test]
    fn contain_keeps_the_shape_and_never_enlarges() {
        let image = decode(&png(1280, 720, [0, 0, 0]), 640, 280, Fit::Contain).expect("decodes");
        assert_eq!((image.width(), image.height()), (498, 280));
        let image = decode(&png(200, 80, [0, 0, 0]), 640, 280, Fit::Contain).expect("decodes");
        assert_eq!((image.width(), image.height()), (200, 80));
    }

    #[test]
    fn not_an_image_fails() {
        assert!(decode(b"<html>404</html>", 146, 221, Fit::Cover).is_err());
    }
}
