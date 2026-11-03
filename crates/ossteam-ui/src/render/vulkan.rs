// SPDX-License-Identifier: GPL-2.0-or-later
//! The Vulkan renderer: written next.

use std::sync::Arc;

use winit::window::Window;

use crate::Error;
use crate::render::FrameStats;
use crate::scene::Scene;
use crate::text::GlyphCache;

pub struct VulkanRenderer {
    window: Arc<Window>,
}

impl VulkanRenderer {
    pub fn new(_window: Arc<Window>) -> Result<Self, Error> {
        Err("the Vulkan renderer is not written yet".into())
    }

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    pub fn draw(&mut self, _scene: &Scene, _glyphs: &GlyphCache) -> Result<FrameStats, Error> {
        Err("the Vulkan renderer is not written yet".into())
    }
}
