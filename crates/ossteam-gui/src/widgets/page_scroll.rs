// SPDX-License-Identifier: GPL-2.0-or-later
//! Page Up, Page Down, Home and End scroll a page that scrolls (GUI-4).
//! Tab shows each control, but what comes after the last one, such as
//! the App page's last run, is seen only by scrolling. A text field and
//! the library grid keep these keys for themselves: their key contexts
//! are deeper than the page's.

use gpui::{
    App, Div, KeyBinding, KeyContext, Pixels, ScrollHandle, Stateful, Window, actions, prelude::*,
    px,
};

actions!(page_scroll, [PageUp, PageDown, Top, Bottom]);

const CONTEXT: &str = "ScrollingPage";
/// What a step of a page keeps in sight of the view it leaves.
const OVERLAP: Pixels = px(48.);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("pageup", PageUp, ctx),
        KeyBinding::new("pagedown", PageDown, ctx),
        KeyBinding::new("home", Top, ctx),
        KeyBinding::new("end", Bottom, ctx),
    ]);
}

/// Makes `d`, the element that scrolls with `scroll`, scroll by
/// keyboard. `context` is the page's own key context, if it has one.
pub fn by_keyboard(
    d: Stateful<Div>,
    scroll: &ScrollHandle,
    context: Option<&'static str>,
) -> Stateful<Div> {
    let mut keys = KeyContext::default();
    keys.add(CONTEXT);
    if let Some(c) = context {
        keys.add(c);
    }
    let on = |scroll: &ScrollHandle, to: fn(Pixels, Pixels, Pixels) -> Pixels| {
        let scroll = scroll.clone();
        move |window: &mut Window| {
            let mut offset = scroll.offset();
            offset.y = to(offset.y, scroll.max_offset().y, scroll.bounds().size.height);
            scroll.set_offset(offset);
            window.refresh();
        }
    };
    let (up, down, top, bottom) = (
        on(scroll, page_up),
        on(scroll, page_down),
        on(scroll, |_, _, _| px(0.)),
        on(scroll, |_, max, _| -max),
    );
    d.key_context(keys)
        .on_action(move |_: &PageUp, window, _| up(window))
        .on_action(move |_: &PageDown, window, _| down(window))
        .on_action(move |_: &Top, window, _| top(window))
        .on_action(move |_: &Bottom, window, _| bottom(window))
}

/// The offset a view of `height` at `y` (0 at the top, `-max` at the
/// bottom) moves to a page up.
fn page_up(y: Pixels, _max: Pixels, height: Pixels) -> Pixels {
    (y + step(height)).min(px(0.))
}

fn page_down(y: Pixels, max: Pixels, height: Pixels) -> Pixels {
    (y - step(height)).max(-max)
}

fn step(height: Pixels) -> Pixels {
    (height - OVERLAP).max(OVERLAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(y: f32, max: f32, height: f32) -> f32 {
        f32::from(page_down(px(y), px(max), px(height)))
    }

    fn up(y: f32, max: f32, height: f32) -> f32 {
        f32::from(page_up(px(y), px(max), px(height)))
    }

    #[test]
    fn a_page_keeps_some_of_the_view_it_leaves() {
        assert_eq!(down(0., 1000., 400.), -352.);
        assert_eq!(up(-500., 1000., 400.), -148.);
    }

    #[test]
    fn a_page_stops_at_either_end() {
        assert_eq!(down(-900., 1000., 400.), -1000.);
        assert_eq!(down(0., 0., 400.), 0.);
        assert_eq!(up(-100., 1000., 400.), 0.);
    }

    #[test]
    fn a_view_shorter_than_the_overlap_still_moves() {
        assert_eq!(down(0., 1000., 20.), -48.);
    }
}
