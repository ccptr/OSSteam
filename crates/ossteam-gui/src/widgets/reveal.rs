// SPDX-License-Identifier: GPL-2.0-or-later
//! Scrolls the control that Tab or Shift-Tab focused into view (GUI-4).
//! gpui moves focus without scrolling, so a control below the fold of a
//! page could have focus, and its focus ring, out of sight.
//!
//! A focus move by keyboard is marked; the control that then has focus
//! records where it was laid out; on the next frame the page that holds
//! it scrolls by the least that shows it. A message that a control's
//! action brings up, such as an error, is shown the same way.

use gpui::{
    App, Bounds, Div, ElementId, FocusHandle, Global, Pixels, ScrollHandle, SharedString, Stateful,
    Window, canvas, prelude::*, px,
};

/// Space kept between the control and the page's edge.
const MARGIN: Pixels = px(16.);

#[derive(Default)]
struct Reveal {
    /// Focus moved by keyboard, and the control has not been laid out.
    pending: bool,
    /// The control focused so, and where it was.
    target: Option<(FocusHandle, Bounds<Pixels>)>,
}

impl Global for Reveal {}

/// Called after Tab or Shift-Tab moved focus.
pub fn focus_moved(window: &mut Window, cx: &mut App) {
    *cx.default_global::<Reveal>() = Reveal {
        pending: true,
        target: None,
    };
    window.refresh();
}

/// Makes `d` a tab stop whose position is recorded when it gets focus
/// by keyboard; a disabled one is skipped by Tab. `key` must be unique
/// among its siblings, like an id. The focus handle lives as long as the
/// element is drawn, enabled or not, so that a control disabled while
/// its own action runs still has focus afterwards.
pub fn tab_stop(
    d: Stateful<Div>,
    key: impl Into<ElementId>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let focus = window
        .use_keyed_state(key, cx, |_, cx| {
            cx.focus_handle().tab_index(0).tab_stop(true)
        })
        .read(cx)
        .clone();
    tab_stop_with(d, focus, enabled)
}

/// [`tab_stop`] with a focus handle its owner keeps, to focus it from
/// outside.
pub fn tab_stop_with(d: Stateful<Div>, focus: FocusHandle, enabled: bool) -> Stateful<Div> {
    if enabled {
        d.track_focus(&focus).relative().child(marker(focus))
    } else {
        d
    }
}

/// A child that fills a focusable element and records where it is when
/// focus has just moved to `focus` by keyboard.
pub fn marker(focus: FocusHandle) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let Some(r) = cx.try_global::<Reveal>() else {
                return;
            };
            if r.pending && focus.is_focused(window) {
                cx.set_global(Reveal {
                    pending: false,
                    target: Some((focus, bounds)),
                });
                // A refresh while drawing is ignored; the page scrolls in
                // the frame after this one.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A child that fills a message and records where it is the first frame
/// it is drawn, so that it scrolls into view while focus is on the same
/// page: the control whose action brought it up keeps focus, and the
/// message can be below it, out of sight. `key` tells messages apart.
pub fn when_new(key: impl Into<SharedString>) -> impl IntoElement {
    let key = ElementId::Name(key.into());
    canvas(
        move |bounds, window, cx| {
            let new = window.with_global_id(key, |id, window| {
                window.with_element_state(id, |seen: Option<()>, _| (seen.is_none(), ()))
            });
            if let Some(focus) = window.focused(cx).filter(|_| new) {
                cx.set_global(Reveal {
                    pending: false,
                    target: Some((focus, bounds)),
                });
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// For a page that scrolls, while it renders: when the control recorded
/// still has focus and is on this page (`page` contains it), scrolls
/// `scroll` to show it.
pub fn scroll_to_focus(page: &FocusHandle, scroll: &ScrollHandle, window: &Window, cx: &mut App) {
    let Some(r) = cx.try_global::<Reveal>() else {
        return;
    };
    let Some((focus, target)) = &r.target else {
        return;
    };
    if !focus.is_focused(window) || !page.contains_focused(window, cx) {
        return;
    }
    let view = scroll.bounds();
    let down = distance(view.top(), view.bottom(), target.top(), target.bottom());
    cx.global_mut::<Reveal>().target = None;
    if down != px(0.) {
        let mut offset = scroll.offset();
        // The page lays out after this, and gpui keeps the offset within
        // its content then.
        offset.y -= down;
        scroll.set_offset(offset);
    }
}

/// How far to scroll down (up when negative) so that `top..bottom` is in
/// `view_top..view_bottom` with `MARGIN` around it; a control taller than
/// the view is shown from its top.
fn distance(view_top: Pixels, view_bottom: Pixels, top: Pixels, bottom: Pixels) -> Pixels {
    let (top, bottom) = (top - MARGIN, bottom + MARGIN);
    if top < view_top {
        top - view_top
    } else if bottom > view_bottom {
        (bottom - view_bottom).min(top - view_top)
    } else {
        px(0.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(view: (f32, f32), target: (f32, f32)) -> f32 {
        f32::from(distance(px(view.0), px(view.1), px(target.0), px(target.1)))
    }

    #[test]
    fn a_control_in_view_stays() {
        assert_eq!(d((100., 500.), (200., 230.)), 0.);
        assert_eq!(d((100., 500.), (116., 484.)), 0.);
    }

    #[test]
    fn a_control_below_scrolls_down_to_its_bottom() {
        assert_eq!(d((100., 500.), (600., 630.)), 146.);
        assert_eq!(d((100., 500.), (480., 500.)), 16.);
    }

    #[test]
    fn a_control_above_scrolls_up_to_its_top() {
        assert_eq!(d((100., 500.), (40., 70.)), -76.);
        assert_eq!(d((100., 500.), (110., 130.)), -6.);
    }

    #[test]
    fn a_control_taller_than_the_view_shows_its_top() {
        assert_eq!(d((100., 500.), (700., 1400.)), 584.);
    }
}
