// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam-gui`, the graphical client (SPEC §16, `docs/spec/gui.md`). It
//! renders what the daemon reports and keeps no state of its own beyond
//! the view (GUI-2).
//!
//! The binary only calls [`run`]. The connection, the store and the model
//! are public for the daemon's tests, which run them against a test
//! daemon on gpui's test platform (`gui.md` §7).

mod args;
mod bench;
pub mod daemon;
mod format;
pub mod model;
mod pages;
mod queue;
mod rate;
mod root;
pub mod store;
mod theme;
mod thumbnails;
mod widgets;

use std::time::Instant;

use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

/// Opens the window and runs until it closes.
pub fn run() {
    let started = Instant::now();
    let autostart = !std::env::args().skip(1).any(|a| a == "--no-autostart");
    gpui_platform::application().run(move |cx: &mut App| {
        root::bind_keys(cx);
        widgets::text_input::bind_keys(cx);
        pages::library::bind_keys(cx);
        pages::app::bind_keys(cx);
        widgets::page_scroll::bind_keys(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("ossteam".into()),
                    ..Default::default()
                }),
                app_id: Some("ossteam".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| root::Root::new(started, autostart, window, cx)),
        );
        if let Err(e) = opened {
            eprintln!("ossteam-gui: cannot open a window: {e:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
