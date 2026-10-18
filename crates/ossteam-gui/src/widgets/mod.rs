// SPDX-License-Identifier: GPL-2.0-or-later
//! Widgets built on plain gpui (SPEC GUI-1).

mod button;
mod controls;
pub mod qr;
pub mod text_input;

pub use button::button;
pub use controls::{choice, progress_bar, switch};
pub use text_input::{TextInput, TextInputEvent};
