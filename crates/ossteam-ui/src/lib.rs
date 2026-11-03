// SPDX-License-Identifier: GPL-2.0-or-later
//! ossteam's GUI toolkit (SPEC GUI-1, ADR 0004): windows and input from
//! winit, layout into a [`scene::Scene`], and two renderers that draw it,
//! Vulkan through `ash` and our own software renderer.
//!
//! This is the spike of ADR 0004 step 2: the scene, text, both renderers
//! and the appearance query, enough for `examples/spike.rs` to measure
//! them. Elements, layout, focus and the event loop come in step 3.

pub mod appearance;
pub mod render;
pub mod scene;
pub mod text;

/// An error from the toolkit, with its context in the message.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
