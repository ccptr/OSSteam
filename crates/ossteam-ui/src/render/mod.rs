// SPDX-License-Identifier: GPL-2.0-or-later
//! The two renderers and how one is chosen (ADR 0004, "Choosing").

pub mod shape;
pub mod software;
pub mod vulkan;

use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use winit::window::Window;

use crate::Error;
use crate::scene::Scene;
use crate::text::GlyphCache;

/// `--renderer auto|vulkan|software`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Choice {
    /// Vulkan, or the software renderer when no Vulkan device can be made.
    #[default]
    Auto,
    Vulkan,
    Software,
}

impl FromStr for Choice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Choice::Auto),
            "vulkan" => Ok(Choice::Vulkan),
            "software" => Ok(Choice::Software),
            other => Err(format!(
                "unknown renderer {other:?}: expected auto, vulkan or software"
            )),
        }
    }
}

/// What a frame cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    /// From the start of drawing to the return of present.
    pub ms: f64,
    /// Of `ms`, how long was spent waiting for an earlier frame to finish
    /// on the GPU or for the display to take one: how FIFO paces the
    /// Vulkan renderer. Acquiring and presenting count as waiting.
    pub wait_ms: f64,
    /// Pixels the software renderer redrew; 0 with Vulkan, which redraws all.
    pub pixels: i64,
    /// Nothing changed, so nothing was presented.
    pub skipped: bool,
}

pub enum Renderer {
    Vulkan(Box<vulkan::VulkanRenderer>),
    Software(SoftwarePresenter),
}

impl Renderer {
    /// Makes the renderer `choice` asks for, and says why it was chosen.
    pub fn new(window: Arc<Window>, choice: Choice) -> Result<(Renderer, String), Error> {
        match choice {
            Choice::Software => Ok((
                Renderer::Software(SoftwarePresenter::new(window)?),
                "--renderer software".into(),
            )),
            Choice::Vulkan => Ok((
                Renderer::Vulkan(Box::new(vulkan::VulkanRenderer::new(window)?)),
                "--renderer vulkan".into(),
            )),
            Choice::Auto => match vulkan::VulkanRenderer::new(window.clone()) {
                Ok(r) => Ok((
                    Renderer::Vulkan(Box::new(r)),
                    "auto: a Vulkan device was found".into(),
                )),
                Err(e) => {
                    let why = format!("auto: no Vulkan device ({e})");
                    tracing::warn!("{why}; using the software renderer");
                    Ok((Renderer::Software(SoftwarePresenter::new(window)?), why))
                }
            },
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Renderer::Vulkan(_) => "vulkan",
            Renderer::Software(_) => "software",
        }
    }

    /// Draws and presents the scene. A Vulkan renderer that fails is
    /// replaced by the software renderer for the rest of the session.
    pub fn draw(&mut self, scene: &Scene, glyphs: &GlyphCache) -> Result<FrameStats, Error> {
        match self {
            Renderer::Software(s) => s.draw(scene, glyphs),
            Renderer::Vulkan(v) => match v.draw(scene, glyphs) {
                Ok(stats) => Ok(stats),
                Err(e) => {
                    tracing::error!(
                        "the Vulkan renderer failed ({e}); using the software renderer"
                    );
                    let window = v.window().clone();
                    *self = Renderer::Software(SoftwarePresenter::new(window)?);
                    self.draw(scene, glyphs)
                }
            },
        }
    }
}

/// The software renderer's canvas, presented with softbuffer.
pub struct SoftwarePresenter {
    window: Arc<Window>,
    // Declared before the context it was made from, so it drops first.
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    _context: softbuffer::Context<Arc<Window>>,
    canvas: software::Canvas,
    size: (u32, u32),
}

impl SoftwarePresenter {
    pub fn new(window: Arc<Window>) -> Result<Self, Error> {
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| format!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| format!("softbuffer surface: {e}"))?;
        Ok(Self {
            window,
            surface,
            _context: context,
            canvas: software::Canvas::new(),
            size: (0, 0),
        })
    }

    pub fn canvas(&self) -> &software::Canvas {
        &self.canvas
    }

    fn draw(&mut self, scene: &Scene, glyphs: &GlyphCache) -> Result<FrameStats, Error> {
        let started = Instant::now();
        let (Some(w), Some(h)) = (NonZeroU32::new(scene.width), NonZeroU32::new(scene.height))
        else {
            return Ok(FrameStats {
                skipped: true,
                ..Default::default()
            });
        };
        let drawn = self.canvas.draw(scene, glyphs);
        let drawn_at = started.elapsed();
        if self.canvas.last_damage().is_empty() && (scene.width, scene.height) == self.size {
            return Ok(FrameStats {
                skipped: true,
                ..Default::default()
            });
        }
        if (scene.width, scene.height) != self.size {
            self.surface
                .resize(w, h)
                .map_err(|e| format!("softbuffer resize: {e}"))?;
            self.size = (scene.width, scene.height);
        }
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| format!("softbuffer buffer: {e}"))?;
        let buffer_at = started.elapsed();
        let age = buffer.age() as usize;
        let pixels = self.canvas.pixels();
        let width = scene.width as usize;
        // A buffer drawn `age` frames ago needs the damage since then.
        match self.canvas.damage_since(age) {
            Some(rects) if age > 0 => {
                for r in rects {
                    for y in r.y0..r.y1 {
                        let row = y as usize * width;
                        let span = row + r.x0 as usize..row + r.x1 as usize;
                        buffer[span.clone()].copy_from_slice(&pixels[span]);
                    }
                }
            }
            _ => buffer.copy_from_slice(pixels),
        }
        let damage: Vec<softbuffer::Rect> = self
            .canvas
            .last_damage()
            .iter()
            .filter_map(|r| {
                Some(softbuffer::Rect {
                    x: r.x0 as u32,
                    y: r.y0 as u32,
                    width: NonZeroU32::new(r.width() as u32)?,
                    height: NonZeroU32::new(r.height() as u32)?,
                })
            })
            .collect();
        let copied_at = started.elapsed();
        self.window.pre_present_notify();
        buffer
            .present_with_damage(&damage)
            .map_err(|e| format!("softbuffer present: {e}"))?;
        tracing::debug!(
            draw = ?drawn_at,
            buffer = ?(buffer_at - drawn_at),
            copy = ?(copied_at - buffer_at),
            present = ?(started.elapsed() - copied_at),
            age,
            pixels = drawn.pixels,
            "software frame"
        );
        Ok(FrameStats {
            ms: started.elapsed().as_secs_f64() * 1000.0,
            pixels: drawn.pixels,
            ..Default::default()
        })
    }
}
