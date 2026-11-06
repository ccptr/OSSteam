// SPDX-License-Identifier: GPL-2.0-or-later
//! Running an [`App`] in a window (ADR 0004, "Synchronous"): winit's event
//! loop on the main thread with `ControlFlow::Wait`, woken by input, by
//! the next timer (`WaitUntil`) or by a [`Sender`] from another thread.
//! A window that nothing changed does not wake and does not draw (GUI-5).
//!
//! The fonts and, on Linux, the desktop's appearance are asked for when
//! [`run`] starts, so both are ready by the time the window exists and the
//! first frame has the right colours (gui.md §6).

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorIcon, WindowId};

use crate::Error;
use crate::appearance::{self, Appearance, StartupAppearance};
use crate::clipboard::Clipboard;
use crate::keys::KeyBinding;
use crate::render::{Choice, Renderer};
use crate::text::{FontsLoading, TextSystem};
use crate::window::{App, Input, WHEEL_LINE, Window};

/// How long the first frame waits for the portal's appearance (Linux).
const APPEARANCE_WAIT: Duration = Duration::from_millis(50);

/// The window to open.
#[derive(Clone, Debug)]
pub struct Options {
    pub title: String,
    /// The Wayland app id and X11 class.
    pub app_id: Option<String>,
    /// Logical pixels.
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    /// The program's choice, such as its `--renderer`. With none,
    /// `OSSTEAM_UI_RENDERER` picks one, and `auto` without it.
    pub renderer: Option<Choice>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            title: String::new(),
            app_id: None,
            width: 960.0,
            height: 640.0,
            min_width: 320.0,
            min_height: 240.0,
            renderer: None,
        }
    }
}

enum Event<M> {
    Message(M),
    Appearance(Appearance),
}

/// Sends messages to the app from any thread; each wakes the event loop
/// and reaches [`App::message`].
pub struct Sender<M: Send + 'static>(EventLoopProxy<Event<M>>);

impl<M: Send + 'static> Clone for Sender<M> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<M: Send + 'static> Sender<M> {
    /// Returns `false` once the event loop has ended.
    pub fn send(&self, m: M) -> bool {
        self.0.send_event(Event::Message(m)).is_ok()
    }
}

/// What the runtime reports about how it started, for logs and tests.
#[derive(Clone, Debug, Default)]
pub struct Started {
    /// `vulkan` or `software`, and why.
    pub renderer: &'static str,
    pub renderer_why: String,
    /// Where the first frame's appearance came from: `portal`, `winit` or
    /// `default`.
    pub appearance_source: &'static str,
}

/// Opens the window and runs `A` until it quits or the window closes.
/// `init` makes the state, with a sender for its threads.
pub fn run<A: App>(
    options: Options,
    bindings: Vec<KeyBinding>,
    init: impl FnOnce(Sender<A::Message>) -> A,
) -> Result<Started, Error> {
    let appearance = StartupAppearance::query();
    let fonts = TextSystem::load_in_background();
    let event_loop = EventLoop::<Event<A::Message>>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let state = init(Sender(proxy.clone()));
    let mut runner = Runner {
        options,
        bindings: Some(bindings),
        state: Some(state),
        appearance: Some(appearance),
        fonts: Some(fonts),
        proxy,
        started: Started::default(),
        portal_appearance: false,
        window: None,
        error: None,
    };
    event_loop.run_app(&mut runner)?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(runner.started),
    }
}

/// The refresh interval of the display `os` is on, if winit knows it.
fn refresh_interval(os: &winit::window::Window) -> Option<Duration> {
    let mhz = os.current_monitor()?.refresh_rate_millihertz()?;
    (mhz > 0).then(|| Duration::from_secs_f64(1000.0 / f64::from(mhz)))
}

struct Live<A: App> {
    os: Arc<winit::window::Window>,
    renderer: Renderer,
    ui: Window<A>,
    cursor: CursorIcon,
    ime_allowed: bool,
}

struct Runner<A: App> {
    options: Options,
    bindings: Option<Vec<KeyBinding>>,
    state: Option<A>,
    appearance: Option<StartupAppearance>,
    fonts: Option<FontsLoading>,
    proxy: EventLoopProxy<Event<A::Message>>,
    started: Started,
    /// The portal gives the appearance, so winit's theme is not used.
    portal_appearance: bool,
    window: Option<Live<A>>,
    error: Option<Error>,
}

impl<A: App> Runner<A> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, e: Error) {
        self.error = Some(e);
        event_loop.exit();
    }

    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Error> {
        let o = &self.options;
        #[allow(unused_mut)]
        let mut attrs = winit::window::Window::default_attributes()
            .with_title(o.title.clone())
            .with_inner_size(LogicalSize::new(o.width, o.height))
            .with_min_inner_size(LogicalSize::new(o.min_width, o.min_height));
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(id) = &o.app_id {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            attrs = WindowAttributesExtWayland::with_name(attrs, id.clone(), "");
            attrs = WindowAttributesExtX11::with_name(attrs, id.clone(), id.clone());
        }
        let os = Arc::new(event_loop.create_window(attrs)?);

        let portal = self.appearance.take().and_then(|q| q.wait(APPEARANCE_WAIT));
        let (look, source) = match (portal, os.theme()) {
            (Some(a), _) => (Some(a), "portal"),
            (None, Some(t)) => (Some(t.into()), "winit"),
            (None, None) => (None, "default"),
        };
        self.started.appearance_source = source;
        self.portal_appearance = portal.is_some();
        if self.portal_appearance {
            let proxy = self.proxy.clone();
            appearance::watch(move |a| {
                let _ = proxy.send_event(Event::Appearance(a));
            });
        }

        let (renderer, why) = Renderer::new(os.clone(), self.options.renderer)?;
        tracing::info!("renderer: {} ({why})", renderer.kind());
        self.started.renderer = renderer.kind();
        self.started.renderer_why = why;

        let text = self
            .fonts
            .take()
            .ok_or("the fonts were already taken")?
            .wait()?;
        let state = self.state.take().ok_or("the window was already opened")?;
        let size = os.inner_size();
        let clipboard = Box::new(Clipboard::new(os.as_ref()));
        let mut ui = Window::new(
            state,
            text,
            clipboard,
            size.width,
            size.height,
            os.scale_factor() as f32,
        );
        ui.set_appearance(look);
        ui.set_refresh_interval(refresh_interval(&os));
        ui.bind_keys(self.bindings.take().unwrap_or_default());
        os.request_redraw();
        self.window = Some(Live {
            os,
            renderer,
            ui,
            cursor: CursorIcon::Default,
            ime_allowed: false,
        });
        Ok(())
    }

    fn redraw(&mut self) -> Result<(), Error> {
        let Some(w) = &mut self.window else {
            return Ok(());
        };
        w.ui.advance(Instant::now());
        // Each renderer tells winit just before it presents: a frame the
        // software renderer skips (nothing changed) must not, or winit
        // waits for a frame callback that never comes and stops redrawing.
        if w.ui.frame().is_some() {
            w.renderer.draw(w.ui.scene(), &w.ui.text.glyphs)?;
            w.ui.presented();
        }
        let cursor = w.ui.cursor();
        if cursor != w.cursor {
            w.os.set_cursor(cursor);
            w.cursor = cursor;
        }
        let ime = w.ui.ime_area();
        if ime.is_some() != w.ime_allowed {
            w.ime_allowed = ime.is_some();
            w.os.set_ime_allowed(w.ime_allowed);
        }
        if let Some(r) = ime {
            w.os.set_ime_cursor_area(
                PhysicalPosition::new(r.x as i32, r.y as i32),
                winit::dpi::PhysicalSize::new(r.w.max(1.0) as u32, r.h.max(1.0) as u32),
            );
        }
        if w.ui.needs_frame() {
            w.os.request_redraw();
        }
        Ok(())
    }

    /// Asks for a frame if the window needs one, and quits if asked to.
    fn after(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &self.window else {
            return;
        };
        if w.ui.wants_quit() {
            event_loop.exit();
        } else if w.ui.needs_frame() {
            w.os.request_redraw();
        }
    }
}

impl<A: App> ApplicationHandler<Event<A::Message>> for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() || self.error.is_some() {
            return;
        }
        if let Err(e) = self.open(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, Some(w)) = (cause, &mut self.window) {
            w.ui.advance(Instant::now());
            if w.ui.needs_frame() {
                w.os.request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event<A::Message>) {
        match (&mut self.window, event) {
            (Some(w), Event::Message(m)) => {
                w.ui.advance(Instant::now());
                w.ui.message(m);
            }
            (Some(w), Event::Appearance(a)) => w.ui.set_appearance(Some(a)),
            // A message before the window opened is handled once it has.
            (None, Event::Message(m)) => {
                let _ = self.proxy.send_event(Event::Message(m));
                return;
            }
            (None, Event::Appearance(_)) => {}
        }
        self.after(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let portal = self.portal_appearance;
        let Some(w) = &mut self.window else {
            return;
        };
        let scale = w.os.scale_factor() as f32;
        let input = match event {
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.fail(event_loop, e);
                }
                return;
            }
            WindowEvent::CloseRequested => {
                if w.ui.close_requested() {
                    event_loop.exit();
                }
                None
            }
            // On Wayland the window enters a display only after it first
            // draws, so the display is asked again here.
            WindowEvent::Resized(size) => {
                w.ui.resize(size.width, size.height, scale);
                w.ui.set_refresh_interval(refresh_interval(&w.os));
                None
            }
            // A new scale factor most often means another display.
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = w.os.inner_size();
                w.ui.resize(size.width, size.height, scale_factor as f32);
                w.ui.set_refresh_interval(refresh_interval(&w.os));
                None
            }
            WindowEvent::ThemeChanged(theme) if !portal => {
                w.ui.set_appearance(Some(theme.into()));
                None
            }
            WindowEvent::CursorMoved { position, .. } => Some(Input::PointerMoved {
                x: position.x as f32,
                y: position.y as f32,
            }),
            WindowEvent::CursorLeft { .. } => Some(Input::PointerLeft),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => Some(match state {
                ElementState::Pressed => Input::Pressed,
                ElementState::Released => Input::Released,
            }),
            WindowEvent::MouseWheel { delta, .. } => Some(Input::Wheel {
                dy: match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * WHEEL_LINE,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / scale,
                },
            }),
            WindowEvent::ModifiersChanged(m) => Some(Input::Modifiers(m.state())),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => Some(Input::Key {
                key: event.logical_key,
                text: event.text.map(|t| t.to_string()),
            }),
            WindowEvent::Ime(ime) => Some(Input::Ime(ime)),
            _ => None,
        };
        if let Some(input) = input {
            w.ui.advance(Instant::now());
            w.ui.input(input);
        }
        self.after(event_loop);
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // Everything that holds winit's display connection goes while it
        // is open: the Wayland clipboard's worker crashes if it outlives
        // it. The renderer goes before the window it draws to.
        if let Some(w) = self.window.take() {
            let Live {
                os, renderer, ui, ..
            } = w;
            drop(renderer);
            drop(ui);
            drop(os);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // A timer or message may have asked to quit.
        if self.window.as_ref().is_some_and(|w| w.ui.wants_quit()) {
            event_loop.exit();
            return;
        }
        let deadline = self.window.as_ref().and_then(|w| w.ui.next_deadline());
        event_loop.set_control_flow(match deadline {
            Some(d) => ControlFlow::WaitUntil(d),
            None => ControlFlow::Wait,
        });
    }
}
