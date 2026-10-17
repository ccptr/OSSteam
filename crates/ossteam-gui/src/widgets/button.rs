// SPDX-License-Identifier: GPL-2.0-or-later
//! A push button. Callers attach behaviour with `.on_click(...)`. Keyboard
//! activation goes through actions bound by the view that owns the button.

use gpui::{Div, ElementId, SharedString, Stateful, div, prelude::*};

use crate::theme::Palette;

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    palette: &Palette,
) -> Stateful<Div> {
    let hover_bg = palette.accent.opacity(0.85);
    div()
        .id(id)
        .px_4()
        .py_1p5()
        .rounded_md()
        .bg(palette.accent)
        .text_color(palette.on_accent)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(label.into())
}
