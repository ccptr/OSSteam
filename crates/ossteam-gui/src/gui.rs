// SPDX-License-Identifier: GPL-2.0-or-later
//! The window (`gui.md` §1): the header with the pages and the account
//! menu, the page, the status bar and notices; or, without a daemon, why
//! there is none.

use std::time::Instant;

use ossteam_ipc::v1::{self, ConnectionState, NoticeLevel};
use ossteam_ipc::{Client, ClientError};
use ossteam_ui::app::Sender;
use ossteam_ui::element::{Element, FocusHandle, overlay, panel, text};
use ossteam_ui::keys::KeyBinding;
use ossteam_ui::scene::Color;
use ossteam_ui::style::{Align, Edges, Shadow, Styled};
use ossteam_ui::window::{App, Cx, Timer, ViewCx};

use crate::daemon::{self, CallError, Update};
use crate::format;
use crate::model::{Model, Reply};
use crate::pages::app::AppPage;
use crate::pages::downloads::DownloadsPage;
use crate::pages::library::LibraryPage;
use crate::pages::login::LoginPage;
use crate::pages::settings::SettingsPage;
use crate::pages::{self, app, library};
use crate::queue;
use crate::theme::Palette;
use crate::thumbnails::{Loaded, Thumbnails};
use crate::widgets::{self, El, SMALL, button, muted, page_keys};

/// What the GUI's other threads send its main thread. The number is the
/// connection's: what an earlier connection sends is dropped.
pub enum Msg {
    Connection(u64, Update),
    Model(u64, Reply),
    Capsule(Loaded),
    Artwork(Loaded),
    /// A call a view made, with what to do with its result.
    Done(u64, Then),
}

/// What to do on the main thread with a result from another.
pub type Then = Box<dyn FnOnce(&mut Gui, &mut Cx<Gui>) + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Library,
    Downloads,
    Settings,
}

enum Connection {
    Connecting,
    Down(String),
    Up(Box<Model>),
}

#[derive(Clone, Debug)]
struct Quit;
#[derive(Clone, Debug)]
struct ShowLibrary;
#[derive(Clone, Debug)]
struct ShowDownloads;
#[derive(Clone, Debug)]
struct ShowSettings;
#[derive(Clone, Debug)]
struct FocusSearch;
#[derive(Clone, Debug)]
struct PlaySelected;
#[derive(Clone, Debug)]
struct CloseMenu;

const MENU_CONTEXT: &str = "AccountMenu";

/// The key map of `gui.md` §6.
pub fn bindings() -> Vec<KeyBinding> {
    let mut b = vec![
        KeyBinding::new("primary+q", Quit, None),
        KeyBinding::new("primary+1", ShowLibrary, None),
        KeyBinding::new("primary+2", ShowDownloads, None),
        KeyBinding::new("primary+3", ShowSettings, None),
        KeyBinding::new("primary+f", FocusSearch, None),
        KeyBinding::new("primary+enter", PlaySelected, None),
        KeyBinding::new("escape", CloseMenu, Some(MENU_CONTEXT)),
    ];
    b.extend(page_keys::bindings());
    b.extend(library::bindings());
    b.extend(app::bindings());
    b
}

/// Capsules kept decoded: more than a 4K display shows at once (§4.2).
const CAPSULES_KEPT: usize = 256;

pub struct Gui {
    sender: Sender<Msg>,
    generation: u64,
    started: Instant,
    first_frame_reported: bool,
    autostart: bool,
    connection: Connection,
    /// The model's deadline and the timer kept for it.
    model_timer: Option<(Instant, Timer)>,
    pub page: Page,
    menu_open: bool,
    /// Add account: the Login page while an account is active.
    adding_account: bool,
    /// The Login page was shown at the last build.
    showed_login: bool,
    pub login: LoginPage,
    pub library: LibraryPage,
    pub app: Option<AppPage>,
    pub downloads: DownloadsPage,
    pub settings: SettingsPage,
    pub capsules: Thumbnails,
    /// The App page's artwork, kept across App pages (§4.3).
    pub artwork: Thumbnails,
}

/// The account button, where focus goes when its menu closes.
fn account_button() -> FocusHandle {
    FocusHandle::named("account-menu")
}

fn menu_focus() -> FocusHandle {
    FocusHandle::named("account-menu-items")
}

impl Gui {
    pub fn new(sender: Sender<Msg>, started: Instant, autostart: bool) -> Self {
        let capsules = {
            let s = sender.clone();
            Thumbnails::new(
                CAPSULES_KEPT,
                std::sync::Arc::new(move |l| {
                    s.send(Msg::Capsule(l));
                }),
            )
        };
        let artwork = {
            let s = sender.clone();
            Thumbnails::new(
                app::ARTWORK_KEPT,
                std::sync::Arc::new(move |l| {
                    s.send(Msg::Artwork(l));
                }),
            )
        };
        let mut gui = Self {
            sender,
            generation: 0,
            started,
            first_frame_reported: false,
            autostart,
            connection: Connection::Connecting,
            model_timer: None,
            page: Page::Library,
            menu_open: false,
            adding_account: false,
            showed_login: false,
            login: LoginPage::default(),
            library: LibraryPage::default(),
            app: None,
            downloads: DownloadsPage::default(),
            settings: SettingsPage::default(),
            capsules,
            artwork,
        };
        gui.connect();
        gui
    }

    fn connect(&mut self) {
        self.generation += 1;
        self.connection = Connection::Connecting;
        let (sender, generation) = (self.sender.clone(), self.generation);
        daemon::connect(self.autostart, move |u| {
            sender.send(Msg::Connection(generation, u))
        });
    }

    fn ready(&mut self, d: daemon::Daemon, initial: daemon::Initial, cx: &mut Cx<Self>) {
        let (sender, generation) = (self.sender.clone(), self.generation);
        let post = std::sync::Arc::new(move |r| {
            sender.send(Msg::Model(generation, r));
        });
        self.connection = Connection::Up(Box::new(Model::new(d, initial, post)));
        // Fresh pages for a fresh connection.
        self.login = LoginPage::default();
        self.library = LibraryPage::default();
        self.app = None;
        self.downloads = DownloadsPage::default();
        self.settings = SettingsPage::default();
        self.page = Page::Library;
        self.adding_account = false;
        self.showed_login = false;
        cx.blur();
    }

    pub fn model(&self) -> Option<&Model> {
        match &self.connection {
            Connection::Up(m) => Some(m),
            _ => None,
        }
    }

    pub fn model_mut(&mut self) -> Option<&mut Model> {
        match &mut self.connection {
            Connection::Up(m) => Some(m),
            _ => None,
        }
    }

    /// Makes a call; `then` gets its result on the main thread. Nothing
    /// happens without a connection.
    pub fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
        then: impl FnOnce(&mut Gui, Result<T, CallError>, &mut Cx<Gui>) + Send + 'static,
    ) {
        let Some(m) = self.model() else {
            return;
        };
        let (sender, generation) = (self.sender.clone(), self.generation);
        m.daemon.call(f, move |r| {
            sender.send(Msg::Done(generation, Box::new(move |g, cx| then(g, r, cx))));
        });
    }

    /// Like [`call`](Self::call), on a thread of its own, for a call that
    /// waits without a timeout (`Launch`, §2.1).
    pub fn call_unbounded<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
        then: impl FnOnce(&mut Gui, Result<T, CallError>, &mut Cx<Gui>) + Send + 'static,
    ) {
        let Some(m) = self.model() else {
            return;
        };
        let (sender, generation) = (self.sender.clone(), self.generation);
        m.daemon.call_unbounded(f, move |r| {
            sender.send(Msg::Done(generation, Box::new(move |g, cx| then(g, r, cx))));
        });
    }

    /// Runs `f` on a thread of its own, for work that blocks, such as the
    /// folder picker; `then` gets its result on the main thread.
    pub fn background<T: Send + 'static>(
        &self,
        name: &str,
        f: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut Gui, T, &mut Cx<Gui>) + Send + 'static,
    ) {
        let (sender, generation) = (self.sender.clone(), self.generation);
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let v = f();
                sender.send(Msg::Done(generation, Box::new(move |g, cx| then(g, v, cx))));
            });
        if let Err(e) = spawned {
            eprintln!("ossteam-gui: cannot start a thread for {name}: {e}");
        }
    }

    /// Whether the Login page stands in for the Library page.
    pub fn showing_login(&self) -> bool {
        self.model().is_some_and(|m| {
            self.page == Page::Library && (self.adding_account || m.store.needs_login())
        })
    }

    pub fn open_app(&mut self, app_id: u32, cx: &mut Cx<Self>) {
        self.close_app();
        let Some(m) = self.model_mut() else {
            return;
        };
        m.open_app(app_id);
        self.app = Some(AppPage::new(app_id));
        self.page = Page::Library;
        cx.focus(app::page_focus());
    }

    /// Closes the App page, if one is open; the library keeps its scroll
    /// and search.
    fn close_app(&mut self) {
        if let Some(app) = self.app.take()
            && let Some(m) = self.model_mut()
        {
            m.close_app(app.app_id);
        }
    }

    pub fn back_to_library(&mut self, cx: &mut Cx<Self>) {
        self.close_app();
        cx.focus(library::items_focus());
    }

    pub fn show(&mut self, page: Page, cx: &mut Cx<Self>) {
        self.page = page;
        self.menu_open = false;
        let target = match page {
            Page::Library if self.showing_login() => pages::login::first_field(self),
            Page::Library if self.app.is_some() => app::page_focus(),
            Page::Library => library::items_focus(),
            Page::Downloads => pages::downloads::page_focus(),
            Page::Settings => pages::settings::page_focus(),
        };
        cx.focus(target);
    }

    fn close_menu(&mut self, cx: &mut Cx<Self>) {
        self.menu_open = false;
        cx.focus_from_keyboard(account_button());
    }

    /// Keeps a timer for the model's deadline.
    fn keep_model_timer(&mut self, cx: &mut Cx<Self>) {
        let due = self.model().and_then(Model::deadline);
        if self.model_timer.as_ref().map(|(t, _)| *t) == due {
            return;
        }
        self.model_timer = due.map(|t| {
            let delay = t.saturating_duration_since(Instant::now());
            let timer = cx.after(delay, |g: &mut Self, _| {
                g.model_timer = None;
                if let Some(m) = g.model_mut() {
                    m.tick(Instant::now());
                }
            });
            (t, timer)
        });
    }

    /// With `OSSTEAM_GUI_STARTUP_TIMING` set, prints the time from process
    /// start to the first presented frame, for the §20 budget. With the
    /// value `exit`, the GUI then quits.
    fn report_first_frame(&mut self, cx: &mut Cx<Self>) {
        if std::mem::replace(&mut self.first_frame_reported, true) {
            return;
        }
        let Some(mode) = std::env::var_os("OSSTEAM_GUI_STARTUP_TIMING") else {
            return;
        };
        let started = self.started;
        cx.on_next_frame(move |_, cx| {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            println!("first frame after {ms:.1} ms (scale factor {})", cx.scale());
            if mode == "exit" {
                cx.quit();
            }
        });
    }
}

impl App for Gui {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        let current = self.generation;
        match msg {
            Msg::Connection(g, _) | Msg::Model(g, _) | Msg::Done(g, _) if g != current => {}
            Msg::Connection(_, Update::Ready(d, initial)) => self.ready(d, *initial, cx),
            Msg::Connection(_, Update::Event(kind)) => {
                if let Some(m) = self.model_mut() {
                    m.apply(kind);
                }
            }
            Msg::Connection(_, Update::Closed(reason)) => {
                self.connection = Connection::Down(reason);
                self.menu_open = false;
                self.app = None;
                cx.blur();
            }
            Msg::Model(_, reply) => {
                if let Some(m) = self.model_mut() {
                    m.handle(reply);
                }
            }
            Msg::Done(_, f) => f(self, cx),
            Msg::Capsule(l) => self.capsules.loaded(l),
            Msg::Artwork(l) => self.artwork.loaded(l),
        }
    }

    fn before_view(&mut self, cx: &mut Cx<Self>) {
        self.report_first_frame(cx);
        self.keep_model_timer(cx);
        let Some(m) = self.model() else {
            return;
        };
        let logged_on = pages::login::logged_on(&m.store).is_some();
        // An App page whose details went with the account closes.
        let app_gone = self
            .app
            .as_ref()
            .is_some_and(|a| !m.store.apps.contains_key(&a.app_id));
        if self.adding_account && logged_on {
            self.adding_account = false;
        }
        if app_gone {
            self.app = None;
            cx.focus(library::items_focus());
        }
        // The Login page appearing puts the cursor in its first field.
        let showing = self.showing_login();
        if showing && !self.showed_login {
            cx.focus(pages::login::first_field(self));
        }
        self.showed_login = showing;
        pages::login::before_view(self, cx);
        pages::app::before_view(self, cx);
        pages::settings::before_view(self, cx);
        pages::library::before_view(self, cx);
    }

    fn focus_lost(&mut self, cx: &mut Cx<Self>) {
        // A focused control that went away, such as Play once the game
        // runs: focus goes to its page, from where Tab goes on.
        let target = match self.page {
            Page::Library if self.showing_login() => pages::login::page_focus(),
            Page::Library if self.app.is_some() => app::page_focus(),
            Page::Library => library::items_focus(),
            Page::Downloads => pages::downloads::page_focus(),
            Page::Settings => pages::settings::page_focus(),
        };
        if self.model().is_some() {
            cx.focus(target);
        }
    }

    fn background(&self, cx: &ViewCx) -> Color {
        Palette::for_view(cx).background
    }

    fn tooltip(&self, tip: &str) -> Element<Self> {
        // The tooltip has no view context: the palette of the light
        // appearance would be wrong on a dark desktop, so it is drawn the
        // same in both.
        panel()
            .max_width(360.0)
            .pad_xy(8.0, 4.0)
            .radius(6.0)
            .background(Color::rgb(0x2a2e36))
            .text_color(Color::rgb(0xe8eaed))
            .font_size(SMALL)
            .child(text(tip))
    }

    fn view(&self, cx: &ViewCx) -> El {
        let p = Palette::for_view(cx);
        let content = match &self.connection {
            Connection::Connecting => down(p, None),
            Connection::Down(reason) => down(p, Some(reason)),
            Connection::Up(m) => {
                let page = match self.page {
                    Page::Library if self.showing_login() => pages::login::view(self, m, p, cx),
                    Page::Library => match &self.app {
                        Some(app) => pages::app::view(self, m, app, p, cx),
                        None => pages::library::view(self, m, p, cx),
                    },
                    Page::Downloads => pages::downloads::view(self, m, p, cx),
                    Page::Settings => pages::settings::view(self, m, p, cx),
                };
                panel()
                    .fill()
                    .child(self.header(m, p))
                    .child(panel().grow().min_height(0.0).child(page))
                    .child(status_bar(m, p))
                    .when_some(notices(m, p), |d, n| d.child(n))
            }
        };
        panel()
            .fill()
            .text_color(p.text)
            .on_action(|_: &mut Self, _: &Quit, cx: &mut Cx<Self>| cx.quit())
            .on_action(|g: &mut Self, _: &ShowLibrary, cx: &mut Cx<Self>| g.show(Page::Library, cx))
            .on_action(|g: &mut Self, _: &ShowDownloads, cx: &mut Cx<Self>| {
                g.show(Page::Downloads, cx)
            })
            .on_action(|g: &mut Self, _: &ShowSettings, cx: &mut Cx<Self>| {
                g.show(Page::Settings, cx)
            })
            .on_action(|g: &mut Self, _: &FocusSearch, cx: &mut Cx<Self>| {
                g.close_app();
                g.page = Page::Library;
                g.menu_open = false;
                if g.model().is_some() && !g.showing_login() {
                    cx.focus(library::search_focus());
                }
            })
            .on_action(|g: &mut Self, _: &PlaySelected, cx: &mut Cx<Self>| play_selected(g, cx))
            .child(content)
    }
}

/// Ctrl-Enter (§6): the open App page's main action, or the selected
/// app's, through its App page.
fn play_selected(g: &mut Gui, cx: &mut Cx<Gui>) {
    if g.page != Page::Library || g.showing_login() || g.model().is_none() {
        return;
    }
    if g.app.is_none() {
        let Some(id) = g.library.selected else {
            return;
        };
        g.open_app(id, cx);
    }
    pages::app::main_action(g, cx);
}

impl Gui {
    fn header(&self, m: &Model, p: Palette) -> El {
        let tab = |id: &str, label: &str, page: Page| {
            button(p, id, label)
                .ghost()
                .selected(self.page == page)
                .on_click(move |g, cx| g.show(page, cx))
        };
        let store = &m.store;
        let account_label = match store.account() {
            Some(a) => format!(
                "{} · {}",
                if a.persona_name.is_empty() {
                    &a.account_name
                } else {
                    &a.persona_name
                },
                format::persona_state(store.status.persona_state())
            ),
            None => "Not signed in".to_owned(),
        };
        let account = panel()
            .child(
                button(p, "account-menu", account_label)
                    .ghost()
                    .selected(self.menu_open)
                    .on_click(|g, cx| {
                        g.menu_open = !g.menu_open;
                        if g.menu_open {
                            cx.focus(menu_focus());
                        }
                    }),
            )
            .when(self.menu_open, |d| {
                // The gap is inside the overlay: a margin on the overlay
                // would grow the header.
                d.child(overlay(
                    panel()
                        .padding(Edges {
                            top: 4.0,
                            ..Edges::default()
                        })
                        .child(account_menu(m, p)),
                ))
            });
        panel()
            .row()
            .align(Align::Center)
            .gap(4.0)
            .pad_xy(16.0, 8.0)
            .border_sides(
                Edges {
                    bottom: 1.0,
                    ..Edges::default()
                },
                p.border,
            )
            .background(p.surface)
            .child(text("ossteam").font_size(18.0).margin(Edges {
                right: 16.0,
                ..Edges::default()
            }))
            .child(tab("page-library", "Library", Page::Library))
            .child(tab("page-downloads", "Downloads", Page::Downloads))
            .child(tab("page-settings", "Settings", Page::Settings))
            .child(panel().grow())
            .child(account)
    }
}

/// The account menu (§4.6). It comes right after the account button in
/// Tab order, so Shift-Tab out of it reaches the button and Tab out of it
/// goes on from there; either way it closes, as with Escape.
fn account_menu(m: &Model, p: Palette) -> El {
    let store = &m.store;
    let active = store.account().map(|a| a.steam_id);
    let mut menu = panel()
        .id("account-menu-box")
        .focus(menu_focus())
        .key_context(MENU_CONTEXT)
        .on_action(|g: &mut Gui, _: &CloseMenu, cx: &mut Cx<Gui>| g.close_menu(cx))
        .on_focus_out(|g: &mut Gui, _| g.menu_open = false)
        .on_press_outside(|g: &mut Gui, _| g.menu_open = false)
        .width(260.0)
        .pad(8.0)
        .gap(4.0)
        .radius(6.0)
        .border(1.0, p.border)
        .background(p.surface)
        .shadow(Shadow {
            color: Color::rgb(0).with_alpha(0.25),
            offset_x: 0.0,
            offset_y: 4.0,
            blur: 8.0,
            spread: 0.0,
        });
    if let Some(a) = store.account() {
        let state = if store.status.logged_on {
            format::persona_state(store.status.persona_state())
        } else {
            "not logged on"
        };
        menu = menu.child(
            panel()
                .pad_xy(8.0, 4.0)
                .child(text(a.persona_name.clone()).font_size(widgets::TEXT))
                .child(
                    text(format!("{} · {state}", a.account_name))
                        .font_size(SMALL)
                        .text_color(p.muted),
                ),
        );
    }
    for a in store
        .accounts
        .accounts
        .iter()
        .filter(|a| Some(a.steam_id) != active)
    {
        let id = a.steam_id;
        let label = if a.persona_name.is_empty() {
            a.account_name.clone()
        } else {
            format!("{} ({})", a.persona_name, a.account_name)
        };
        menu = menu.child(
            button(p, widgets::id("menu-account", id), label)
                .ghost()
                .full_width()
                .on_click(move |g, cx| {
                    g.close_menu(cx);
                    if let Some(m) = g.model_mut() {
                        m.run(move |c| c.switch_account(id), |_, _| {});
                    }
                }),
        );
    }
    menu = menu.child(
        button(p, "menu-add-account", "Add account")
            .ghost()
            .full_width()
            .on_click(|g, cx| {
                if let Some(m) = g.model_mut() {
                    m.store.login_stopped(None);
                }
                g.login = LoginPage::default();
                g.menu_open = false;
                g.adding_account = true;
                g.show(Page::Library, cx);
            }),
    );
    if store.account().is_some() {
        menu = menu.child(
            button(p, "menu-logout", "Log out")
                .ghost()
                .full_width()
                .on_click(|g, cx| {
                    g.close_menu(cx);
                    if let Some(m) = g.model_mut() {
                        m.run(|c| c.logout(), |_, ()| {});
                    }
                }),
        );
    }
    menu
}

/// The status bar (§4.7).
fn status_bar(m: &Model, p: Palette) -> El {
    let store = &m.store;
    let connection = match store.status.connection() {
        ConnectionState::Connected => "Connected to Steam",
        ConnectionState::Connecting => "Connecting to Steam",
        ConnectionState::Disconnected => "Not connected to Steam",
        ConnectionState::Unspecified => "Steam connection unknown",
    };
    let account = match store.account() {
        Some(a) if store.status.logged_on => format!("{} logged on", a.account_name),
        Some(a) => format!("{} not logged on", a.account_name),
        None => "no account".to_owned(),
    };
    let q = queue::queue(store.jobs.values());
    let jobs = match q.iter().find(|j| j.state() == v1::JobState::Running) {
        Some(j) => {
            let name = store
                .app_name(j.app_id)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("App {}", j.app_id));
            let pct = j
                .progress
                .as_ref()
                .and_then(format::fraction)
                .map(|f| format!(" {:.0}%", f * 100.))
                .unwrap_or_default();
            match q.len() - 1 {
                0 => format!("{} {name}{pct}", format::kind(j.kind())),
                more => format!("{} {name}{pct}, {more} more", format::kind(j.kind())),
            }
        }
        None => queue::counts(&q),
    };
    let mut games = panel().row().gap(8.0);
    for g in store.running.values() {
        let id = g.app_id;
        let name = store
            .app_name(id)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("App {id}"));
        games = games.child(
            panel()
                .row()
                .align(Align::Center)
                .gap(4.0)
                .child(text(format!("{name} running")))
                .child(
                    button(p, widgets::id("stop-game", id), "Stop")
                        .danger()
                        .on_click(move |g, _| {
                            if let Some(m) = g.model_mut() {
                                m.run(move |c| c.stop_game(id), |_, ()| {});
                            }
                        }),
                ),
        );
    }
    panel()
        .row()
        .align(Align::Center)
        .gap(16.0)
        .pad_xy(16.0, 4.0)
        .border_sides(
            Edges {
                top: 1.0,
                ..Edges::default()
            },
            p.border,
        )
        .background(p.surface)
        .font_size(SMALL)
        .text_color(p.muted)
        .child(text(connection))
        .child(text(account))
        .when(!jobs.is_empty(), |d| d.child(text(jobs)))
        .child(panel().grow())
        .child(games)
}

/// Notices, newest last, over the bottom right corner (§2.4).
fn notices(m: &Model, p: Palette) -> Option<El> {
    let notices = &m.store.notices;
    if notices.is_empty() {
        return None;
    }
    let mut list = panel()
        .absolute()
        .inset(Edges {
            right: 16.0.into(),
            bottom: 40.0.into(),
            ..Edges::all(ossteam_ui::style::Length::Auto)
        })
        .width(360.0)
        .gap(8.0);
    for n in notices {
        let color = match n.level {
            NoticeLevel::Error => p.error,
            NoticeLevel::Warning => p.warning,
            _ => p.text,
        };
        let id = n.id;
        list = list.child(
            panel()
                .row()
                .align(Align::Start)
                .gap(8.0)
                .pad(12.0)
                .radius(6.0)
                .border(1.0, p.border)
                .background(p.surface)
                .shadow(Shadow {
                    color: Color::rgb(0).with_alpha(0.2),
                    offset_x: 0.0,
                    offset_y: 2.0,
                    blur: 6.0,
                    spread: 0.0,
                })
                .child(
                    text(n.text.clone())
                        .grow()
                        .font_size(widgets::TEXT)
                        .text_color(color),
                )
                .child(
                    button(p, widgets::id("dismiss", id), "Dismiss")
                        .ghost()
                        .on_click(move |g, _| {
                            if let Some(m) = g.model_mut() {
                                m.dismiss_notice(id);
                            }
                        }),
                ),
        );
    }
    Some(list)
}

/// No daemon: connecting, or why there is none, with Retry (§1).
fn down(p: Palette, reason: Option<&str>) -> El {
    let body = match reason {
        None => muted(p, "Connecting to ossteamd…"),
        Some(reason) => panel()
            .gap(12.0)
            .child(text("Cannot reach ossteamd").text_color(p.error))
            .child(muted(p, reason))
            .child(button(p, "retry", "Retry").on_click(|g, cx| {
                if matches!(g.connection, Connection::Down(_)) {
                    g.connect();
                    cx.blur();
                }
            })),
    };
    panel().fill().center().child(
        panel()
            .width(420.0)
            .gap(16.0)
            .pad(24.0)
            .radius(8.0)
            .border(1.0, p.border)
            .background(p.surface)
            .child(text("ossteam").font_size(widgets::TITLE))
            .child(body),
    )
}
