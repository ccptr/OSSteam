// SPDX-License-Identifier: GPL-2.0-or-later
//! The window (`gui.md` §1): the header with the pages and the account
//! menu, the page, the status bar and notices; or, without a daemon, why
//! there is none.

use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString,
    Subscription, Task, Window, actions, anchored, deferred, div, prelude::*, px,
};
use ossteam_ipc::v1::{self, ConnectionState, NoticeLevel};

use crate::daemon::{self, Update};
use crate::format;
use crate::model::Model;
use crate::pages::app::{ARTWORK_KEPT, AppPage, AppPageEvent};
use crate::pages::downloads::DownloadsPage;
use crate::pages::library::{LibraryEvent, LibraryPage};
use crate::pages::login::{self, LoginPage};
use crate::pages::settings::SettingsPage;
use crate::queue;
use crate::theme::{Palette, palette};
use crate::thumbnails::Thumbnails;
use crate::widgets::{button, reveal};

actions!(
    ossteam,
    [
        Retry,
        Quit,
        ShowLibrary,
        ShowDownloads,
        ShowSettings,
        FocusSearch,
        PlaySelected,
        FocusNext,
        FocusPrev,
        CloseMenu,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-1", ShowLibrary, None),
        KeyBinding::new("secondary-2", ShowDownloads, None),
        KeyBinding::new("secondary-3", ShowSettings, None),
        KeyBinding::new("secondary-f", FocusSearch, None),
        KeyBinding::new("secondary-enter", PlaySelected, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("escape", CloseMenu, Some("AccountMenu")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Downloads,
    Settings,
}

enum Connection {
    Connecting,
    Down(SharedString),
    Up(Box<Ui>),
}

/// The pages, made once connected.
struct Ui {
    model: Entity<Model>,
    login: Entity<LoginPage>,
    library: Entity<LibraryPage>,
    app: Option<Entity<AppPage>>,
    /// The App page's artwork, kept across App pages.
    artwork: Entity<Thumbnails>,
    downloads: Entity<DownloadsPage>,
    settings: Entity<SettingsPage>,
    _subscriptions: Vec<Subscription>,
}

pub struct Root {
    started: Instant,
    first_frame_reported: bool,
    autostart: bool,
    focus: FocusHandle,
    /// The account button, where Tab out of its menu goes on from.
    account_focus: FocusHandle,
    menu_focus: FocusHandle,
    connection: Connection,
    _connection_task: Option<Task<()>>,
    page: Page,
    menu_open: bool,
    /// Add account: the Login page while an account is active.
    adding_account: bool,
    /// The Login page was shown last frame.
    showed_login: bool,
}

impl Root {
    pub fn new(
        started: Instant,
        autostart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Palette::for_appearance(window.appearance()));
            cx.notify();
        })
        .detach();
        cx.set_global(Palette::for_appearance(window.appearance()));
        let menu_focus = cx.focus_handle();
        // Tab out of the open account menu closes it, as Escape does.
        cx.on_focus_out(&menu_focus, window, |root, _, _, cx| {
            root.menu_open = false;
            cx.notify();
        })
        .detach();
        // A focused control that goes away, such as Launch once the game
        // runs, leaves focus nowhere in the tree, where Tab reaches no
        // handler: focus goes to what held the control, or to the window.
        cx.on_focus_lost(window, |root, window, cx| {
            let target = window
                .focus_lost_restore_target(cx)
                .unwrap_or_else(|| root.focus.clone());
            window.focus(&target, cx);
        })
        .detach();
        let mut root = Self {
            started,
            first_frame_reported: false,
            autostart,
            focus,
            account_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            menu_focus,
            connection: Connection::Connecting,
            _connection_task: None,
            page: Page::Library,
            menu_open: false,
            adding_account: false,
            showed_login: false,
        };
        root.connect(window, cx);
        root
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connection = Connection::Connecting;
        let updates = daemon::connect(self.autostart);
        self._connection_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let applied = this.update_in(cx, |root, window, cx| {
                    match update {
                        Update::Ready(d, initial) => root.ready(d, *initial, window, cx),
                        Update::Event(kind) => {
                            if let Connection::Up(ui) = &root.connection {
                                ui.model.update(cx, |m, cx| m.apply(kind, cx));
                            }
                        }
                        Update::Closed(reason) => {
                            root.connection = Connection::Down(reason.into());
                            root.menu_open = false;
                            window.focus(&root.focus, cx);
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn ready(
        &mut self,
        d: daemon::Daemon,
        initial: daemon::Initial,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = cx.new(|_| Model::new(d, initial));
        let login = cx.new(|cx| LoginPage::new(model.clone(), window, cx));
        let library = cx.new(|cx| LibraryPage::new(model.clone(), window, cx));
        let artwork = cx.new(|_| Thumbnails::new(ARTWORK_KEPT));
        let downloads = cx.new(|cx| DownloadsPage::new(model.clone(), cx));
        let settings = cx.new(|cx| SettingsPage::new(model.clone(), window, cx));
        let subscriptions = vec![
            cx.observe_in(&model, window, |root, _, window, cx| {
                root.model_changed(window, cx)
            }),
            cx.subscribe_in(
                &library,
                window,
                |root, _, e: &LibraryEvent, window, cx| match e {
                    LibraryEvent::Open(id) => root.open_app(*id, window, cx),
                },
            ),
        ];
        self.connection = Connection::Up(Box::new(Ui {
            model,
            login,
            library,
            app: None,
            artwork,
            downloads,
            settings,
            _subscriptions: subscriptions,
        }));
        self.page = Page::Library;
        self.adding_account = false;
        // A new Login page, whose first field has not had focus yet.
        self.showed_login = false;
        self.model_changed(window, cx);
    }

    fn ui(&self) -> Option<&Ui> {
        match &self.connection {
            Connection::Up(ui) => Some(ui),
            _ => None,
        }
    }

    fn showing_login(&self, cx: &App) -> bool {
        self.ui().is_some_and(|ui| {
            self.page == Page::Library
                && (self.adding_account || ui.model.read(cx).store.needs_login())
        })
    }

    fn model_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Connection::Up(ui) = &mut self.connection else {
            return;
        };
        let store = &ui.model.read(cx).store;
        if self.adding_account && login::logged_on(store).is_some() {
            self.adding_account = false;
        }
        // An App page whose details went with the account closes.
        if let Some(app) = &ui.app {
            let id = app.read(cx).app_id();
            if !store.apps.contains_key(&id) {
                ui.app = None;
                window.focus(&ui.library.focus_handle(cx), cx);
            }
        }
        let showing = self.showing_login(cx);
        if showing
            && !self.showed_login
            && let Some(ui) = self.ui()
        {
            let field = ui.login.read(cx).first_field(cx);
            window.focus(&field, cx);
        }
        self.showed_login = showing;
        cx.notify();
    }

    fn open_app(&mut self, app_id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Connection::Up(ui) = &mut self.connection else {
            return;
        };
        if let Some(old) = ui.app.take() {
            let id = old.read(cx).app_id();
            ui.model.update(cx, |m, _| m.close_app(id));
        }
        let model = ui.model.clone();
        let artwork = ui.artwork.clone();
        let page = cx.new(|cx| AppPage::new(model, artwork, app_id, window, cx));
        let sub = cx.subscribe_in(
            &page,
            window,
            |root, _, e: &AppPageEvent, window, cx| match e {
                AppPageEvent::Close => root.close_app(window, cx),
            },
        );
        ui._subscriptions.push(sub);
        window.focus(&page.focus_handle(cx), cx);
        ui.app = Some(page);
        self.page = Page::Library;
        cx.notify();
    }

    fn close_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Connection::Up(ui) = &mut self.connection else {
            return;
        };
        if let Some(app) = ui.app.take() {
            let id = app.read(cx).app_id();
            ui.model.update(cx, |m, _| m.close_app(id));
        }
        window.focus(&ui.library.focus_handle(cx), cx);
        cx.notify();
    }

    fn show(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.menu_open = false;
        if let Some(ui) = self.ui() {
            let target = match page {
                Page::Library => match &ui.app {
                    Some(app) => app.focus_handle(cx),
                    None => ui.library.focus_handle(cx),
                },
                Page::Downloads => ui.downloads.focus_handle(cx),
                Page::Settings => ui.settings.focus_handle(cx),
            };
            window.focus(&target, cx);
        }
        self.model_changed(window, cx);
    }

    fn retry(&mut self, _: &Retry, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.connection, Connection::Down(_)) {
            self.connect(window, cx);
        }
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn show_library(&mut self, _: &ShowLibrary, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Page::Library, window, cx);
    }

    fn show_downloads(&mut self, _: &ShowDownloads, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Page::Downloads, window, cx);
    }

    fn show_settings(&mut self, _: &ShowSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Page::Settings, window, cx);
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        let Connection::Up(ui) = &mut self.connection else {
            return;
        };
        if let Some(app) = ui.app.take() {
            let id = app.read(cx).app_id();
            ui.model.update(cx, |m, _| m.close_app(id));
        }
        let library = ui.library.clone();
        self.page = Page::Library;
        if !self.showing_login(cx) {
            let search = library.read(cx).search_focus(cx);
            window.focus(&search, cx);
        }
        cx.notify();
    }

    /// Ctrl-Enter (§6): the open App page's main action, or the selected
    /// app's, through its App page.
    fn play_selected(&mut self, _: &PlaySelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Library || self.showing_login(cx) {
            return;
        }
        let Some(ui) = self.ui() else {
            return;
        };
        if let Some(app) = ui.app.clone() {
            app.update(cx, |page, cx| page.main_action(window, cx));
            return;
        }
        if let Some(id) = ui.library.read(cx).selected() {
            self.open_app(id, window, cx);
            if let Some(app) = self.ui().and_then(|ui| ui.app.clone()) {
                app.update(cx, |page, cx| page.main_action(window, cx));
            }
        }
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        let in_menu = self.menu_focus.contains_focused(window, cx);
        window.focus_next(cx);
        if in_menu && self.left_menu(window, cx) {
            window.focus_next(cx);
        }
        reveal::focus_moved(window, cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        let in_menu = self.menu_focus.contains_focused(window, cx);
        window.focus_prev(cx);
        if in_menu {
            self.left_menu(window, cx);
        }
        reveal::focus_moved(window, cx);
    }

    /// The menu is drawn over the window, after everything else, so Tab
    /// order puts it last: Tab out of it would wrap to the window's
    /// start, and Shift-Tab out of it reach the page's end. When focus
    /// has left it, the menu closes and focus goes back to the account
    /// button, from where Tab goes on.
    fn left_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu_focus.contains_focused(window, cx) {
            return false;
        }
        self.menu_open = false;
        window.focus(&self.account_focus, cx);
        cx.notify();
        true
    }

    fn close_menu(&mut self, _: &CloseMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        window.focus(&self.account_focus, cx);
        cx.notify();
    }

    fn toggle_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        if self.menu_open {
            window.focus(&self.menu_focus, cx);
        }
        cx.notify();
    }

    /// With `OSSTEAM_GUI_STARTUP_TIMING` set, prints the time from process
    /// start to the first presented frame, for the §20 budget. With the
    /// value `exit`, the GUI then quits.
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

    fn header(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let tab = |id: &'static str, label: &'static str, page: Page, cx: &mut Context<Self>| {
            button(id, label)
                .ghost()
                .selected(self.page == page)
                .on_click(cx.listener(move |root, _, window, cx| root.show(page, window, cx)))
        };
        let store = &ui.model.read(cx).store;
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
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(p.border)
            .bg(p.surface)
            .child(div().text_lg().mr_4().child("ossteam"))
            .child(tab("page-library", "Library", Page::Library, cx))
            .child(tab("page-downloads", "Downloads", Page::Downloads, cx))
            .child(tab("page-settings", "Settings", Page::Settings, cx))
            .child(div().flex_1())
            .child(
                div()
                    .child(
                        button("account-menu", account_label)
                            .focus(self.account_focus.clone())
                            .ghost()
                            .selected(self.menu_open)
                            .on_click(
                                cx.listener(|root, _, window, cx| root.toggle_menu(window, cx)),
                            ),
                    )
                    .when(self.menu_open, |d| d.child(self.account_menu(ui, cx))),
            )
            .into_any_element()
    }

    /// The account menu (§4.6).
    fn account_menu(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let store = &ui.model.read(cx).store;
        let active = store.account().map(|a| a.steam_id);
        let others: Vec<v1::Account> = store
            .accounts
            .accounts
            .iter()
            .filter(|a| Some(a.steam_id) != active)
            .cloned()
            .collect();
        let mut menu = div()
            .key_context("AccountMenu")
            .track_focus(&self.menu_focus)
            .on_action(cx.listener(Self::close_menu))
            .w(px(260.))
            .mt_1()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.surface)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|root, _, _, cx| {
                root.menu_open = false;
                cx.notify();
            }));
        if let Some(a) = store.account() {
            let state = if store.status.logged_on {
                format::persona_state(store.status.persona_state())
            } else {
                "not logged on"
            };
            menu = menu.child(
                div()
                    .px_2()
                    .pb_1()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().child(a.persona_name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(format!("{} · {state}", a.account_name)),
                    ),
            );
        }
        for a in others {
            let id = a.steam_id;
            let label = if a.persona_name.is_empty() {
                a.account_name.clone()
            } else {
                format!("{} ({})", a.persona_name, a.account_name)
            };
            let model = ui.model.clone();
            menu = menu.child(
                button(SharedString::from(format!("menu-account-{id}")), label)
                    .ghost()
                    .full_width()
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.menu_open = false;
                        model.update(cx, |m, cx| {
                            m.run(cx, move |c| c.switch_account(id), |_, _, _| {})
                        });
                    })),
            );
        }
        let model = ui.model.clone();
        menu = menu.child(
            button("menu-add-account", "Add account")
                .ghost()
                .full_width()
                .on_click(cx.listener(move |root, _, window, cx| {
                    model.update(cx, |m, cx| {
                        m.store.login_stopped(None);
                        cx.notify();
                    });
                    root.menu_open = false;
                    root.adding_account = true;
                    root.show(Page::Library, window, cx);
                })),
        );
        if store.account().is_some() {
            let model = ui.model.clone();
            menu = menu.child(
                button("menu-logout", "Log out")
                    .ghost()
                    .full_width()
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.menu_open = false;
                        model.update(cx, |m, cx| m.run(cx, |c| c.logout(), |_, (), _| {}));
                    })),
            );
        }
        deferred(anchored().snap_to_window_with_margin(px(8.)).child(menu))
            .priority(1)
            .into_any_element()
    }

    /// The status bar (§4.7).
    fn status_bar(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let store = &ui.model.read(cx).store;
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
                let more = q.len() - 1;
                if more > 0 {
                    format!("{} {name}{pct}, {more} more", format::kind(j.kind()))
                } else {
                    format!("{} {name}{pct}", format::kind(j.kind()))
                }
            }
            None if !q.is_empty() => format!("{} queued", q.len()),
            None => String::new(),
        };
        let mut games = div().flex().gap_2();
        for g in store.running.values() {
            let id = g.app_id;
            let name = store
                .app_name(id)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("App {id}"));
            let model = ui.model.clone();
            games = games.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_xs().child(format!("{name} running")))
                    .child(
                        button(("stop-game", id as usize), "Stop")
                            .danger()
                            .on_click(move |_, _, cx| {
                                model.update(cx, |m, cx| {
                                    m.run(cx, move |c| c.stop_game(id), |_, (), _| {})
                                });
                            }),
                    ),
            );
        }
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_1()
            .border_t_1()
            .border_color(p.border)
            .bg(p.surface)
            .text_xs()
            .text_color(p.muted)
            .child(connection)
            .child(account)
            .when(!jobs.is_empty(), |d| d.child(jobs))
            .child(div().flex_1())
            .child(games)
            .into_any_element()
    }

    /// Notices, newest last, over the bottom right corner (§2.4).
    fn notices(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        let notices = ui.model.read(cx).store.notices.clone();
        if notices.is_empty() {
            return None;
        }
        let mut list = div()
            .absolute()
            .right_4()
            .bottom_10()
            .w(px(360.))
            .flex()
            .flex_col()
            .gap_2();
        for n in notices {
            let color = match n.level {
                NoticeLevel::Error => p.error,
                NoticeLevel::Warning => p.warning,
                _ => p.text,
            };
            let model = ui.model.clone();
            let id = n.id;
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.surface)
                    .shadow_md()
                    .child(div().flex_1().text_sm().text_color(color).child(n.text))
                    .child(
                        button(("dismiss", id as usize), "Dismiss")
                            .ghost()
                            .on_click(move |_, _, cx| {
                                model.update(cx, |m, cx| m.dismiss_notice(id, cx));
                            }),
                    ),
            );
        }
        Some(list.into_any_element())
    }

    fn down(&self, reason: Option<&SharedString>, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let body =
            match reason {
                None => div()
                    .text_color(p.muted)
                    .child("Connecting to ossteamd…")
                    .into_any_element(),
                Some(reason) => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(p.error).child("Cannot reach ossteamd"))
                    .child(div().text_sm().text_color(p.muted).child(reason.clone()))
                    .child(div().flex().child(button("retry", "Retry").on_click(
                        cx.listener(|root, _, window, cx| root.retry(&Retry, window, cx)),
                    )))
                    .into_any_element(),
            };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
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
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.report_first_frame(window);
        let p = palette(cx);
        let content = match &self.connection {
            Connection::Connecting => self.down(None, cx),
            Connection::Down(reason) => {
                let reason = reason.clone();
                self.down(Some(&reason), cx)
            }
            Connection::Up(ui) => {
                let page: AnyElement = match self.page {
                    Page::Library if self.showing_login(cx) => ui.login.clone().into_any_element(),
                    Page::Library => match &ui.app {
                        Some(app) => app.clone().into_any_element(),
                        None => ui.library.clone().into_any_element(),
                    },
                    Page::Downloads => ui.downloads.clone().into_any_element(),
                    Page::Settings => ui.settings.clone().into_any_element(),
                };
                let header = self.header(ui, cx);
                let status = self.status_bar(ui, cx);
                let notices = self.notices(ui, cx);
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(page))
                    .child(status)
                    .when_some(notices, |d, n| d.child(n))
                    .into_any_element()
            }
        };
        div()
            .key_context("Root")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::retry))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::show_library))
            .on_action(cx.listener(Self::show_downloads))
            .on_action(cx.listener(Self::show_settings))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::play_selected))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .size_full()
            .bg(p.background)
            .text_color(p.text)
            .child(content)
    }
}
