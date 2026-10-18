// SPDX-License-Identifier: GPL-2.0-or-later
//! The Library page (`gui.md` §4.2).

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ObjectFit,
    ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
    actions, div, img, prelude::*, px, uniform_list,
};
use ossteam_ipc::v1::{self, AssetKind};

use crate::model::{Asset, Model};
use crate::pages::muted;
use crate::store::LibraryQuery;
use crate::theme::palette;
use crate::widgets::{TextInput, TextInputEvent, button, switch};

actions!(
    library,
    [
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectFirst,
        SelectLast,
        PageUp,
        PageDown,
        Open
    ]
);

const CONTEXT: &str = "Library";
/// The search waits this long after the last key press (§4.2).
const SEARCH_DELAY: Duration = Duration::from_millis(150);
/// Capsules are 600 × 900 images.
const CELL_WIDTH: f32 = 150.;
const CELL_HEIGHT: f32 = 225.;
const GAP: f32 = 16.;
const LIST_ROW: f32 = 36.;
/// The rows Page Up and Page Down move by.
const PAGE_ROWS: usize = 4;

pub fn bind_keys(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", SelectLeft, ctx),
        KeyBinding::new("right", SelectRight, ctx),
        KeyBinding::new("up", SelectUp, ctx),
        KeyBinding::new("down", SelectDown, ctx),
        KeyBinding::new("home", SelectFirst, ctx),
        KeyBinding::new("end", SelectLast, ctx),
        KeyBinding::new("pageup", PageUp, ctx),
        KeyBinding::new("pagedown", PageDown, ctx),
        KeyBinding::new("enter", Open, ctx),
    ]);
}

pub enum LibraryEvent {
    Open(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Grid,
    List,
}

pub struct LibraryPage {
    model: Entity<Model>,
    /// The grid or list; arrows move the selection while it has focus.
    focus: FocusHandle,
    search: Entity<TextInput>,
    installed_only: bool,
    updates_only: bool,
    layout: Layout,
    selected: Option<u32>,
    /// Columns of the grid as last drawn.
    columns: usize,
    scroll: UniformListScrollHandle,
    search_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for LibraryPage {}

impl LibraryPage {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = TextInput::build("Search the library", window, cx);
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |page, _, e, window, cx| match e {
                TextInputEvent::Changed => page.search_later(cx),
                TextInputEvent::Submitted => {
                    page.query_now(cx);
                    window.focus(&page.focus, cx);
                }
                TextInputEvent::Blurred => {}
            }),
        ];
        Self {
            model,
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            search,
            installed_only: false,
            updates_only: false,
            layout: Layout::Grid,
            selected: None,
            columns: 1,
            scroll: UniformListScrollHandle::new(),
            search_task: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn search_focus(&self, cx: &App) -> FocusHandle {
        self.search.focus_handle(cx)
    }

    /// The selected app, for Ctrl-Enter (§6).
    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    fn query(&self, cx: &App) -> LibraryQuery {
        LibraryQuery {
            filter: self.search.read(cx).text().to_owned(),
            installed_only: self.installed_only,
        }
    }

    fn search_later(&mut self, cx: &mut Context<Self>) {
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let _ = this.update(cx, |page, cx| page.query_now(cx));
        }));
    }

    fn query_now(&mut self, cx: &mut Context<Self>) {
        self.search_task = None;
        let q = self.query(cx);
        self.model.update(cx, |m, cx| m.set_library_query(q, cx));
    }

    /// The apps shown: top-level apps of `ListApps`, less those the
    /// Updates filter drops.
    fn apps(&self, cx: &App) -> Vec<v1::AppSummary> {
        let Some(lib) = &self.model.read(cx).store.library else {
            return Vec::new();
        };
        lib.apps
            .iter()
            .filter(|a| !self.updates_only || a.update_available)
            .cloned()
            .collect()
    }

    fn open(&mut self, app_id: u32, cx: &mut Context<Self>) {
        self.selected = Some(app_id);
        cx.emit(LibraryEvent::Open(app_id));
    }

    fn move_selection(&mut self, f: impl Fn(usize, usize, usize) -> usize, cx: &mut Context<Self>) {
        let apps = self.apps(cx);
        if apps.is_empty() {
            return;
        }
        let cols = match self.layout {
            Layout::Grid => self.columns.max(1),
            Layout::List => 1,
        };
        let current = self
            .selected
            .and_then(|id| apps.iter().position(|a| a.app_id == id));
        let next = match current {
            Some(i) => f(i, cols, apps.len()).min(apps.len() - 1),
            None => 0,
        };
        self.selected = Some(apps[next].app_id);
        self.scroll
            .scroll_to_item(next / cols, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, _, _| i.saturating_sub(1), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, _, _| i + 1, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, cols, _| i.saturating_sub(cols), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, cols, n| if i + cols < n { i + cols } else { i }, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|_, _, _| 0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|_, _, n| n - 1, cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, cols, _| i.saturating_sub(cols * PAGE_ROWS), cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(|i, cols, _| i + cols * PAGE_ROWS, cx);
    }

    fn open_selected(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected {
            self.open(id, cx);
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().w(px(280.)).child(self.search.clone()))
            .child(
                switch("installed-only", "Installed", self.installed_only).on_toggle(cx.listener(
                    |page, _, _, cx| {
                        page.installed_only = !page.installed_only;
                        page.query_now(cx);
                    },
                )),
            )
            .child(
                switch("updates-only", "Updates", self.updates_only).on_toggle(cx.listener(
                    |page, _, _, cx| {
                        page.updates_only = !page.updates_only;
                        cx.notify();
                    },
                )),
            )
            .child(div().flex_1())
            .child(
                button("layout-grid", "Grid")
                    .ghost()
                    .selected(self.layout == Layout::Grid)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.layout = Layout::Grid;
                        cx.notify();
                    })),
            )
            .child(
                button("layout-list", "List")
                    .ghost()
                    .selected(self.layout == Layout::List)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.layout = Layout::List;
                        cx.notify();
                    })),
            )
    }

    /// The loading state and packages without info (§4.2).
    fn status_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.model.read(cx).store.library.as_ref()?;
        let mut parts = div().flex().gap_3();
        let mut any = false;
        if !lib.complete {
            parts = parts.child(muted("The library is still loading.", cx));
            any = true;
        }
        if !lib.packages_without_info.is_empty() {
            let ids = lib
                .packages_without_info
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let n = lib.packages_without_info.len();
            let text = format!("{n} package{} without info", if n == 1 { "" } else { "s" });
            parts = parts.child(
                div()
                    .id("packages-without-info")
                    .child(muted(text, cx))
                    .tooltip(move |_, cx| {
                        let ids = ids.clone();
                        cx.new(|_| Tooltip(format!("Packages: {ids}").into()))
                            .into()
                    }),
            );
            any = true;
        }
        any.then(|| parts.into_any_element())
    }

    fn grid(
        &mut self,
        apps: Rc<Vec<v1::AppSummary>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = f32::from(window.viewport_size().width) - 48.;
        let cols = ((width + GAP) / (CELL_WIDTH + GAP)).floor().max(1.) as usize;
        self.columns = cols;
        let rows = apps.len().div_ceil(cols);
        let model = self.model.clone();
        let page = cx.entity().downgrade();
        let selected = self.selected;
        let high_dpi = window.scale_factor() > 1.;
        uniform_list("library-grid", rows, move |range, _, cx| {
            range
                .map(|row| {
                    let mut r = div().flex().gap(px(GAP)).pb(px(GAP));
                    for app in apps.iter().skip(row * cols).take(cols) {
                        r = r.child(capsule(app, selected, high_dpi, &model, &page, cx));
                    }
                    r
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }

    fn list(&mut self, apps: Rc<Vec<v1::AppSummary>>, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity().downgrade();
        let selected = self.selected;
        uniform_list("library-list", apps.len(), move |range, _, cx| {
            let p = palette(cx);
            range
                .map(|i| {
                    let a = &apps[i];
                    let id = a.app_id;
                    let page = page.clone();
                    let state = if a.update_available {
                        "update available"
                    } else if a.installed {
                        "installed"
                    } else {
                        ""
                    };
                    div()
                        .id(("library-row", id as usize))
                        .h(px(LIST_ROW))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected == Some(id), |d| d.bg(p.selection))
                        .hover(move |s| s.bg(p.surface_raised))
                        .on_click(move |_, _, cx| {
                            let _ = page.update(cx, |page, cx| page.open(id, cx));
                        })
                        .child(div().flex_1().text_sm().child(name(a)))
                        .child(div().text_xs().text_color(p.muted).child(state))
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }
}

/// An app's name, or its id while its info is not available.
fn name(a: &v1::AppSummary) -> SharedString {
    if a.name.is_empty() {
        format!("App {}", a.app_id).into()
    } else {
        a.name.clone().into()
    }
}

fn capsule(
    app: &v1::AppSummary,
    selected: Option<u32>,
    high_dpi: bool,
    model: &Entity<Model>,
    page: &WeakEntity<LibraryPage>,
    cx: &mut App,
) -> AnyElement {
    let p = palette(cx);
    let id = app.app_id;
    let asset = if app.has_info {
        model.update(cx, |m, cx| {
            m.want_asset(id, AssetKind::Capsule, high_dpi, cx);
            m.asset(id, AssetKind::Capsule).cloned()
        })
    } else {
        Some(Asset::Missing)
    };
    let label = name(app);
    let fallback = {
        let label = label.clone();
        move || {
            div()
                .size_full()
                .p_2()
                .flex()
                .items_end()
                .text_sm()
                .child(label.clone())
                .into_any_element()
        }
    };
    let page = page.clone();
    div()
        .id(("capsule", id as usize))
        .relative()
        .w(px(CELL_WIDTH))
        .h(px(CELL_HEIGHT))
        .flex_none()
        .rounded_md()
        .overflow_hidden()
        .bg(p.surface_raised)
        .border_2()
        .border_color(if selected == Some(id) {
            p.focus_ring
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            let _ = page.update(cx, |page, cx| page.open(id, cx));
        })
        .child(match asset {
            Some(Asset::Ready(path)) => img(path)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(fallback)
                .into_any_element(),
            _ => fallback(),
        })
        .when(app.update_available, |d| {
            d.child(
                div()
                    .absolute()
                    .top_1()
                    .right_1()
                    .px_1()
                    .rounded_sm()
                    .bg(p.accent)
                    .text_color(p.on_accent)
                    .text_xs()
                    .child("Update"),
            )
        })
        .into_any_element()
}

pub struct Tooltip(pub SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .max_w(px(360.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.surface)
            .text_xs()
            .text_color(p.text)
            .child(self.0.clone())
    }
}

impl Focusable for LibraryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LibraryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let apps = Rc::new(self.apps(cx));
        let has_library = self.model.read(cx).store.library.is_some();
        let empty = apps.is_empty();
        let ring = p.focus_ring;
        let status = self.status_line(cx);
        let body = if !has_library {
            muted("Reading the library…", cx).into_any_element()
        } else if empty {
            muted("No apps match.", cx).into_any_element()
        } else {
            match self.layout {
                Layout::Grid => self.grid(apps, window, cx),
                Layout::List => self.list(apps, cx),
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .child(self.toolbar(cx))
            .when_some(status, |d, s| d.child(s))
            .child(
                div()
                    .id("library-items")
                    .key_context(CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(cx.listener(Self::select_left))
                    .on_action(cx.listener(Self::select_right))
                    .on_action(cx.listener(Self::select_up))
                    .on_action(cx.listener(Self::select_down))
                    .on_action(cx.listener(Self::select_first))
                    .on_action(cx.listener(Self::select_last))
                    .on_action(cx.listener(Self::page_up))
                    .on_action(cx.listener(Self::page_down))
                    .on_action(cx.listener(Self::open_selected))
                    .flex_1()
                    .min_h_0()
                    .rounded_md()
                    .border_2()
                    .border_color(gpui::transparent_black())
                    .focus_visible(move |s| s.border_color(ring))
                    .child(body),
            )
    }
}
