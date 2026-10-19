// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam-gui`, the graphical client (SPEC §16, `docs/spec/gui.md`). It
//! renders what the daemon reports and keeps no state of its own beyond
//! the view (GUI-2).

mod args;
mod bench;
mod daemon;
mod format;
mod model;
mod pages;
mod queue;
mod rate;
mod root;
mod store;
mod theme;
mod thumbnails;
mod widgets;

use std::time::Instant;

use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    let started = Instant::now();
    let autostart = !std::env::args().skip(1).any(|a| a == "--no-autostart");
    gpui_platform::application().run(move |cx: &mut App| {
        root::bind_keys(cx);
        widgets::text_input::bind_keys(cx);
        pages::library::bind_keys(cx);
        pages::app::bind_keys(cx);
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
