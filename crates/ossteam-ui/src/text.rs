// SPDX-License-Identifier: GPL-2.0-or-later
//! Text: `cosmic-text` shapes it and `swash` rasterises its glyphs into a
//! CPU-side cache that both renderers draw from (ADR 0004). The Vulkan
//! renderer copies each glyph it draws into its atlas once.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use cosmic_text::{
    Attrs, Buffer, FontSystem, LayoutGlyph, Metrics, Shaping, SwashCache, SwashContent, Weight,
    Wrap, fontdb,
};

use crate::fonts;
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
    /// Whether a visible character of `text` (the text this was shaped
    /// from) got no glyph: no loaded face has it.
    fn has_missing_glyphs(&self, text: &str) -> bool {
        missing_glyph(&self.glyphs, text)
    }

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

/// How a block of text is set: its size, line height and weight, in
/// logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextAttrs {
    pub size: f32,
    pub line_height: f32,
    /// 400 is regular, 700 bold.
    pub weight: u16,
}

/// A block of text broken into lines, in logical pixels.
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub lines: Vec<TextLine>,
    /// The widest line.
    pub width: f32,
    pub height: f32,
}

/// One line of a [`TextLayout`].
#[derive(Clone, Debug)]
pub struct TextLine {
    glyphs: Vec<LayoutGlyph>,
    /// The baseline, from the top of the layout.
    pub baseline: f32,
    pub width: f32,
}

impl TextLayout {
    fn has_missing_glyphs(&self, text: &str) -> bool {
        self.lines.iter().any(|l| missing_glyph(&l.glyphs, text))
    }
}

fn missing_glyph(glyphs: &[LayoutGlyph], text: &str) -> bool {
    glyphs.iter().any(|g| {
        g.glyph_id == 0
            && text
                .get(g.start..g.end)
                .is_some_and(|s| s.chars().any(|c| !c.is_whitespace() && !c.is_control()))
    })
}

/// The fonts could not be loaded.
#[derive(Debug)]
pub struct FontError(String);

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FontError {}

/// The fonts being loaded on their own thread
/// ([`TextSystem::load_in_background`]).
pub struct FontsLoading(mpsc::Receiver<Result<TextSystem, FontError>>);

impl FontsLoading {
    /// Waits until the UI font is loaded. The rest of the system's fonts
    /// may still be loading; text that needs them waits for them.
    pub fn wait(self) -> Result<TextSystem, FontError> {
        self.0
            .recv()
            .unwrap_or_else(|_| Err(FontError("the font thread ended without fonts".into())))
    }
}

/// Fonts, shaping and the glyph cache.
pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    pub glyphs: GlyphCache,
    /// How long loading the UI font took, from the start of loading.
    pub load_ms: f64,
    /// The UI family, if one of the platform's was found (`fonts.rs`).
    family: Option<String>,
    /// The rest of the system's fonts, from the font thread, and how long
    /// they took; merged when text first needs them.
    rest: Option<mpsc::Receiver<(fontdb::Database, f64)>>,
    rest_ms: Option<f64>,
}

impl TextSystem {
    /// Loads the UI font, then the rest of the system's fonts. Use
    /// [`TextSystem::load_in_background`] so the UI font is ready sooner.
    pub fn load() -> Result<Self, FontError> {
        let loading = Self::load_in_background();
        let mut text = loading.wait()?;
        text.merge_rest();
        Ok(text)
    }

    /// Starts loading on a thread of its own: the UI family's files
    /// first (a handful), then the rest of the system's fonts, which can
    /// take a second from a cold disk cache. Start it when the process
    /// starts; [`FontsLoading::wait`] returns as soon as the UI font is
    /// ready.
    pub fn load_in_background() -> FontsLoading {
        let (ready, ready_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("fonts".into())
            .spawn(move || {
                let started = Instant::now();
                let found = fonts::discover();
                let load_ms = started.elapsed().as_secs_f64() * 1000.0;
                if found.db.is_empty() {
                    let dirs: Vec<_> = found.dirs.iter().map(|d| d.display().to_string()).collect();
                    let _ = ready.send(Err(FontError(format!(
                        "found no fonts (looked in {})",
                        dirs.join(", ")
                    ))));
                    return;
                }
                let (rest_tx, rest_rx) = mpsc::channel();
                let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".into());
                let text = TextSystem {
                    fonts: FontSystem::new_with_locale_and_db(locale, found.db),
                    swash: SwashCache::new(),
                    glyphs: GlyphCache::default(),
                    load_ms,
                    family: found.family,
                    rest: (!found.rest.is_empty()).then_some(rest_rx),
                    rest_ms: None,
                };
                if ready.send(Ok(text)).is_err() || found.rest.is_empty() {
                    return;
                }
                let rest = fonts::load_files(&found.rest);
                let _ = rest_tx.send((rest, started.elapsed().as_secs_f64() * 1000.0));
            });
        if let Err(e) = spawned {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(FontError(format!("starting the font thread: {e}"))));
            return FontsLoading(rx);
        }
        FontsLoading(ready_rx)
    }

    /// How many font faces are loaded.
    pub fn face_count(&self) -> usize {
        self.fonts.db().len()
    }

    /// The UI family, if one of the platform's was found; otherwise every
    /// font was loaded before the first frame and fontdb's defaults stand.
    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// When the rest of the system's fonts had loaded, from the start of
    /// loading, if text has needed them so far.
    pub fn rest_ms(&self) -> Option<f64> {
        self.rest_ms
    }

    /// Whether the rest of the system's fonts are still to be merged.
    pub fn rest_pending(&self) -> bool {
        self.rest.is_some()
    }

    /// Merges the rest of the system's fonts before shaping `text` if the
    /// UI face lacks one of its characters. Shaping it with only the UI
    /// family loaded would send cosmic-text through every face it has,
    /// each mapped and kept for good: "▾" alone made 72 Noto Sans files,
    /// 16 MB, resident. The check after shaping stays for the rarer case
    /// that another face of the family lacks what this one has.
    fn merge_rest_if_lacking(&mut self, text: &str, weight: Weight) {
        if self.rest.is_none() {
            return;
        }
        let query = fontdb::Query {
            families: &[fontdb::Family::SansSerif],
            weight,
            ..fontdb::Query::default()
        };
        let Some(id) = self.fonts.db().query(&query) else {
            return;
        };
        let Some(font) = self.fonts.get_font(id, weight) else {
            return;
        };
        let charmap = font.as_swash().charmap();
        if text
            .chars()
            .any(|c| !c.is_whitespace() && !c.is_control() && charmap.map(c) == 0)
        {
            self.merge_rest();
        }
    }

    /// Waits for the rest of the system's fonts and adds them. The faces
    /// already loaded keep their ids, so shaped text and cached glyphs
    /// stay valid.
    fn merge_rest(&mut self) {
        let Some(rest) = self.rest.take() else {
            return;
        };
        let Ok((db, ms)) = rest.recv() else {
            return;
        };
        let live = self.fonts.db_mut();
        for face in db.faces() {
            live.push_face_info(face.clone());
        }
        self.rest_ms = Some(ms);
    }

    /// Shapes one line of text in the default sans-serif face, `size`
    /// logical pixels high.
    pub fn shape(&mut self, text: &str, size: f32) -> ShapedText {
        self.merge_rest_if_lacking(text, Weight::NORMAL);
        let shaped = self.shape_with_loaded(text, size);
        if self.rest.is_some() && shaped.has_missing_glyphs(text) {
            // A face the UI family lacks: wait for the rest of the fonts.
            self.merge_rest();
            return self.shape_with_loaded(text, size);
        }
        shaped
    }

    fn shape_with_loaded(&mut self, text: &str, size: f32) -> ShapedText {
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

    /// Sets `text` in lines no wider than `max_width` logical pixels
    /// (`None`: one line per paragraph), breaking between words where it
    /// can and inside a word that is wider than a line.
    pub fn layout(&mut self, text: &str, attrs: TextAttrs, max_width: Option<f32>) -> TextLayout {
        self.merge_rest_if_lacking(text, Weight(attrs.weight));
        let laid = self.layout_with_loaded(text, attrs, max_width);
        if self.rest.is_some() && laid.has_missing_glyphs(text) {
            self.merge_rest();
            return self.layout_with_loaded(text, attrs, max_width);
        }
        laid
    }

    fn layout_with_loaded(
        &mut self,
        text: &str,
        attrs: TextAttrs,
        max_width: Option<f32>,
    ) -> TextLayout {
        let metrics = Metrics::new(attrs.size, attrs.line_height);
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_wrap(Wrap::WordOrGlyph);
        // A hair of slack, so text measured at its own width does not wrap
        // again when laid out at that width.
        buffer.set_size(max_width.map(|w| w + 0.01), None);
        let font = Attrs::new().weight(Weight(attrs.weight));
        buffer.set_text(text, &font, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut out = TextLayout::default();
        for run in buffer.layout_runs() {
            out.width = out.width.max(run.line_w);
            out.height = out.height.max(run.line_top + run.line_height);
            out.lines.push(TextLine {
                glyphs: run.glyphs.to_vec(),
                baseline: run.line_y,
                width: run.line_w,
            });
        }
        if out.lines.is_empty() {
            out.height = attrs.line_height;
        }
        out
    }

    /// Adds a laid-out block to the scene with its top-left corner at
    /// `(x, y)` device pixels, drawn at `scale` device pixels per logical
    /// pixel. Each line's baseline is rounded to a whole device pixel.
    pub fn paint_layout(
        &mut self,
        scene: &mut Scene,
        layout: &TextLayout,
        x: f32,
        y: f32,
        scale: f32,
        color: Color,
    ) {
        for line in &layout.lines {
            let baseline = (y + line.baseline * scale).round();
            for g in &line.glyphs {
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
