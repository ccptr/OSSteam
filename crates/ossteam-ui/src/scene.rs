// SPDX-License-Identifier: LGPL-2.1-or-later
//! What a frame draws, in device pixels (ADR 0004, "Two renderers, one
//! scene"). The Vulkan and the software renderer draw the same scene, so
//! everything here is plain data: rounded rectangles with borders and
//! shadows, and glyphs from the shared glyph cache, in paint order, each
//! with the clip rectangle it is drawn in.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::image::{Image, ImageId};

/// A colour in sRGB with straight alpha. Both renderers blend in sRGB
/// space, so they agree with each other (and with what the GUI drew
/// before it moved to this toolkit).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// `0xRRGGBB`, opaque.
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as f32 / 255.0,
            g: ((hex >> 8) & 0xff) as f32 / 255.0,
            b: (hex & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    fn hash_into(self, state: &mut impl Hasher) {
        for c in self.to_array() {
            c.to_bits().hash(state);
        }
    }
}

/// An axis-aligned rectangle in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn outset(&self, by: f32) -> Rect {
        Rect::new(
            self.x - by,
            self.y - by,
            self.w + 2.0 * by,
            self.h + 2.0 * by,
        )
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    fn hash_into(&self, state: &mut impl Hasher) {
        for v in [self.x, self.y, self.w, self.h] {
            v.to_bits().hash(state);
        }
    }
}

/// A rounded rectangle with an optional border, or, with `blur` above
/// zero, the shadow of one: the same signed distance with a Gaussian
/// falloff of standard deviation `blur`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub bounds: Rect,
    pub background: Color,
    pub border_color: Color,
    pub border_width: f32,
    pub corner_radius: f32,
    pub blur: f32,
}

impl Quad {
    pub fn filled(bounds: Rect, background: Color) -> Self {
        Self {
            bounds,
            background,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
            corner_radius: 0.0,
            blur: 0.0,
        }
    }

    /// The pixels the quad can touch.
    pub fn paint_bounds(&self) -> Rect {
        if self.blur > 0.0 {
            self.bounds.outset(3.0 * self.blur)
        } else {
            self.bounds
        }
    }
}

/// One glyph from the glyph cache, its image's origin at a whole device
/// pixel, so neither renderer scales or filters it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub key: crate::text::GlyphKey,
    /// Where the glyph's pen position is, in device pixels.
    pub x: i32,
    pub y: i32,
    pub color: Color,
    /// The glyph image's bounds, filled in from the glyph cache when the
    /// glyph is added, so damage tracking needs no cache lookups.
    pub bounds: Rect,
}

/// An image drawn pixel for pixel with its top-left corner at a whole
/// device pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageQuad {
    pub id: ImageId,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Multiplies every pixel.
    pub opacity: f32,
    /// The rounded rectangle the image shows through, in device pixels,
    /// its corners cut at `corner_radius`; a radius of 0 cuts nothing.
    pub round: Rect,
    pub corner_radius: f32,
}

impl ImageQuad {
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Quad(Quad),
    Glyph(Glyph),
    Image(ImageQuad),
}

impl Primitive {
    pub fn paint_bounds(&self) -> Rect {
        match self {
            Primitive::Quad(q) => q.paint_bounds(),
            Primitive::Glyph(g) => g.bounds,
            Primitive::Image(i) => i.bounds(),
        }
    }
}

/// A primitive with the clip it is drawn in and the scroll layer it moves with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub primitive: Primitive,
    pub clip: Rect,
    /// 0 for the window; otherwise the id of the [`ScrollLayer`] it is in.
    pub layer: u32,
}

impl Item {
    /// What the item covers on screen.
    pub fn visible_bounds(&self) -> Rect {
        self.primitive.paint_bounds().intersect(&self.clip)
    }

    /// A hash of the item as it would be drawn if `dy` were added to its
    /// position: scroll layers hash their items in content coordinates, so
    /// an item that only moved with its layer hashes the same.
    pub fn content_hash(&self, dy: f32) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        self.layer.hash(&mut h);
        self.clip.hash_into(&mut h);
        match &self.primitive {
            Primitive::Quad(q) => {
                0u8.hash(&mut h);
                q.bounds.translate(0.0, dy).hash_into(&mut h);
                q.background.hash_into(&mut h);
                q.border_color.hash_into(&mut h);
                for v in [q.border_width, q.corner_radius, q.blur] {
                    v.to_bits().hash(&mut h);
                }
            }
            Primitive::Glyph(g) => {
                1u8.hash(&mut h);
                g.key.hash(&mut h);
                g.x.hash(&mut h);
                (g.y as f32 + dy).to_bits().hash(&mut h);
                g.color.hash_into(&mut h);
            }
            Primitive::Image(i) => {
                2u8.hash(&mut h);
                i.id.hash(&mut h);
                i.x.hash(&mut h);
                (i.y as f32 + dy).to_bits().hash(&mut h);
                i.opacity.to_bits().hash(&mut h);
                i.round.translate(0.0, dy).hash_into(&mut h);
                i.corner_radius.to_bits().hash(&mut h);
            }
        }
        h.finish()
    }
}

/// A region whose content moves together by a whole number of device
/// pixels. The software renderer moves the pixels it already drew and
/// draws only the strip a scroll reveals (ADR 0004).
///
/// A layer is opaque: it starts with a backdrop of `background` over its
/// whole rectangle, so what was drawn before it never shows through and
/// need not be redrawn when it scrolls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollLayer {
    pub id: u32,
    pub rect: Rect,
    /// How far the content is scrolled, in whole device pixels.
    pub offset_y: i32,
    pub background: Color,
    /// The layer's items are `items[first_item..end_item]`; window items
    /// after `end_item` are drawn over it.
    pub first_item: usize,
    pub end_item: usize,
}

/// One frame's drawing.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub background: Color,
    pub items: Vec<Item>,
    pub layers: Vec<ScrollLayer>,
    /// The images the frame draws.
    pub images: HashMap<ImageId, Arc<Image>>,
    clips: Vec<Rect>,
    layer_stack: Vec<u32>,
    /// The size of the target in device pixels.
    pub width: u32,
    pub height: u32,
}

impl Scene {
    pub fn new(width: u32, height: u32, background: Color) -> Self {
        Self {
            background,
            width,
            height,
            ..Default::default()
        }
    }

    pub fn clear(&mut self, width: u32, height: u32, background: Color) {
        self.items.clear();
        self.layers.clear();
        self.images.clear();
        self.clips.clear();
        self.layer_stack.clear();
        self.width = width;
        self.height = height;
        self.background = background;
    }

    pub fn viewport(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f32, self.height as f32)
    }

    fn current_clip(&self) -> Rect {
        self.clips
            .last()
            .copied()
            .unwrap_or_else(|| self.viewport())
    }

    fn current_layer(&self) -> u32 {
        self.layer_stack.last().copied().unwrap_or(0)
    }

    pub fn push_clip(&mut self, rect: Rect) {
        let clip = self.current_clip().intersect(&rect);
        self.clips.push(clip);
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// Starts a scroll layer: what is added until [`Scene::pop_scroll_layer`]
    /// is clipped to `rect` and moves with `offset_y`.
    ///
    /// Layers do not nest.
    pub fn push_scroll_layer(&mut self, id: u32, rect: Rect, offset_y: i32, background: Color) {
        assert_ne!(id, 0, "layer 0 is the window");
        assert!(self.layer_stack.is_empty(), "scroll layers do not nest");
        self.quad(Quad::filled(rect, background));
        let first_item = self.items.len();
        self.layers.push(ScrollLayer {
            id,
            rect,
            offset_y,
            background,
            first_item,
            end_item: first_item,
        });
        self.layer_stack.push(id);
        self.push_clip(rect);
    }

    pub fn pop_scroll_layer(&mut self) {
        self.pop_clip();
        self.layer_stack.pop();
        let end = self.items.len();
        if let Some(layer) = self.layers.last_mut() {
            layer.end_item = end;
        }
    }

    pub fn quad(&mut self, quad: Quad) {
        self.push(Primitive::Quad(quad));
    }

    pub fn glyph(&mut self, glyph: Glyph) {
        self.push(Primitive::Glyph(glyph));
    }

    /// Draws `image` with its top-left corner at `(x, y)`.
    pub fn image(&mut self, image: &Arc<Image>, x: i32, y: i32, opacity: f32) {
        let bounds = Rect::new(
            x as f32,
            y as f32,
            image.width() as f32,
            image.height() as f32,
        );
        self.image_rounded(image, x, y, opacity, bounds, 0.0);
    }

    /// Draws `image` with its top-left corner at `(x, y)`, showing only
    /// what lies inside `round` with its corners cut at `corner_radius`.
    pub fn image_rounded(
        &mut self,
        image: &Arc<Image>,
        x: i32,
        y: i32,
        opacity: f32,
        round: Rect,
        corner_radius: f32,
    ) {
        self.images
            .entry(image.id())
            .or_insert_with(|| image.clone());
        self.push(Primitive::Image(ImageQuad {
            id: image.id(),
            x,
            y,
            width: image.width(),
            height: image.height(),
            opacity,
            round,
            corner_radius,
        }));
    }

    /// Whether a scroll layer is open: layers do not nest.
    pub fn in_scroll_layer(&self) -> bool {
        !self.layer_stack.is_empty()
    }

    fn push(&mut self, primitive: Primitive) {
        let clip = self.current_clip();
        let item = Item {
            primitive,
            clip,
            layer: self.current_layer(),
        };
        if !item.visible_bounds().is_empty() {
            self.items.push(item);
        }
    }
}
