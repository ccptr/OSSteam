// SPDX-License-Identifier: GPL-2.0-or-later
//! The M0 gpui spike (SPEC §21 M0): opens a window, then reports the time
//! to the first presented frame, the display scale factor and appearance,
//! and whether a clipboard round trip works. Prints one JSON line and
//! exits, so it can run unattended on each platform:
//!
//! ```sh
//! cargo run --release -p ossteam-gui --example spike
//! ```

use std::time::Instant;

use gpui::{
    App, Bounds, ClipboardItem, Context, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    size,
};

struct Spike;

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("ossteam gpui spike")
    }
}

fn main() {
    let started = Instant::now();
    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.), px(320.)), cx);
        let window = cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| cx.new(|_| Spike),
        );
        let window = match window {
            Ok(w) => w,
            Err(e) => {
                println!("{{\"window\":false,\"error\":{:?}}}", format!("{e:#}"));
                cx.quit();
                return;
            }
        };
        let _ = window.update(cx, |_, window, _| {
            window.on_next_frame(move |window, cx| {
                let first_frame_ms = started.elapsed().as_secs_f64() * 1000.0;
                let probe = format!("ossteam-spike-{}", std::process::id());
                cx.write_to_clipboard(ClipboardItem::new_string(probe.clone()));
                let clipboard = cx.read_from_clipboard().and_then(|c| c.text()).as_deref() == Some(probe.as_str());
                println!(
                    "{{\"window\":true,\"os\":{:?},\"first_frame_ms\":{first_frame_ms:.1},\"scale_factor\":{},\"appearance\":{:?},\"clipboard_round_trip\":{clipboard}}}",
                    std::env::consts::OS,
                    window.scale_factor(),
                    format!("{:?}", window.appearance()),
                );
                cx.quit();
            });
        });
    });
}
