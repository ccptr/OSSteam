// SPDX-License-Identifier: GPL-2.0-or-later
//! The M0 gpui spike (SPEC §21 M0): opens a window, then reports the time
//! to the first presented frame, the display scale factor and appearance,
//! and whether a clipboard round trip works. Prints one JSON line and
//! exits, so it can run unattended on each platform:
//!
//! ```sh
//! cargo run --release -p ossteam-gui --example spike
//! ```
//!
//! The clipboard is tried at the first frame, again once the window is
//! active, and on the first key press, read back 100 ms later. Wayland
//! lets a client set the selection only with the serial of a key or
//! pointer press, so on Wayland only the last can work; press a key while
//! the spike waits (`wtype a` does). With `OSSTEAM_SPIKE_CLIPBOARD` set,
//! the spike skips the first two writes and reports whether the clipboard
//! held that text at the key press, as put there by another client
//! (`wl-copy`, `xclip`).
//! Each frame drawn and each appearance change is logged with its time,
//! so a frame drawn with the wrong appearance shows up (GUI-4).

use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, Context, FocusHandle, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};

/// How long the spike waits for the window to become active, and for a
/// late appearance, before it reports.
const SETTLE: Duration = Duration::from_secs(3);

struct Spike {
    started: Instant,
    focus: FocusHandle,
    /// `(ms, "appearance@scale factor")` for each frame drawn.
    frames: Vec<(f64, String)>,
    /// `(ms, appearance)` for each appearance change.
    appearance_changes: Vec<(f64, String)>,
    first_frame_ms: Option<f64>,
    first_frame_clipboard: Option<bool>,
    /// `(ms, round trip worked)` once the window was active.
    active_clipboard: Option<(f64, bool)>,
    /// `(ms, round trip worked)` after the first key press.
    key_clipboard: Option<(f64, bool)>,
    key_pressed: bool,
    /// Whether the clipboard held `OSSTEAM_SPIKE_CLIPBOARD` at the first key press.
    external_clipboard: Option<bool>,
}

impl Spike {
    fn ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ms = self.ms();
        self.frames.push((ms, format!("{:?}@{}", window.appearance(), window.scale_factor())));
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|spike, _, _, cx| {
                if std::mem::replace(&mut spike.key_pressed, true) {
                    return;
                }
                spike.external_clipboard = std::env::var("OSSTEAM_SPIKE_CLIPBOARD").ok().map(|text| {
                    cx.read_from_clipboard().and_then(|c| c.text()).as_deref() == Some(text.as_str())
                });
                let probe = format!("ossteam-spike-key-{}", std::process::id());
                cx.write_to_clipboard(ClipboardItem::new_string(probe.clone()));
                cx.spawn(async move |spike, cx| {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let _ = spike.update(cx, |spike, cx| {
                        let ok = cx.read_from_clipboard().and_then(|c| c.text()).as_deref()
                            == Some(probe.as_str());
                        spike.key_clipboard = Some((spike.ms(), ok));
                    });
                })
                .detach();
            }))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("ossteam gpui spike")
    }
}

fn clipboard_round_trip(cx: &mut App, probe: String) -> bool {
    cx.write_to_clipboard(ClipboardItem::new_string(probe.clone()));
    cx.read_from_clipboard().and_then(|c| c.text()).as_deref() == Some(probe.as_str())
}

fn main() {
    let started = Instant::now();
    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.), px(320.)), cx);
        let window = cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |window, cx| {
                cx.new(|cx| {
                    cx.observe_window_appearance(window, |spike: &mut Spike, window, _| {
                        let ms = spike.ms();
                        spike.appearance_changes.push((ms, format!("{:?}", window.appearance())));
                    })
                    .detach();
                    cx.observe_window_activation(window, |spike: &mut Spike, window, cx| {
                        if window.is_window_active()
                            && spike.active_clipboard.is_none()
                            && std::env::var_os("OSSTEAM_SPIKE_CLIPBOARD").is_none()
                        {
                            let probe = format!("ossteam-spike-active-{}", std::process::id());
                            let ok = clipboard_round_trip(cx, probe);
                            spike.active_clipboard = Some((spike.ms(), ok));
                        }
                    })
                    .detach();
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Spike {
                        started,
                        focus,
                        frames: Vec::new(),
                        appearance_changes: Vec::new(),
                        first_frame_ms: None,
                        first_frame_clipboard: None,
                        active_clipboard: None,
                        key_clipboard: None,
                        key_pressed: false,
                        external_clipboard: None,
                    }
                })
            },
        );
        let window = match window {
            Ok(w) => w,
            Err(e) => {
                println!("{{\"window\":false,\"error\":{:?}}}", format!("{e:#}"));
                cx.quit();
                return;
            }
        };
        let _ = window.update(cx, |_, window, cx| {
            let view = cx.entity();
            window.on_next_frame(move |window, cx| {
                let ok = std::env::var_os("OSSTEAM_SPIKE_CLIPBOARD").is_none().then(|| {
                    clipboard_round_trip(cx, format!("ossteam-spike-{}", std::process::id()))
                });
                view.update(cx, |spike, _| {
                    spike.first_frame_ms = Some(spike.ms());
                    spike.first_frame_clipboard = ok;
                });
                let first_frame_appearance = format!("{:?}", window.appearance());
                let scale_factor = window.scale_factor();
                let view = view.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(SETTLE).await;
                    view.update(cx, |spike, cx| {
                        let pairs = |log: &[(f64, String)]| {
                            log.iter()
                                .map(|(ms, a)| format!("[{ms:.1},{a:?}]"))
                                .collect::<Vec<_>>()
                                .join(",")
                        };
                        let timed = |result: Option<(f64, bool)>| match result {
                            Some((ms, ok)) => format!("{{\"ms\":{ms:.1},\"ok\":{ok}}}"),
                            None => "null".to_owned(),
                        };
                        let flag = |result: Option<bool>| match result {
                            Some(ok) => ok.to_string(),
                            None => "null".to_owned(),
                        };
                        println!(
                            "{{\"window\":true,\"os\":{:?},\"first_frame_ms\":{:.1},\"scale_factor\":{scale_factor},\"appearance\":{first_frame_appearance:?},\"clipboard_round_trip\":{},\"clipboard_when_active\":{},\"clipboard_after_key\":{},\"clipboard_from_other_client\":{},\"appearance_changes\":[{}],\"frames\":[{}]}}",
                            std::env::consts::OS,
                            spike.first_frame_ms.unwrap_or_default(),
                            flag(spike.first_frame_clipboard),
                            timed(spike.active_clipboard),
                            timed(spike.key_clipboard),
                            flag(spike.external_clipboard),
                            pairs(&spike.appearance_changes),
                            pairs(&spike.frames),
                        );
                        cx.quit();
                    });
                })
                .detach();
            });
        });
    });
}
