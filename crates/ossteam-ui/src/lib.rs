// SPDX-License-Identifier: GPL-2.0-or-later
//! ossteam's GUI toolkit (SPEC GUI-1, ADR 0004): windows and input from
//! winit, layout into a [`scene::Scene`], and two renderers that draw it,
//! Vulkan through `ash` and our own software renderer.
//!
//! A view is a function from the application's state to an
//! [`element::Element`] tree (`element.rs`, `style.rs`); a
//! [`window::Window`] lays it out with `taffy`, paints it into a scene and
//! dispatches input, focus, key bindings (`keys.rs`) and timers to the
//! handlers in it. `examples/spike.rs` is ADR 0004's spike, which measured
//! the renderers, text and input before the toolkit was built on them.

pub mod app;
pub mod appearance;
pub mod clipboard;
pub mod element;
pub mod folder_picker;
mod fonts;
pub mod image;
pub mod keys;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod portal;
pub mod render;
pub mod scene;
pub mod style;
pub mod text;
pub mod text_field;
pub mod window;

/// An error from the toolkit, with its context in the message.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
