// SPDX-License-Identifier: GPL-2.0-or-later
//! Text: `cosmic-text` shapes it and `swash` rasterises its glyphs into a
//! CPU-side cache that both renderers draw from (ADR 0004). The Vulkan
//! renderer copies each glyph it draws into its atlas once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cosmic_text::{
    Attrs, Buffer, FontSystem, LayoutGlyph, Metrics, Shaping, SwashCache, SwashContent,
};

use crate::scene::{Color, Glyph, Rect, Scene};

/// A glyph at a size and subpixel position: the glyph cache's key.
pub type GlyphKey = cosmic_text::CacheKey;

/// A rasterised glyph.
#[derive(Debug)]
pub struct GlyphImage {
    /// From the pen position to the image's top-left corner, in pixels;
    /// `top` is upward.
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// `true` for colour glyphs (emoji): `data` is RGBA with straight
    /// alpha. Otherwise `data` is one coverage byte per pixel.
    pub color: bool,
    pub data: Vec<u8>,
}

impl GlyphImage {
    pub fn bounds_at(&self, x: i32, y: i32) -> Rect {
        Rect::new(
            (x + self.left) as f32,
            (y - self.top) as f32,
            self.width as f32,
            self.height as f32,
        )
    }
}

/// Glyphs rasterised so far, shared by both renderers.
#[derive(Default)]
pub struct GlyphCache {
    glyphs: HashMap<GlyphKey, Option<Arc<GlyphImage>>>,
}

impl GlyphCache {
    /// The glyph's image, if it has been rasterised and is not empty.
    pub fn get(&self, key: &GlyphKey) -> Option<&Arc<GlyphImage>> {
        self.glyphs.get(key).and_then(Option::as_ref)
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// The bytes the cached images hold.
    pub fn bytes(&self) -> usize {
        self.glyphs.values().flatten().map(|g| g.data.len()).sum()
    }
}

/// A line of shaped text, in logical pixels. Shaped once, painted at any
/// position and scale.
#[derive(Clone, Debug)]
pub struct ShapedText {
    glyphs: Vec<LayoutGlyph>,
    /// The baseline, from the top of the line.
    pub baseline: f32,
    pub width: f32,
    pub height: f32,
    /// Where each glyph starts and ends in the text, and its x extent, for
    /// hit-testing and cursors in a text field.
    pub clusters: Vec<Cluster>,
}

#[derive(Clone, Copy, Debug)]
pub struct Cluster {
    pub start: usize,
    pub end: usize,
    pub x: f32,
    pub w: f32,
}

impl ShapedText {
    /// The x position of a byte offset into the text: the caret's position.
    pub fn x_of(&self, offset: usize) -> f32 {
        for c in &self.clusters {
            if offset <= c.start {
                return c.x;
            }
            if offset < c.end {
                return c.x + c.w;
            }
        }
        self.width
    }

    /// The byte offset nearest to `x`.
    pub fn offset_at(&self, x: f32) -> usize {
        for c in &self.clusters {
            if x < c.x + c.w / 2.0 {
                return c.start;
            }
        }
        self.clusters.last().map_or(0, |c| c.end)
    }
}

/// Fonts, shaping and the glyph cache.
pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    pub glyphs: GlyphCache,
    /// How long loading the system's fonts took.
    pub load_ms: f64,
}

impl TextSystem {
    /// Loads the system's fonts. This reads every font file's header and
    /// can take hundreds of milliseconds, so start it on a thread of its
    /// own when the process starts ([`TextSystem::load_in_background`]).
    pub fn load() -> Self {
        let started = Instant::now();
        let fonts = FontSystem::new();
        Self {
            fonts,
            swash: SwashCache::new(),
            glyphs: GlyphCache::default(),
            load_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }

    pub fn load_in_background() -> std::thread::JoinHandle<TextSystem> {
        std::thread::Builder::new()
            .name("fonts".into())
            .spawn(TextSystem::load)
            .expect("starting the font thread")
    }

    /// How many font faces were found.
    pub fn face_count(&self) -> usize {
        self.fonts.db().len()
    }

    /// Shapes one line of text in the default sans-serif face, `size`
    /// logical pixels high.
    pub fn shape(&mut self, text: &str, size: f32) -> ShapedText {
        let line_height = (size * 1.3).ceil();
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, line_height));
        buffer.set_size(None, None);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut glyphs = Vec::new();
        let mut clusters = Vec::new();
        let mut width: f32 = 0.0;
        let mut baseline = size;
        if let Some(run) = buffer.layout_runs().next() {
            baseline = run.line_y - run.line_top;
            width = run.line_w;
            for g in run.glyphs {
                clusters.push(Cluster {
                    start: g.start,
                    end: g.end,
                    x: g.x,
                    w: g.w,
                });
                glyphs.push(g.clone());
            }
        }
        ShapedText {
            glyphs,
            baseline,
            width,
            height: line_height,
            clusters,
        }
    }

    /// Adds the text to the scene with its top-left corner at `(x, y)`
    /// device pixels, drawn at `scale` device pixels per logical pixel.
    pub fn paint(
        &mut self,
        scene: &mut Scene,
        text: &ShapedText,
        x: f32,
        y: f32,
        scale: f32,
        color: Color,
    ) {
        let baseline = y + text.baseline * scale;
        for g in &text.glyphs {
            let p = g.physical((x, baseline), scale);
            let Some(image) = self.rasterise(p.cache_key) else {
                continue;
            };
            scene.glyph(Glyph {
                key: p.cache_key,
                x: p.x,
                y: p.y,
                color,
                bounds: image.bounds_at(p.x, p.y),
            });
        }
    }

    /// The glyph's image, rasterised now if this is its first use.
    pub fn rasterise(&mut self, key: GlyphKey) -> Option<Arc<GlyphImage>> {
        if let Some(cached) = self.glyphs.glyphs.get(&key) {
            return cached.clone();
        }
        let image = self
            .swash
            .get_image_uncached(&mut self.fonts, key)
            .and_then(|img| {
                if img.placement.width == 0 || img.placement.height == 0 {
                    return None;
                }
                let (color, data) = match img.content {
                    SwashContent::Mask => (false, img.data),
                    SwashContent::Color => (true, img.data),
                    // Subpixel masks are not asked for; keep their green channel.
                    SwashContent::SubpixelMask => (
                        false,
                        img.data.as_chunks::<4>().0.iter().map(|p| p[1]).collect(),
                    ),
                };
                Some(Arc::new(GlyphImage {
                    left: img.placement.left,
                    top: img.placement.top,
                    width: img.placement.width,
                    height: img.placement.height,
                    color,
                    data,
                }))
            });
        self.glyphs.glyphs.insert(key, image.clone());
        image
    }
}
