// SPDX-License-Identifier: GPL-2.0-or-later
//! A push button. It is a tab stop, and Enter or Space on it clicks it
//! (gpui turns those keys into clicks on the focused element), so every
//! button is reachable from the keyboard (GUI-4).

use std::rc::Rc;

use gpui::{App, ClickEvent, ElementId, SharedString, Window, div, prelude::*};

use crate::theme::palette;
use crate::widgets::reveal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Primary,
    Secondary,
    Danger,
    /// No background until hovered; for tabs and menu items.
    Ghost,
}

type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    kind: Kind,
    disabled: bool,
    selected: bool,
    full_width: bool,
    on_click: Option<OnClick>,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button {
        id: id.into(),
        label: label.into(),
        kind: Kind::Primary,
        disabled: false,
        selected: false,
        full_width: false,
        on_click: None,
    }
}

impl Button {
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub fn secondary(self) -> Self {
        self.kind(Kind::Secondary)
    }

    pub fn danger(self) -> Self {
        self.kind(Kind::Danger)
    }

    pub fn ghost(self) -> Self {
        self.kind(Kind::Ghost)
    }

    /// Drawn greyed out, skipped by Tab, and not clickable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Marks the current tab or chosen item.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let (bg, fg, border) = match self.kind {
            Kind::Primary => (p.accent, p.on_accent, p.accent),
            Kind::Secondary => (p.surface_raised, p.text, p.border),
            Kind::Danger => (p.surface_raised, p.error, p.border),
            Kind::Ghost if self.selected => (p.selection, p.text, p.selection),
            Kind::Ghost => (gpui::transparent_black(), p.text, gpui::transparent_black()),
        };
        let hover_bg = match self.kind {
            Kind::Primary => p.accent.opacity(0.85),
            _ => p.border,
        };
        let ring = p.focus_ring;
        let clickable = !self.disabled && self.on_click.is_some();
        reveal::tab_stop(
            div().id(self.id.clone()),
            self.id,
            !self.disabled,
            window,
            cx,
        )
        .flex()
        .items_center()
        .justify_center()
        .when(self.full_width, |d| d.w_full())
        .px_3()
        .py_1()
        .rounded_md()
        .border_2()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .whitespace_nowrap()
        .focus_visible(move |s| s.border_color(ring))
        .when(self.disabled, |d| d.opacity(0.5))
        .when(clickable, |d| {
            let f = self.on_click.clone().expect("clickable");
            d.cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .on_click(move |e, w, cx| f(e, w, cx))
        })
        .child(self.label)
    }
}
