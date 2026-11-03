// SPDX-License-Identifier: GPL-2.0-or-later
//! A scripted input method for testing IME on a Wayland compositor that
//! supports `zwp_input_method_v2` (sway does): it composes and commits
//! text in whichever window has the keyboard focus, as an IME such as
//! fcitx5 would, and prints what the compositor tells it as JSON lines.
//!
//! Run it in a compositor of its own, never the desktop's: only one input
//! method can be bound to a seat.
//!
//! ```sh
//! WAYLAND_DISPLAY=wayland-2 cargo run -p ossteam-ui --example wayland_ime -- \
//!     wait popup preedit:にほ:3:6 sleep:500 preedit: commit:日本 sleep:300
//! ```
//!
//! Steps, run in order:
//!
//! - `wait`: until a text field is active (`activate` and `done`).
//! - `popup`: shows a 1×1 candidate popup, so the compositor reports the
//!   caret rectangle the client gave (`text_input_rectangle`).
//! - `preedit:TEXT:BEGIN:END`: a composition with its cursor (bytes;
//!   `-1:-1` hides it); `preedit:` clears it.
//! - `commit:TEXT`: commits text, ending any composition.
//! - `sleep:MS`: waits, still reading events.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn main() {
    if let Err(e) = linux::run(std::env::args().skip(1).collect()) {
        println!("{}", serde_json::json!({ "error": e }));
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn main() {
    eprintln!("wayland_ime: Wayland only");
    std::process::exit(2);
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use std::time::{Duration, Instant};

    use serde_json::json;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_seat::WlSeat,
        wl_shm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    };
    use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
    use wayland_protocols_misc::zwp_input_method_v2::client::{
        zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        zwp_input_method_v2::{self, ZwpInputMethodV2},
        zwp_input_popup_surface_v2::{self, ZwpInputPopupSurfaceV2},
    };

    #[derive(Default)]
    struct State {
        started: Option<Instant>,
        /// `done` events so far: the serial `commit` must carry.
        serial: u32,
        active: bool,
        pending_active: bool,
        unavailable: bool,
    }

    impl State {
        fn print(&self, value: serde_json::Value) {
            let ms = self
                .started
                .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0);
            println!(
                "{}",
                json!({ "ms": (ms * 10.0).round() / 10.0, "event": value })
            );
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_noop!(State: ignore WlSeat);
    delegate_noop!(State: WlCompositor);
    delegate_noop!(State: ignore wl_shm::WlShm);
    delegate_noop!(State: WlShmPool);
    delegate_noop!(State: ignore WlBuffer);
    delegate_noop!(State: ignore WlSurface);
    delegate_noop!(State: ZwpInputMethodManagerV2);

    impl Dispatch<ZwpInputMethodV2, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZwpInputMethodV2,
            event: zwp_input_method_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use zwp_input_method_v2::Event;
            match event {
                Event::Activate => {
                    state.pending_active = true;
                    state.print(json!("activate"));
                }
                Event::Deactivate => {
                    state.pending_active = false;
                    state.print(json!("deactivate"));
                }
                Event::SurroundingText {
                    text,
                    cursor,
                    anchor,
                } => state.print(json!({ "surrounding_text": text, "cursor": cursor, "anchor": anchor })),
                Event::TextChangeCause { cause } => {
                    state.print(json!({ "text_change_cause": format!("{cause:?}") }))
                }
                Event::ContentType { hint, purpose } => state.print(
                    json!({ "content_type": { "hint": format!("{hint:?}"), "purpose": format!("{purpose:?}") } }),
                ),
                Event::Done => {
                    state.serial += 1;
                    state.active = state.pending_active;
                    state.print(json!({ "done": state.serial, "active": state.active }));
                }
                Event::Unavailable => {
                    state.unavailable = true;
                    state.print(json!("unavailable"));
                }
                _ => {}
            }
        }
    }

    impl Dispatch<ZwpInputPopupSurfaceV2, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZwpInputPopupSurfaceV2,
            event: zwp_input_popup_surface_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
                x,
                y,
                width,
                height,
            } = event
            {
                state.print(json!({ "text_input_rectangle": [x, y, width, height] }));
            }
        }
    }

    /// Reads events for `time`.
    fn pump(
        queue: &mut EventQueue<State>,
        state: &mut State,
        time: Duration,
    ) -> Result<(), String> {
        let until = Instant::now() + time;
        loop {
            queue.roundtrip(state).map_err(|e| e.to_string())?;
            if Instant::now() >= until || state.unavailable {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn run(steps: Vec<String>) -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("connecting: {e}"))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
        let qh = queue.handle();
        let bind_err = |what: &'static str| {
            move |e: wayland_client::globals::BindError| format!("{what}: {e}")
        };
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(bind_err("wl_seat"))?;
        let manager: ZwpInputMethodManagerV2 = globals
            .bind(&qh, 1..=1, ())
            .map_err(bind_err("zwp_input_method_manager_v2"))?;
        let im = manager.get_input_method(&seat, &qh, ());
        let mut state = State {
            started: Some(Instant::now()),
            ..State::default()
        };
        pump(&mut queue, &mut state, Duration::ZERO)?;
        let mut popup = None;
        for step in steps {
            if state.unavailable {
                return Err("another input method is bound to the seat".into());
            }
            let (verb, rest) = step.split_once(':').unwrap_or((step.as_str(), ""));
            state.print(json!({ "step": step }));
            match verb {
                "wait" => {
                    let until = Instant::now() + Duration::from_secs(10);
                    while !state.active {
                        if Instant::now() >= until {
                            return Err("no text field became active in 10 s".into());
                        }
                        pump(&mut queue, &mut state, Duration::from_millis(20))?;
                    }
                }
                "popup" => {
                    let compositor: WlCompositor = globals
                        .bind(&qh, 1..=4, ())
                        .map_err(bind_err("wl_compositor"))?;
                    let shm: wl_shm::WlShm =
                        globals.bind(&qh, 1..=1, ()).map_err(bind_err("wl_shm"))?;
                    let surface = compositor.create_surface(&qh, ());
                    let p = im.get_input_popup_surface(&surface, &qh, ());
                    let buffer = one_pixel(&shm, &qh)?;
                    surface.attach(Some(&buffer), 0, 0);
                    surface.commit();
                    popup = Some((surface, p, buffer));
                    pump(&mut queue, &mut state, Duration::from_millis(50))?;
                }
                "preedit" => {
                    let mut parts = rest.splitn(3, ':');
                    let text = parts.next().unwrap_or("");
                    let num = |p: Option<&str>| p.and_then(|v| v.parse().ok());
                    let begin = num(parts.next()).unwrap_or(text.len() as i32);
                    let end = num(parts.next()).unwrap_or(begin);
                    im.set_preedit_string(text.to_owned(), begin, end);
                    im.commit(state.serial);
                    pump(&mut queue, &mut state, Duration::ZERO)?;
                }
                "commit" => {
                    im.commit_string(rest.to_owned());
                    im.commit(state.serial);
                    pump(&mut queue, &mut state, Duration::ZERO)?;
                }
                "sleep" => {
                    let ms: u64 = rest.parse().map_err(|e| format!("{step}: {e}"))?;
                    pump(&mut queue, &mut state, Duration::from_millis(ms))?;
                }
                other => return Err(format!("unknown step {other:?}")),
            }
        }
        drop(popup);
        im.destroy();
        conn.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// A 1×1 transparent buffer, so the popup surface is mapped.
    fn one_pixel(shm: &wl_shm::WlShm, qh: &QueueHandle<State>) -> Result<WlBuffer, String> {
        use std::os::fd::AsFd;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
        let path = std::path::Path::new(&dir).join(format!("ossteam-ime-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&path);
        file.set_len(4).map_err(|e| e.to_string())?;
        let pool = shm.create_pool(file.as_fd(), 4, qh, ());
        let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        Ok(buffer)
    }
}
