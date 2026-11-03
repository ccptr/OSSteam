// SPDX-License-Identifier: GPL-2.0-or-later
//! Images (ADR 0004, "Images are explicit"). The application decodes an
//! image and scales it to the size it is drawn at; the toolkit draws it
//! pixel for pixel. The renderers keep a copy of an image (the Vulkan
//! renderer a texture) only while the application still holds it: they
//! let go when the last [`Arc`] does. There is no cache of the toolkit's
//! own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Names an image for the renderers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(u64);

static NEXT_IMAGE: AtomicU64 = AtomicU64::new(1);

/// Pixels in premultiplied RGBA, 8 bits a channel, rows top to bottom.
#[derive(Debug)]
pub struct Image {
    id: ImageId,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// The pixels do not fill the size given.
#[derive(Debug, PartialEq, Eq)]
pub struct WrongSize {
    pub expected: usize,
    pub got: usize,
}

impl std::fmt::Display for WrongSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "image data is {} bytes, not {}", self.got, self.expected)
    }
}

impl std::error::Error for WrongSize {}

impl Image {
    /// From RGBA with straight (not premultiplied) alpha, as image
    /// decoders give it.
    pub fn from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Result<Arc<Image>, WrongSize> {
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(WrongSize {
                expected,
                got: rgba.len(),
            });
        }
        for p in rgba.as_chunks_mut::<4>().0 {
            let a = u16::from(p[3]);
            if a != 255 {
                for c in &mut p[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        Ok(Arc::new(Image {
            id: ImageId(NEXT_IMAGE.fetch_add(1, Ordering::Relaxed)),
            width,
            height,
            pixels: rgba,
        }))
    }

    pub fn id(&self) -> ImageId {
        self.id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_premultiplied_and_sizes_checked() {
        let img = Image::from_rgba(2, 1, vec![255, 128, 0, 128, 10, 20, 30, 255]).unwrap();
        assert_eq!(img.pixels(), &[128, 64, 0, 128, 10, 20, 30, 255]);
        assert_ne!(img.id(), Image::from_rgba(1, 1, vec![0; 4]).unwrap().id());
        assert_eq!(
            Image::from_rgba(2, 2, vec![0; 4]).unwrap_err(),
            WrongSize {
                expected: 16,
                got: 4
            }
        );
    }
}
