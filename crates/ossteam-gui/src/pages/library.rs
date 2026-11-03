// SPDX-License-Identifier: GPL-2.0-or-later
//! The Library page (`gui.md` §4.2).

use std::rc::Rc;
use std::time::Duration;

use ossteam_ipc::v1::{self, AssetKind};
use ossteam_ui::element::{FocusHandle, ScrollHandle, image, list, panel, text};
use ossteam_ui::keys::KeyBinding;
use ossteam_ui::scene::Color;
use ossteam_ui::style::{Align, CursorIcon, Edges, Justify, Length, Styled};
use ossteam_ui::text_field::TextField;
use ossteam_ui::window::{Cx, Timer, ViewCx};

use crate::bench::LibraryBench;
use crate::gui::Gui;
use crate::model::{Asset, Model};
use crate::store::LibraryQuery;
use crate::theme::Palette;
use crate::thumbnails::{Fit, Thumbnail};
use crate::widgets::{self, El, SMALL, button, muted, switch, text_field};

const CONTEXT: &str = "Library";
/// The search waits this long after the last key press (§4.2).
const SEARCH_DELAY: Duration = Duration::from_millis(150);
/// Capsules are 600 × 900 images.
pub const CELL_WIDTH: f32 = 150.0;
pub const CELL_HEIGHT: f32 = 225.0;
pub const GAP: f32 = 16.0;
/// The page's padding, each side.
const PAD: f32 = 24.0;
/// The selection's border around a capsule.
const BORDER: f32 = 2.0;
pub const LIST_ROW: f32 = 36.0;
/// The rows Page Up and Page Down move by.
const PAGE_ROWS: usize = 4;

#[derive(Clone, Debug)]
struct SelectLeft;
#[derive(Clone, Debug)]
struct SelectRight;
#[derive(Clone, Debug)]
struct SelectUp;
#[derive(Clone, Debug)]
struct SelectDown;
#[derive(Clone, Debug)]
struct SelectFirst;
#[derive(Clone, Debug)]
struct SelectLast;
#[derive(Clone, Debug)]
struct PageUp;
#[derive(Clone, Debug)]
struct PageDown;
#[derive(Clone, Debug)]
struct Open;

pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some(CONTEXT);
    vec![
        KeyBinding::new("left", SelectLeft, ctx),
        KeyBinding::new("right", SelectRight, ctx),
        KeyBinding::new("up", SelectUp, ctx),
        KeyBinding::new("down", SelectDown, ctx),
        KeyBinding::new("home", SelectFirst, ctx),
        KeyBinding::new("end", SelectLast, ctx),
        KeyBinding::new("page_up", PageUp, ctx),
        KeyBinding::new("page_down", PageDown, ctx),
        KeyBinding::new("enter", Open, ctx),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Grid,
    List,
}

pub struct LibraryPage {
    search: TextField,
    installed_only: bool,
    updates_only: bool,
    pub layout: Layout,
    /// The selected app, for the arrows, Enter and Ctrl-Enter (§6).
    pub selected: Option<u32>,
    /// The grid's and the list's: one scroll for whichever is shown.
    pub scroll: ScrollHandle,
    search_timer: Option<Timer>,
    /// The library scroll of `OSSTEAM_GUI_BENCH` (`gui.md` §7).
    pub bench: Option<LibraryBench>,
}

impl Default for LibraryPage {
    fn default() -> Self {
        Self {
            search: TextField::new("Search the library", widgets::TEXT),
            installed_only: false,
            updates_only: false,
            layout: Layout::Grid,
            selected: None,
            scroll: ScrollHandle::new(),
            search_timer: None,
            bench: LibraryBench::wanted(),
        }
    }
}

/// The grid or list; arrows move the selection while it has focus.
pub fn items_focus() -> FocusHandle {
    FocusHandle::named("library-items")
}

pub fn search_focus() -> FocusHandle {
    FocusHandle::named("library-search")
}

pub fn before_view(g: &mut Gui, cx: &mut Cx<Gui>) {
    crate::bench::library_before_view(g, cx);
}

fn query_now(g: &mut Gui) {
    g.library.search_timer = None;
    let q = LibraryQuery {
        filter: g.library.search.text().to_owned(),
        installed_only: g.library.installed_only,
    };
    if let Some(m) = g.model_mut() {
        m.set_library_query(q);
    }
}

/// The apps shown: indexes into `ListApps`, less those the Updates filter
/// drops.
fn shown(g: &Gui, lib: &v1::ListAppsResponse) -> Rc<Vec<usize>> {
    let only = g.library.updates_only;
    Rc::new(
        lib.apps
            .iter()
            .enumerate()
            .filter(|(_, a)| !only || a.update_available)
            .map(|(i, _)| i)
            .collect(),
    )
}

/// Columns of the grid in a window `width` logical pixels wide.
pub fn columns(width: f32) -> usize {
    let width = width - 2.0 * PAD;
    ((width + GAP) / (CELL_WIDTH + GAP)).floor().max(1.0) as usize
}

fn move_selection(g: &mut Gui, cx: &mut Cx<Gui>, f: impl Fn(usize, usize, usize) -> usize) {
    let Some(lib) = g.model().and_then(|m| m.store.library.clone()) else {
        return;
    };
    let apps = shown(g, &lib);
    if apps.is_empty() {
        return;
    }
    let cols = match g.library.layout {
        Layout::Grid => columns(cx.viewport().0),
        Layout::List => 1,
    };
    let current = g
        .library
        .selected
        .and_then(|id| apps.iter().position(|&i| lib.apps[i].app_id == id));
    let next = match current {
        Some(i) => f(i, cols, apps.len()).min(apps.len() - 1),
        None => 0,
    };
    g.library.selected = Some(lib.apps[apps[next]].app_id);
    g.library.scroll.reveal_row(next / cols);
}

fn open_selected(g: &mut Gui, cx: &mut Cx<Gui>) {
    if let Some(id) = g.library.selected {
        g.open_app(id, cx);
    }
}

fn open(g: &mut Gui, cx: &mut Cx<Gui>, app_id: u32) {
    g.library.selected = Some(app_id);
    g.open_app(app_id, cx);
}

fn toolbar(g: &Gui, p: Palette, cx: &ViewCx) -> El {
    let page = &g.library;
    panel()
        .row()
        .align(Align::Center)
        .gap(12.0)
        .child(
            text_field(p, cx, search_focus(), |g: &mut Gui| &mut g.library.search)
                .width(280.0)
                .on_change(|g, cx| {
                    g.library.search_timer = Some(cx.after(SEARCH_DELAY, |g, _| query_now(g)));
                })
                .on_submit(|g, cx| {
                    query_now(g);
                    cx.focus(items_focus());
                }),
        )
        .child(
            switch(p, "installed-only", "Installed", page.installed_only).on_toggle(|g, _| {
                g.library.installed_only = !g.library.installed_only;
                query_now(g);
            }),
        )
        .child(
            switch(p, "updates-only", "Updates", page.updates_only)
                .on_toggle(|g, _| g.library.updates_only = !g.library.updates_only),
        )
        .child(panel().grow())
        .child(
            button(p, "layout-grid", "Grid")
                .ghost()
                .selected(page.layout == Layout::Grid)
                .on_click(|g, _| g.library.layout = Layout::Grid),
        )
        .child(
            button(p, "layout-list", "List")
                .ghost()
                .selected(page.layout == Layout::List)
                .on_click(|g, _| g.library.layout = Layout::List),
        )
}

/// The loading state and packages without info (§4.2).
fn status_line(lib: &v1::ListAppsResponse, p: Palette) -> Option<El> {
    let mut parts = panel().row().gap(12.0);
    let mut any = false;
    if !lib.complete {
        parts = parts.child(muted(p, "The library is still loading."));
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
        let words = format!("{n} package{} without info", if n == 1 { "" } else { "s" });
        parts = parts.child(
            muted(p, words)
                .id("packages-without-info")
                .tooltip(format!("Packages: {ids}")),
        );
        any = true;
    }
    any.then_some(parts)
}

/// An app's name, or its id while its info is not available.
fn name(a: &v1::AppSummary) -> String {
    if a.name.is_empty() {
        format!("App {}", a.app_id)
    } else {
        a.name.clone()
    }
}

fn capsule(g: &Gui, m: &Model, a: &v1::AppSummary, selected: bool, p: Palette, scale: f32) -> El {
    let id = a.app_id;
    let asset = if a.has_info {
        m.asset(id, AssetKind::Capsule, scale > 1.0)
    } else {
        Asset::Missing
    };
    // The image fills the cell inside its border.
    let inner = (CELL_WIDTH - 2.0 * BORDER, CELL_HEIGHT - 2.0 * BORDER);
    let image_now = match asset {
        Asset::Ready(path) => {
            let size = (
                (inner.0 * scale).round() as u32,
                (inner.1 * scale).round() as u32,
            );
            g.capsules.get(&path, size, Fit::Cover)
        }
        Asset::Waiting => Thumbnail::Loading,
        Asset::Missing => Thumbnail::Failed,
    };
    let body = match image_now {
        Thumbnail::Ready(img) => image(img).radius(4.0),
        Thumbnail::Loading | Thumbnail::Failed => panel()
            .fill()
            .pad(8.0)
            .justify(Justify::End)
            .font_size(widgets::TEXT)
            .child(text(name(a))),
    };
    panel()
        .id(("capsule", u64::from(id)))
        .size(CELL_WIDTH, CELL_HEIGHT)
        .no_shrink()
        .radius(6.0)
        .clip()
        .background(p.surface_raised)
        .border(
            BORDER,
            if selected {
                p.focus_ring
            } else {
                Color::TRANSPARENT
            },
        )
        .cursor(CursorIcon::Pointer)
        .on_click(move |g, cx| open(g, cx, id))
        .child(body)
        .when(a.update_available, |d| {
            d.child(
                text("Update")
                    .absolute()
                    .inset(Edges {
                        top: Length::Px(4.0),
                        right: Length::Px(4.0),
                        ..Edges::all(Length::Auto)
                    })
                    .pad_xy(4.0, 0.0)
                    .radius(3.0)
                    .background(p.accent)
                    .text_color(p.on_accent)
                    .font_size(SMALL),
            )
        })
}

fn grid(apps: Rc<Vec<usize>>, cols: usize, scroll: &ScrollHandle) -> El {
    let rows = apps.len().div_ceil(cols);
    list(scroll, rows, move |g: &Gui, row, cx: &ViewCx| {
        let p = Palette::for_view(cx);
        let mut r = panel().row().gap(GAP).padding(Edges {
            bottom: GAP,
            ..Edges::default()
        });
        let (Some(m), Some(lib)) = (g.model(), g.model().and_then(|m| m.store.library.as_ref()))
        else {
            return r;
        };
        for &i in &apps[row * cols..((row + 1) * cols).min(apps.len())] {
            let a = &lib.apps[i];
            let selected = g.library.selected == Some(a.app_id);
            r = r.child(capsule(g, m, a, selected, p, cx.scale));
        }
        r
    })
    .fill()
}

fn app_list(apps: Rc<Vec<usize>>, scroll: &ScrollHandle) -> El {
    list(scroll, apps.len(), move |g: &Gui, i, cx: &ViewCx| {
        let p = Palette::for_view(cx);
        let Some(a) = g
            .model()
            .and_then(|m| m.store.library.as_ref())
            .and_then(|lib| lib.apps.get(apps[i]))
        else {
            return panel().height(LIST_ROW);
        };
        let id = a.app_id;
        let state = if a.update_available {
            "update available"
        } else if a.installed {
            "installed"
        } else {
            ""
        };
        let hover = p.surface_raised;
        panel()
            .id(("library-row", u64::from(id)))
            .height(LIST_ROW)
            .row()
            .align(Align::Center)
            .gap(12.0)
            .pad_xy(8.0, 0.0)
            .radius(6.0)
            .cursor(CursorIcon::Pointer)
            .when(g.library.selected == Some(id), |d| {
                d.background(p.selection)
            })
            .hover_style(move |s| s.background(hover))
            .on_click(move |g, cx| open(g, cx, id))
            .child(text(name(a)).grow().nowrap().font_size(widgets::TEXT))
            .child(text(state).font_size(SMALL).text_color(p.muted))
    })
    .fill()
}

pub fn view(g: &Gui, m: &Model, p: Palette, cx: &ViewCx) -> El {
    let lib = m.store.library.clone();
    let body: El = match &lib {
        None => muted(p, "Reading the library…"),
        Some(lib) => {
            let apps = shown(g, lib);
            if apps.is_empty() {
                muted(p, "No apps match.")
            } else {
                match g.library.layout {
                    Layout::Grid => grid(apps, columns(cx.width), &g.library.scroll),
                    Layout::List => app_list(apps, &g.library.scroll),
                }
            }
        }
    };
    let status = lib.as_deref().and_then(|l| status_line(l, p));
    let ring = p.focus_ring;
    panel()
        .fill()
        .gap(12.0)
        .pad(PAD)
        .child(toolbar(g, p, cx))
        .when_some(status, |d, s| d.child(s))
        .child(
            panel()
                .id("library-items")
                .focus(items_focus())
                .tab_stop(true)
                .key_context(CONTEXT)
                .on_action(|g: &mut Gui, _: &SelectLeft, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, _, _| i.saturating_sub(1))
                })
                .on_action(|g: &mut Gui, _: &SelectRight, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, _, _| i + 1)
                })
                .on_action(|g: &mut Gui, _: &SelectUp, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, cols, _| i.saturating_sub(cols))
                })
                .on_action(|g: &mut Gui, _: &SelectDown, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, cols, n| if i + cols < n { i + cols } else { i })
                })
                .on_action(|g: &mut Gui, _: &SelectFirst, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |_, _, _| 0)
                })
                .on_action(|g: &mut Gui, _: &SelectLast, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |_, _, n| n - 1)
                })
                .on_action(|g: &mut Gui, _: &PageUp, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, cols, _| i.saturating_sub(cols * PAGE_ROWS))
                })
                .on_action(|g: &mut Gui, _: &PageDown, cx: &mut Cx<Gui>| {
                    move_selection(g, cx, |i, cols, _| i + cols * PAGE_ROWS)
                })
                .on_action(|g: &mut Gui, _: &Open, cx: &mut Cx<Gui>| open_selected(g, cx))
                .grow()
                .min_height(0.0)
                .radius(6.0)
                .border(2.0, Color::TRANSPARENT)
                .focus_style(move |s| s.border_color(ring))
                .child(body),
        )
        .when_some(
            g.library.bench.as_ref().map(LibraryBench::painted),
            |d, painted| d.child(painted),
        )
}

/// The capsules a library shows, in the order shown, for the bench.
pub fn shown_count(g: &Gui) -> usize {
    g.model()
        .and_then(|m| m.store.library.as_ref())
        .map_or(0, |lib| shown(g, lib).len())
}

/// The scroll's step for one row of the layout shown.
pub fn row_height(layout: Layout) -> f32 {
    match layout {
        Layout::Grid => CELL_HEIGHT + GAP,
        Layout::List => LIST_ROW,
    }
}
