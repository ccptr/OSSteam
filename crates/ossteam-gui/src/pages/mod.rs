// SPDX-License-Identifier: GPL-2.0-or-later
//! The pages of `gui.md` §4. Each keeps its view state in a struct the
//! [`Gui`](crate::gui::Gui) holds, and draws with a `view` function of
//! the GUI, the model and the palette.

pub mod app;
pub mod downloads;
pub mod library;
pub mod login;
pub mod settings;
