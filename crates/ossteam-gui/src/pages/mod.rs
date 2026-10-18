// SPDX-License-Identifier: GPL-2.0-or-later
//! The pages of `gui.md` §4, and what they share.

pub mod app;
pub mod downloads;
pub mod library;
pub mod login;
pub mod settings;

use gpui::{App, Div, SharedString, div, prelude::*};

use crate::theme::palette;

/// A page section: a heading over its content.
pub fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    let p = palette(cx);
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(p.muted)
            .child(title.into()),
    )
}

/// A label and its value on one line.
pub fn row(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .gap_3()
        .text_sm()
        .child(
            div()
                .w_32()
                .flex_none()
                .text_color(p.muted)
                .child(label.into()),
        )
        .child(div().flex_1().child(value.into()))
}

pub fn error_text(message: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(palette(cx).error)
        .child(message.into())
}

pub fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(palette(cx).muted)
        .child(text.into())
}

/// A bordered box for a group of controls.
pub fn panel(cx: &App) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
}
