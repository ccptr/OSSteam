// SPDX-License-Identifier: GPL-2.0-or-later
//! Small controls: a switch, a choice in a list, and a progress bar.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, FocusHandle, SharedString, Window, div, prelude::*, px, relative,
};

use crate::theme::palette;
use crate::widgets::reveal;

type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// An on/off switch with its label. Clicking it, or Enter or Space while
/// it has focus, calls `on_toggle`; the switch shows the daemon's value,
/// so it changes when that does.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    label: SharedString,
    on: bool,
    on_toggle: Option<OnClick>,
    focus: Option<FocusHandle>,
}

pub fn switch(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Switch {
    Switch {
        id: id.into(),
        label: label.into(),
        on,
        on_toggle: None,
        focus: None,
    }
}

impl Switch {
    /// Takes focus through `focus`, which its owner keeps, instead of a
    /// handle of its own.
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_toggle(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let ring = p.focus_ring;
        let track = if self.on { p.accent } else { p.border };
        let d = div().id(self.id.clone());
        match self.focus {
            Some(focus) => reveal::tab_stop_with(d, focus, true, window),
            None => reveal::tab_stop(d, self.id, true, window, cx),
        }
        .flex()
        .items_center()
        .gap_2()
        .p_0p5()
        .rounded_md()
        .border_2()
        .border_color(gpui::transparent_black())
        .focus_visible(move |s| s.border_color(ring))
        .cursor_pointer()
        .when_some(self.on_toggle, |d, f| {
            d.on_click(move |e, w, cx| f(e, w, cx))
        })
        .child(
            div()
                .w(px(32.))
                .h(px(18.))
                .rounded_full()
                .bg(track)
                .p(px(2.))
                .flex()
                .when(self.on, |d| d.justify_end())
                .child(div().size(px(14.)).rounded_full().bg(p.surface)),
        )
        .child(div().text_sm().child(self.label))
        .child(
            div()
                .text_xs()
                .text_color(p.muted)
                .child(if self.on { "on" } else { "off" }),
        )
    }
}

/// One option of a list where one is chosen: a marker, a label and a
/// detail line.
#[derive(IntoElement)]
pub struct Choice {
    id: ElementId,
    label: SharedString,
    detail: Option<SharedString>,
    chosen: bool,
    disabled: bool,
    on_choose: Option<OnClick>,
}

pub fn choice(id: impl Into<ElementId>, label: impl Into<SharedString>, chosen: bool) -> Choice {
    Choice {
        id: id.into(),
        label: label.into(),
        detail: None,
        chosen,
        disabled: false,
        on_choose: None,
    }
}

impl Choice {
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_choose(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Choice {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let ring = p.focus_ring;
        let hover = p.surface_raised;
        let enabled = !self.disabled;
        reveal::tab_stop(div().id(self.id.clone()), self.id, enabled, window, cx)
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_2()
            .border_color(gpui::transparent_black())
            .when(self.chosen, |d| d.bg(p.selection))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .when(!self.chosen, |d| d.hover(move |s| s.bg(hover)))
            })
            .when(self.disabled, |d| d.opacity(0.5))
            .focus_visible(move |s| s.border_color(ring))
            .when_some(self.on_choose.filter(|_| enabled), |d, f| {
                d.on_click(move |e, w, cx| f(e, w, cx))
            })
            .child(
                div()
                    .mt(px(3.))
                    .size(px(12.))
                    .flex_none()
                    .rounded_full()
                    .border_2()
                    .border_color(if self.chosen { p.accent } else { p.muted })
                    .when(self.chosen, |d| d.bg(p.accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().child(self.label))
                    .when_some(self.detail, |d, detail| {
                        d.child(div().text_xs().text_color(p.muted).child(detail))
                    }),
            )
    }
}

/// A bar filled to `fraction` (0 to 1); `None` draws an empty track, for
/// progress not known yet.
pub fn progress_bar(fraction: Option<f32>, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div().w_full().h(px(6.)).rounded_full().bg(p.border).child(
        div()
            .h_full()
            .rounded_full()
            .bg(p.accent)
            .w(relative(fraction.unwrap_or(0.).clamp(0., 1.))),
    )
}
