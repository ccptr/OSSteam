// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam-gui`, the graphical client (SPEC §16). It renders what the
//! daemon reports and keeps no state of its own beyond the view (GUI-2).

mod daemon;
mod theme;
mod widgets;

use std::time::Instant;

use gpui::{
    App, Bounds, Context, FocusHandle, KeyBinding, SharedString, Task, TitlebarOptions, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use ossteam_ipc::v1::{self, ConnectionState};

use crate::theme::Palette;

actions!(ossteam, [Retry, Quit]);

enum State {
    Connecting,
    Ready {
        status: Box<v1::GetStatusResponse>,
        version: v1::GetVersionResponse,
    },
    Unreachable(SharedString),
}

struct Root {
    state: State,
    focus: FocusHandle,
    retry_focus: FocusHandle,
    connection: Option<Task<()>>,
    started: Instant,
    first_frame_reported: bool,
}

impl Root {
    fn new(started: Instant, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let mut root = Self {
            state: State::Connecting,
            focus,
            retry_focus: cx.focus_handle(),
            connection: None,
            started,
            first_frame_reported: false,
        };
        root.connect(cx);
        root
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.state = State::Connecting;
        let updates = daemon::connect();
        self.connection = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let applied = this.update(cx, |root, cx| {
                    root.state = match update {
                        daemon::Update::Ready { status, version } => {
                            State::Ready { status, version }
                        }
                        daemon::Update::Unreachable(reason) => State::Unreachable(reason.into()),
                    };
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn retry(&mut self, _: &Retry, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state, State::Unreachable(_)) {
            self.connect(cx);
        }
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    /// With `OSSTEAM_GUI_STARTUP_TIMING` set, prints the time from process
    /// start to the first presented frame, for the §20 budget and the M0
    /// spike record. With the value `exit`, the GUI then quits.
    fn report_first_frame(&mut self, window: &mut Window) {
        if self.first_frame_reported {
            return;
        }
        self.first_frame_reported = true;
        let Some(mode) = std::env::var_os("OSSTEAM_GUI_STARTUP_TIMING") else {
            return;
        };
        let started = self.started;
        window.on_next_frame(move |window, cx| {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "first frame after {ms:.1} ms (scale factor {})",
                window.scale_factor()
            );
            if mode == "exit" {
                cx.quit();
            }
        });
    }

    fn body(&self, p: &Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.state {
            State::Connecting => div()
                .text_color(p.muted)
                .child("Connecting to ossteamd…")
                .into_any_element(),
            State::Ready { status, version } => {
                let protocol = version.protocol.unwrap_or_default();
                let connection = match ConnectionState::try_from(status.connection) {
                    Ok(ConnectionState::Connected) => "Connected to Steam",
                    Ok(ConnectionState::Connecting) => "Connecting to Steam",
                    Ok(ConnectionState::Disconnected) => "Not connected to Steam",
                    _ => "Steam connection state unknown",
                };
                let account = match &status.account {
                    Some(a) => format!("Signed in as {}", a.persona_name),
                    None => "Not signed in".to_owned(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(p.text).child(connection))
                    .child(div().text_color(p.text).child(account))
                    .child(div().text_sm().text_color(p.muted).child(format!(
                        "ossteamd {} · pid {} · protocol {}.{}",
                        status.daemon_version, status.pid, protocol.major, protocol.minor
                    )))
                    .into_any_element()
            }
            State::Unreachable(reason) => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_color(p.error).child("Cannot reach ossteamd"))
                .child(div().text_sm().text_color(p.muted).child(reason.clone()))
                .child(
                    div().flex().child(
                        widgets::button("retry", "Retry", p)
                            .track_focus(&self.retry_focus)
                            .focus_visible({
                                let ring = p.focus_ring;
                                move |s| s.border_2().border_color(ring)
                            })
                            .on_click(
                                cx.listener(|root, _, window, cx| root.retry(&Retry, window, cx)),
                            ),
                    ),
                )
                .into_any_element(),
        }
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.report_first_frame(window);
        let p = Palette::for_appearance(window.appearance());
        div()
            .key_context("Root")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::retry))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.background)
            .text_color(p.text)
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.surface)
                    .child(div().text_xl().child("ossteam"))
                    .child(self.body(&p, cx)),
            )
    }
}

fn main() {
    let started = Instant::now();
    gpui_platform::application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("enter", Retry, Some("Root")),
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
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
            |window, cx| cx.new(|cx| Root::new(started, window, cx)),
        );
        if let Err(e) = opened {
            eprintln!("ossteam-gui: cannot open a window: {e:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
