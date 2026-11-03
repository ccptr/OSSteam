// SPDX-License-Identifier: GPL-2.0-or-later
//! The software renderer (ADR 0004): draws a [`Scene`] into a 32-bit
//! buffer with no GPU driver, redrawing only what changed.
//!
//! Each frame is compared with the last. Items are hashed, those in a
//! scroll layer in content coordinates, so an item that only moved with
//! its layer counts as unchanged: the layer's pixels are moved instead,
//! and only the strip the scroll reveals is drawn. Whatever an added or
//! removed item covered is damage. Damage is gathered per 64-pixel tile,
//! and each damaged tile redraws the bounding box of its damage. When much
//! of the window is damaged, bands of tiles are drawn on a few threads.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::render::shape::quad_pixel;
use crate::scene::{Color, Item, Primitive, Quad, Rect, Scene, ScrollLayer};
use crate::text::{GlyphCache, GlyphImage};

const TILE: i32 = 64;
/// Damage, in pixels, above which tiles are drawn on several threads.
const PARALLEL_PIXELS: i64 = 256 * 256;
/// At most this many threads draw a frame.
const MAX_THREADS: usize = 8;
/// How many frames of damage are kept for buffers presented again later
/// (a buffer's age).
const HISTORY: usize = 3;

/// A rectangle of whole pixels, `x0..x1` by `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelRect {
    /// The pixels `rect` touches.
    pub fn covering(rect: &Rect) -> Self {
        Self {
            x0: rect.x.floor() as i32,
            y0: rect.y.floor() as i32,
            x1: rect.right().ceil() as i32,
            y1: rect.bottom().ceil() as i32,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn intersect(&self, o: &PixelRect) -> PixelRect {
        PixelRect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    pub fn union(&self, o: &PixelRect) -> PixelRect {
        PixelRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            (self.x1 - self.x0) as i64 * (self.y1 - self.y0) as i64
        }
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }
}

/// What the last frame cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawStats {
    pub ms: f64,
    /// Pixels redrawn.
    pub pixels: i64,
    pub tiles: usize,
    /// The whole buffer was redrawn.
    pub full: bool,
    /// Rows moved by scrolling.
    pub moved_rows: i64,
}

struct PrevItem {
    hash: u64,
    bounds: Rect,
    layer: u32,
    index: usize,
}

struct PrevFrame {
    width: u32,
    height: u32,
    background: Color,
    items: Vec<PrevItem>,
    layers: Vec<ScrollLayer>,
}

/// A scene drawn into memory: `0x00RRGGBB` per pixel, opaque.
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    prev: Option<PrevFrame>,
    /// Each recent frame's damage, newest first.
    history: VecDeque<Vec<PixelRect>>,
    threads: usize,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    pub fn new() -> Self {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(MAX_THREADS);
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            prev: None,
            history: VecDeque::new(),
            threads,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The pixels as RGBA bytes, for tests and screenshots.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255])
            .collect()
    }

    /// What changed in the last `frames` frames, or `None` if that is not
    /// known (the buffer must then be copied whole).
    pub fn damage_since(&self, frames: usize) -> Option<Vec<PixelRect>> {
        if frames == 0 || frames > self.history.len() {
            return None;
        }
        Some(
            self.history
                .iter()
                .take(frames)
                .flatten()
                .copied()
                .collect(),
        )
    }

    /// The last frame's damage.
    pub fn last_damage(&self) -> &[PixelRect] {
        self.history.front().map_or(&[], Vec::as_slice)
    }

    /// Forgets the last frame, so the next draw redraws everything.
    pub fn invalidate(&mut self) {
        self.prev = None;
    }

    pub fn draw(&mut self, scene: &Scene, glyphs: &GlyphCache) -> DrawStats {
        let started = Instant::now();
        let mut stats = DrawStats::default();
        let viewport = PixelRect {
            x0: 0,
            y0: 0,
            x1: scene.width as i32,
            y1: scene.height as i32,
        };
        let full = match &self.prev {
            None => true,
            Some(p) => {
                p.width != scene.width
                    || p.height != scene.height
                    || p.background != scene.background
            }
        };
        if self.width != scene.width || self.height != scene.height {
            self.width = scene.width;
            self.height = scene.height;
            self.pixels = vec![0; scene.width as usize * scene.height as usize];
            // Damage drawn at another size does not bring a buffer of this
            // size up to date: a buffer older than the resize is copied whole.
            self.history.clear();
        }
        let damage = if full {
            stats.full = true;
            vec![viewport]
        } else {
            self.diff(scene, &mut stats)
        };

        // Gather damage per tile.
        let tiles_x = (scene.width as i32 + TILE - 1) / TILE;
        let tiles_y = (scene.height as i32 + TILE - 1) / TILE;
        let mut tiles: Vec<Option<PixelRect>> = vec![None; (tiles_x * tiles_y) as usize];
        for d in &damage {
            let d = d.intersect(&viewport);
            if d.is_empty() {
                continue;
            }
            for ty in d.y0 / TILE..=(d.y1 - 1) / TILE {
                for tx in d.x0 / TILE..=(d.x1 - 1) / TILE {
                    let tile = PixelRect {
                        x0: tx * TILE,
                        y0: ty * TILE,
                        x1: (tx + 1) * TILE,
                        y1: (ty + 1) * TILE,
                    };
                    let part = d.intersect(&tile);
                    let slot = &mut tiles[(ty * tiles_x + tx) as usize];
                    *slot = Some(slot.map_or(part, |s| s.union(&part)));
                }
            }
        }

        // Which items each damaged tile draws.
        let mut tile_items: Vec<Vec<u32>> = vec![Vec::new(); tiles.len()];
        for (index, item) in scene.items.iter().enumerate() {
            let b = PixelRect::covering(&item.visible_bounds()).intersect(&viewport);
            if b.is_empty() {
                continue;
            }
            for ty in b.y0 / TILE..=(b.y1 - 1) / TILE {
                for tx in b.x0 / TILE..=(b.x1 - 1) / TILE {
                    let t = (ty * tiles_x + tx) as usize;
                    if let Some(rect) = tiles[t]
                        && !rect.intersect(&b).is_empty()
                    {
                        tile_items[t].push(index as u32);
                    }
                }
            }
        }
        let dirty: Vec<PixelRect> = tiles.iter().flatten().copied().collect();
        stats.tiles = dirty.len();
        stats.pixels = dirty.iter().map(PixelRect::area).sum();

        // Draw, by bands of one tile row.
        let width = scene.width as usize;
        let band_rows = TILE as usize * width;
        let draw_band = |ty: usize, band: &mut [u32]| {
            let mut target = Target {
                pixels: band,
                width,
                y0: ty as i32 * TILE,
            };
            for tx in 0..tiles_x as usize {
                let t = ty * tiles_x as usize + tx;
                let Some(rect) = tiles[t] else { continue };
                target.fill(&rect, scene.background);
                for &index in &tile_items[t] {
                    draw_item(&mut target, &scene.items[index as usize], &rect, glyphs);
                }
            }
        };
        let bands = self.pixels.chunks_mut(band_rows.max(1));
        if stats.pixels > PARALLEL_PIXELS && self.threads > 1 {
            let bands: Vec<(usize, &mut [u32])> = bands
                .enumerate()
                .filter(|(ty, _)| {
                    (0..tiles_x as usize).any(|tx| tiles[ty * tiles_x as usize + tx].is_some())
                })
                .collect();
            // Every n-th band to each thread, so busy and quiet parts of the
            // window are shared out evenly.
            let n = self.threads.min(bands.len());
            let mut shares: Vec<Vec<(usize, &mut [u32])>> = (0..n).map(|_| Vec::new()).collect();
            for (i, band) in bands.into_iter().enumerate() {
                shares[i % n].push(band);
            }
            std::thread::scope(|s| {
                for share in shares {
                    let draw_band = &draw_band;
                    s.spawn(move || {
                        for (ty, band) in share {
                            draw_band(ty, band);
                        }
                    });
                }
            });
        } else {
            for (ty, band) in bands.enumerate() {
                draw_band(ty, band);
            }
        }

        self.history.push_front(dirty);
        self.history.truncate(HISTORY);
        self.prev = Some(PrevFrame {
            width: scene.width,
            height: scene.height,
            background: scene.background,
            items: scene
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| PrevItem {
                    hash: item.content_hash(layer_offset(scene, item.layer)),
                    bounds: item.visible_bounds(),
                    layer: item.layer,
                    index,
                })
                .collect(),
            layers: scene.layers.clone(),
        });
        stats.ms = started.elapsed().as_secs_f64() * 1000.0;
        stats
    }

    /// Moves what scrolled and returns what must be redrawn.
    fn diff(&mut self, scene: &Scene, stats: &mut DrawStats) -> Vec<PixelRect> {
        let prev = self.prev.take().expect("diff needs a previous frame");
        let mut damage = Vec::new();
        // How far each layer's pixels moved down; absent if it did not
        // carry over (new, gone, or a different rectangle).
        let mut moved: HashMap<u32, i32> = HashMap::new();
        for layer in &scene.layers {
            let old = prev.layers.iter().find(|l| l.id == layer.id);
            match old {
                Some(old) if old.rect == layer.rect && old.background == layer.background => {
                    let shift = old.offset_y - layer.offset_y;
                    let rect = PixelRect::covering(&layer.rect);
                    if shift.abs() >= rect.height() {
                        damage.push(rect);
                    } else if shift != 0 {
                        stats.moved_rows += self.scroll(&rect, shift);
                        damage.push(if shift < 0 {
                            PixelRect {
                                y0: rect.y1 + shift,
                                ..rect
                            }
                        } else {
                            PixelRect {
                                y1: rect.y0 + shift,
                                ..rect
                            }
                        });
                    }
                    moved.insert(layer.id, shift);
                }
                Some(old) => {
                    damage.push(PixelRect::covering(&old.rect));
                    damage.push(PixelRect::covering(&layer.rect));
                }
                None => damage.push(PixelRect::covering(&layer.rect)),
            }
        }
        for old in &prev.layers {
            if !scene.layers.iter().any(|l| l.id == old.id) {
                damage.push(PixelRect::covering(&old.rect));
            }
        }

        let mut unmatched: HashMap<u64, u32> = HashMap::new();
        for p in &prev.items {
            *unmatched.entry(p.hash).or_default() += 1;
        }
        let mut matched_current = vec![false; scene.items.len()];
        for (i, item) in scene.items.iter().enumerate() {
            let hash = item.content_hash(layer_offset(scene, item.layer));
            if let Some(n) = unmatched.get_mut(&hash)
                && *n > 0
            {
                *n -= 1;
                matched_current[i] = true;
            }
        }
        for (i, item) in scene.items.iter().enumerate() {
            if !matched_current[i] {
                damage.push(PixelRect::covering(&item.visible_bounds()));
            }
        }
        for p in &prev.items {
            let Some(n) = unmatched.get_mut(&p.hash) else {
                continue;
            };
            if *n == 0 {
                continue;
            }
            *n -= 1;
            damage.push(moved_bounds(&p.bounds, p.layer, &moved, &scene.layers));
        }

        // Window items over a layer that moved were moved with it.
        for layer in &scene.layers {
            let Some(&shift) = moved.get(&layer.id) else {
                continue;
            };
            if shift == 0 {
                continue;
            }
            let rect = PixelRect::covering(&layer.rect);
            for item in &scene.items[layer.end_item..] {
                if item.layer == 0 {
                    damage.push(PixelRect::covering(&item.visible_bounds()).intersect(&rect));
                }
            }
            if let Some(old) = prev.layers.iter().find(|l| l.id == layer.id) {
                for p in prev
                    .items
                    .iter()
                    .filter(|p| p.layer == 0 && p.index >= old.end_item)
                {
                    let ghost = PixelRect::covering(&p.bounds).intersect(&rect);
                    damage.push(
                        PixelRect {
                            y0: ghost.y0 + shift,
                            y1: ghost.y1 + shift,
                            ..ghost
                        }
                        .intersect(&rect),
                    );
                }
            }
        }
        damage.retain(|d| !d.is_empty());
        damage
    }

    /// Moves the pixels in `rect` down by `shift` rows (up if negative).
    fn scroll(&mut self, rect: &PixelRect, shift: i32) -> i64 {
        let w = self.width as i32;
        let r = rect.intersect(&PixelRect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: self.height as i32,
        });
        if r.is_empty() {
            return 0;
        }
        let (x0, x1) = (r.x0 as usize, r.x1 as usize);
        let rows = r.height() - shift.abs();
        let row = |y: i32| y as usize * w as usize;
        if shift < 0 {
            for y in r.y0..r.y0 + rows {
                let src = row(y - shift);
                self.pixels.copy_within(src + x0..src + x1, row(y) + x0);
            }
        } else {
            for y in (r.y0 + shift..r.y1).rev() {
                let src = row(y - shift);
                self.pixels.copy_within(src + x0..src + x1, row(y) + x0);
            }
        }
        rows as i64
    }
}

fn layer_offset(scene: &Scene, layer: u32) -> f32 {
    if layer == 0 {
        return 0.0;
    }
    scene
        .layers
        .iter()
        .find(|l| l.id == layer)
        .map_or(0.0, |l| l.offset_y as f32)
}

/// Where an item of the last frame is now, after its layer moved.
fn moved_bounds(
    bounds: &Rect,
    layer: u32,
    moved: &HashMap<u32, i32>,
    layers: &[ScrollLayer],
) -> PixelRect {
    let b = PixelRect::covering(bounds);
    match (moved.get(&layer), layers.iter().find(|l| l.id == layer)) {
        (Some(&shift), Some(l)) => PixelRect {
            y0: b.y0 + shift,
            y1: b.y1 + shift,
            ..b
        }
        .intersect(&PixelRect::covering(&l.rect)),
        _ => b,
    }
}

/// One band of the canvas: whole rows from `y0`.
struct Target<'a> {
    pixels: &'a mut [u32],
    width: usize,
    y0: i32,
}

impl Target<'_> {
    #[inline]
    fn at(&mut self, x: i32, y: i32) -> &mut u32 {
        &mut self.pixels[(y - self.y0) as usize * self.width + x as usize]
    }

    fn fill(&mut self, rect: &PixelRect, color: Color) {
        let c = pack(color.r, color.g, color.b);
        for y in rect.y0..rect.y1 {
            let start = (y - self.y0) as usize * self.width;
            self.pixels[start + rect.x0 as usize..start + rect.x1 as usize].fill(c);
        }
    }
}

#[inline]
fn pack(r: f32, g: f32, b: f32) -> u32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

#[inline]
fn div255(x: u32) -> u32 {
    (x + 128 + ((x + 128) >> 8)) >> 8
}

/// Blends a premultiplied source, each channel 0..=255, over `dst`.
#[inline]
fn blend(dst: u32, r: u32, g: u32, b: u32, a: u32) -> u32 {
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return (r << 16) | (g << 8) | b;
    }
    let inv = 255 - a;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    ((r + div255(dr * inv)) << 16) | ((g + div255(dg * inv)) << 8) | (b + div255(db * inv))
}

#[inline]
fn blend_f(dst: u32, src: [f32; 4]) -> u32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    blend(dst, c(src[0]), c(src[1]), c(src[2]), c(src[3]))
}

fn draw_item(target: &mut Target<'_>, item: &Item, tile: &PixelRect, glyphs: &GlyphCache) {
    let clip = PixelRect::covering(&item.clip).intersect(tile);
    match &item.primitive {
        Primitive::Quad(q) => draw_quad(target, q, &clip),
        Primitive::Glyph(g) => {
            if let Some(image) = glyphs.get(&g.key) {
                draw_glyph(target, image, g.x, g.y, g.color, &clip);
            }
        }
    }
}

fn draw_quad(target: &mut Target<'_>, q: &Quad, clip: &PixelRect) {
    let area = PixelRect::covering(&q.paint_bounds()).intersect(clip);
    if area.is_empty() {
        return;
    }
    let b = q.bounds;
    let (hw, hh) = (b.w / 2.0, b.h / 2.0);
    let (cx, cy) = (b.x + hw, b.y + hh);
    let background = q.background.to_array();
    let border = q.border_color.to_array();
    // Pixels whose centres lie this far inside the bounds are plain fill.
    let inner = if q.blur > 0.0 {
        PixelRect {
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
        }
    } else {
        let inset = q.corner_radius.max(0.0) + q.border_width.max(0.0) + 1.0;
        PixelRect {
            x0: (b.x + inset).ceil() as i32,
            y0: (b.y + inset).ceil() as i32,
            x1: (b.right() - inset).floor() as i32,
            y1: (b.bottom() - inset).floor() as i32,
        }
    };
    let fill_a = q.background.a;
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let (fr, fg, fb, fa) = (
        c(background[0] * fill_a),
        c(background[1] * fill_a),
        c(background[2] * fill_a),
        c(fill_a),
    );
    for y in area.y0..area.y1 {
        let py = y as f32 + 0.5 - cy;
        let fast = y >= inner.y0 && y < inner.y1;
        let (fx0, fx1) = if fast {
            (inner.x0.max(area.x0), inner.x1.min(area.x1))
        } else {
            (0, 0)
        };
        let mut x = area.x0;
        while x < area.x1 {
            if fast && x == fx0 && fx0 < fx1 {
                for xx in fx0..fx1 {
                    let p = target.at(xx, y);
                    *p = blend(*p, fr, fg, fb, fa);
                }
                x = fx1;
                continue;
            }
            let px = x as f32 + 0.5 - cx;
            let src = quad_pixel(
                px,
                py,
                hw,
                hh,
                q.corner_radius,
                q.border_width,
                q.blur,
                background,
                border,
            );
            if src[3] > 0.0 {
                let p = target.at(x, y);
                *p = blend_f(*p, src);
            }
            x += 1;
        }
    }
}

fn draw_glyph(
    target: &mut Target<'_>,
    image: &GlyphImage,
    x: i32,
    y: i32,
    color: Color,
    clip: &PixelRect,
) {
    let bounds = PixelRect::covering(&image.bounds_at(x, y));
    let area = bounds.intersect(clip);
    if area.is_empty() {
        return;
    }
    let w = image.width as usize;
    let ca = (color.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let (cr, cg, cb) = (
        (color.r.clamp(0.0, 1.0) * 255.0 + 0.5) as u32,
        (color.g.clamp(0.0, 1.0) * 255.0 + 0.5) as u32,
        (color.b.clamp(0.0, 1.0) * 255.0 + 0.5) as u32,
    );
    for py in area.y0..area.y1 {
        let row = (py - bounds.y0) as usize * w;
        for px in area.x0..area.x1 {
            let i = row + (px - bounds.x0) as usize;
            let p = target.at(px, py);
            if image.color {
                let s = &image.data[i * 4..i * 4 + 4];
                let a = div255(s[3] as u32 * ca);
                *p = blend(
                    *p,
                    div255(s[0] as u32 * a),
                    div255(s[1] as u32 * a),
                    div255(s[2] as u32 * a),
                    a,
                );
            } else {
                let a = div255(image.data[i] as u32 * ca);
                *p = blend(*p, div255(cr * a), div255(cg * a), div255(cb * a), a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Quad;

    fn quads(scene: &mut Scene, offset: i32) {
        scene.push_scroll_layer(
            1,
            Rect::new(0.0, 10.0, 100.0, 80.0),
            offset,
            Color::rgb(0xffffff),
        );
        for row in 0..50 {
            let y = 10.0 + row as f32 * 20.0 - offset as f32;
            let colour = if row % 2 == 0 { 0x3366cc } else { 0xcc6633 };
            scene.quad(Quad::filled(
                Rect::new(5.0, y + 2.0, 90.0, 16.0),
                Color::rgb(colour),
            ));
        }
        scene.pop_scroll_layer();
    }

    fn render(offset: i32) -> Vec<u32> {
        let mut scene = Scene::new(100, 100, Color::rgb(0x202020));
        quads(&mut scene, offset);
        let mut canvas = Canvas::new();
        canvas.draw(&scene, &GlyphCache::default());
        canvas.pixels().to_vec()
    }

    #[test]
    fn scrolling_moves_pixels_and_matches_a_full_redraw() {
        let glyphs = GlyphCache::default();
        let mut canvas = Canvas::new();
        let mut scene = Scene::new(100, 100, Color::rgb(0x202020));
        for offset in [0, 7, 30, 29, 100, 0] {
            scene.clear(100, 100, Color::rgb(0x202020));
            quads(&mut scene, offset);
            let stats = canvas.draw(&scene, &glyphs);
            assert_eq!(
                canvas.pixels(),
                render(offset).as_slice(),
                "offset {offset}"
            );
            if offset == 7 {
                assert!(!stats.full);
                assert!(stats.moved_rows > 0);
                assert!(
                    stats.pixels < 100 * 100 / 2,
                    "redrew {} pixels",
                    stats.pixels
                );
            }
        }
    }

    #[test]
    fn damage_does_not_reach_back_across_a_resize() {
        let glyphs = GlyphCache::default();
        let mut canvas = Canvas::new();
        for offset in [0, 7, 30] {
            let mut scene = Scene::new(100, 100, Color::rgb(0x202020));
            quads(&mut scene, offset);
            canvas.draw(&scene, &glyphs);
        }
        let mut scene = Scene::new(50, 100, Color::rgb(0x202020));
        quads(&mut scene, 30);
        canvas.draw(&scene, &glyphs);
        assert_eq!(canvas.damage_since(2), None);
        let full = canvas.damage_since(1).expect("the frame after the resize");
        assert!(full.iter().all(|r| r.x1 <= 50 && r.y1 <= 100));
    }

    #[test]
    fn an_unchanged_frame_redraws_nothing() {
        let glyphs = GlyphCache::default();
        let mut canvas = Canvas::new();
        let mut scene = Scene::new(100, 100, Color::rgb(0x202020));
        quads(&mut scene, 3);
        canvas.draw(&scene, &glyphs);
        let stats = canvas.draw(&scene, &glyphs);
        assert_eq!(stats.pixels, 0);
    }

    #[test]
    fn a_window_item_over_a_scrolled_layer_is_redrawn_in_place() {
        let glyphs = GlyphCache::default();
        let draw = |canvas: &mut Canvas, offset: i32| {
            let mut scene = Scene::new(100, 100, Color::rgb(0x202020));
            quads(&mut scene, offset);
            scene.quad(Quad::filled(
                Rect::new(90.0, 20.0, 6.0, 30.0),
                Color::rgb(0x00ff00),
            ));
            canvas.draw(&scene, &glyphs);
        };
        let mut canvas = Canvas::new();
        draw(&mut canvas, 0);
        draw(&mut canvas, 13);
        let mut fresh = Canvas::new();
        draw(&mut fresh, 13);
        assert_eq!(canvas.pixels(), fresh.pixels());
    }
}
