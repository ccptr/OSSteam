// SPDX-License-Identifier: GPL-2.0-or-later
//! The App page (`gui.md` §4.3): artwork, the main action and its
//! choosers, details, compatibility, managing the install, and the last
//! run.

use std::path::Path;
use std::sync::Arc;

use ossteam_ipc::v1::{self, AssetKind, CompatSource};
use ossteam_ipc::{Client, ClientError};
use ossteam_ui::element::{FocusHandle, ScrollHandle, image, panel, text};
use ossteam_ui::image::Image;
use ossteam_ui::keys::KeyBinding;
use ossteam_ui::style::{Align, Edges, Length, Styled};
use ossteam_ui::text_field::TextField;
use ossteam_ui::window::{Cx, ViewCx};

use crate::args;
use crate::daemon::CallError;
use crate::format;
use crate::gui::Gui;
use crate::model::{Asset, Model};
use crate::pages::downloads::job_row;
use crate::store::Read;
use crate::theme::Palette;
use crate::thumbnails::{Fit, Thumbnail};
use crate::widgets::{
    self, El, SMALL, button, choice, error_text, group, muted, row, scrolling_page, section,
    switch, text_field,
};

const CONTEXT: &str = "AppPage";

/// Artwork kept decoded, for every App page together (§4.3).
pub const ARTWORK_KEPT: usize = 8;
const HERO_HEIGHT: f32 = 240.0;
/// The hero is decoded for the window's width rounded up to this, so
/// that resizing the window does not decode it again at every step.
const HERO_WIDTH_STEP: u32 = 64;
const LOGO_BOX: (f32, f32) = (320.0, 140.0);
const HEADER_SIZE: (f32, f32) = (460.0, 215.0);

#[derive(Clone, Debug)]
struct Back;

pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("escape", Back, Some(CONTEXT))]
}

/// Which inline chooser is open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chooser {
    None,
    Install,
    Launch,
    Uninstall,
}

pub struct AppPage {
    pub app_id: u32,
    scroll: ScrollHandle,
    open: Chooser,
    /// What had focus when the chooser opened, to get it back when the
    /// chooser closes.
    opener: Option<FocusHandle>,
    folder: Option<String>,
    branch: Option<String>,
    option: Option<u32>,
    allow_stale: bool,
    args: TextField,
    remove_compat_data: bool,
    error: Option<String>,
    compat_error: Option<String>,
    /// Turning an optional DLC on or off failed.
    dlc_error: Option<String>,
    busy: bool,
    /// Ctrl-Enter came before the details: act once they arrive.
    pending_main_action: bool,
}

impl AppPage {
    pub fn new(app_id: u32) -> Self {
        Self {
            app_id,
            scroll: ScrollHandle::new(),
            open: Chooser::None,
            opener: None,
            folder: None,
            branch: None,
            option: None,
            allow_stale: false,
            args: TextField::new("Arguments", widgets::TEXT),
            remove_compat_data: false,
            error: None,
            compat_error: None,
            dlc_error: None,
            busy: false,
            pending_main_action: false,
        }
    }
}

pub fn page_focus() -> FocusHandle {
    FocusHandle::named("app-page")
}

fn args_focus() -> FocusHandle {
    FocusHandle::named("app-args")
}

/// The App page of `app_id`, if it is still the one open.
fn page(g: &mut Gui, app_id: u32) -> Option<&mut AppPage> {
    g.app.as_mut().filter(|a| a.app_id == app_id)
}

fn info(m: &Model, app_id: u32) -> Option<&v1::GetAppResponse> {
    m.store.apps.get(&app_id)?.info.as_ref()?.as_ref().ok()
}

fn options(m: &Model, app_id: u32) -> Option<&v1::GetLaunchOptionsResponse> {
    m.store.apps.get(&app_id)?.options.as_ref()?.as_ref().ok()
}

/// Ctrl-Enter that came before the details acts once they arrive.
pub fn before_view(g: &mut Gui, cx: &mut Cx<Gui>) {
    let Some(app) = &g.app else {
        return;
    };
    let id = app.app_id;
    if app.pending_main_action && g.model().is_some_and(|m| info(m, id).is_some()) {
        if let Some(app) = &mut g.app {
            app.pending_main_action = false;
        }
        main_action(g, cx);
    }
}

/// Opens a chooser, remembering what had focus.
fn open_chooser(g: &mut Gui, which: Chooser, cx: &mut Cx<Gui>) {
    let Some(app) = &mut g.app else {
        return;
    };
    if app.open != which {
        app.open = which;
        app.opener = cx.focused();
    }
}

/// Closes the chooser, by Escape, Cancel or its call: focus that was in
/// the page goes back to what opened it.
fn close_chooser(g: &mut Gui, app_id: u32, cx: &mut Cx<Gui>) {
    let Some(app) = page(g, app_id) else {
        return;
    };
    app.open = Chooser::None;
    if let Some(opener) = app.opener.take()
        && cx.contains_focused(page_focus())
    {
        // With the ring it had, if the keyboard put focus in the chooser.
        if cx.focus_visible() {
            cx.focus_from_keyboard(opener);
        } else {
            cx.focus(opener);
        }
    }
}

fn back(g: &mut Gui, cx: &mut Cx<Gui>) {
    let Some(app) = &g.app else {
        return;
    };
    if app.open == Chooser::None {
        g.back_to_library(cx);
    } else {
        close_chooser(g, app.app_id, cx);
    }
}

/// Ctrl-Enter (§6): plays, updates, or opens and then confirms the
/// install chooser, as the page's main button would.
pub fn main_action(g: &mut Gui, cx: &mut Cx<Gui>) {
    let (Some(m), Some(app)) = (g.model(), &g.app) else {
        return;
    };
    let id = app.app_id;
    let Some(info) = info(m, id) else {
        if let Some(app) = &mut g.app {
            app.pending_main_action = true;
        }
        return;
    };
    if m.store.running.contains_key(&id) || m.store.unfinished_job(id).is_some() {
        return;
    }
    match &info.installed {
        None if app.open == Chooser::Install => install(g, cx),
        None => open_chooser(g, Chooser::Install, cx),
        Some(i) if i.update_available => update(g),
        Some(_) => play(g, cx),
    }
}

/// Runs a call whose error shows by the main action; one at a time.
fn act<T: Send + 'static>(
    g: &mut Gui,
    f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    done: impl FnOnce(&mut Gui, u32, T, &mut Cx<Gui>) + Send + 'static,
) {
    let Some(app) = &mut g.app else {
        return;
    };
    if app.busy {
        return;
    }
    app.busy = true;
    app.error = None;
    let id = app.app_id;
    g.call(f, move |g, r, cx| {
        let Some(app) = page(g, id) else {
            return;
        };
        app.busy = false;
        match r {
            Ok(v) => done(g, id, v, cx),
            Err(e) => app.error = Some(e.message),
        }
    });
}

fn add_jobs(g: &mut Gui, jobs: Vec<v1::Job>) {
    if let Some(m) = g.model_mut() {
        for j in jobs {
            m.store.add_job(j);
        }
    }
}

fn install(g: &mut Gui, _cx: &mut Cx<Gui>) {
    let Some(app) = &g.app else {
        return;
    };
    let id = app.app_id;
    let folder = app.folder.clone().unwrap_or_default();
    let branch = app.branch.clone().unwrap_or_default();
    act(
        g,
        move |c| c.install(id, &folder, &branch),
        |g, id, job, cx| {
            close_chooser(g, id, cx);
            add_jobs(g, vec![job]);
        },
    );
}

fn update(g: &mut Gui) {
    let Some(id) = g.app.as_ref().map(|a| a.app_id) else {
        return;
    };
    act(
        g,
        move |c| c.update(&[id], false),
        |g, _, jobs, _| add_jobs(g, jobs),
    );
}

fn verify(g: &mut Gui) {
    let Some(id) = g.app.as_ref().map(|a| a.app_id) else {
        return;
    };
    act(
        g,
        move |c| c.verify(id),
        |g, _, job, _| add_jobs(g, vec![job]),
    );
}

fn uninstall(g: &mut Gui) {
    let Some(app) = &g.app else {
        return;
    };
    let (id, remove) = (app.app_id, app.remove_compat_data);
    act(
        g,
        move |c| c.uninstall(id, remove),
        |g, id, job, cx| {
            close_chooser(g, id, cx);
            add_jobs(g, vec![job]);
        },
    );
}

fn stop(g: &mut Gui) {
    let Some(id) = g.app.as_ref().map(|a| a.app_id) else {
        return;
    };
    act(g, move |c| c.stop_game(id), |_, _, (), _| {});
}

/// Play: opens the chooser when several options qualify, then launches
/// the chosen one; otherwise launches, and when none qualifies shows why
/// (§4.3).
fn play(g: &mut Gui, cx: &mut Cx<Gui>) {
    let (Some(m), Some(app)) = (g.model(), &g.app) else {
        return;
    };
    let id = app.app_id;
    let qualifying: Vec<u32> = options(m, id)
        .map(|o| {
            o.options
                .iter()
                .filter(|o| o.qualifies)
                .map(|o| o.index)
                .collect()
        })
        .unwrap_or_default();
    let several = qualifying.len() > 1;
    if several && app.open != Chooser::Launch {
        open_chooser(g, Chooser::Launch, cx);
        if let Some(app) = &mut g.app
            && app.option.is_none()
        {
            app.option = qualifying.first().copied();
        }
        return;
    }
    let Some(app) = &mut g.app else {
        return;
    };
    if app.busy {
        return;
    }
    let extra = match args::split(app.args.text()) {
        Ok(a) => a,
        Err(e) => {
            app.error = Some(format!("Arguments: {e}"));
            return;
        }
    };
    let option = if several { app.option } else { None };
    let stale = app.allow_stale;
    app.busy = true;
    app.error = None;
    // Launch waits without a timeout while tools install (§2.1).
    g.call_unbounded(
        move |c| c.launch(id, option, &extra, stale),
        move |g, r, cx| {
            if let Some(m) = g.model_mut() {
                m.store.launch_steps.remove(&id);
            }
            let Some(app) = page(g, id) else {
                return;
            };
            app.busy = false;
            app.allow_stale = false;
            match r {
                Ok(_) => close_chooser(g, id, cx),
                Err(e) => app.error = Some(e.message),
            }
        },
    );
}

fn set_compat(g: &mut Gui, value: Option<String>) {
    let Some(app) = &mut g.app else {
        return;
    };
    app.compat_error = None;
    let id = app.app_id;
    let key = format!("compat.tool.{id}");
    g.call(
        move |c| c.set_setting(&key, value.as_deref()),
        move |g, r: Result<v1::Setting, CallError>, _| match r {
            Ok(s) => {
                if let Some(m) = g.model_mut() {
                    m.store.set_setting(s);
                    m.read(Read::App(id));
                }
            }
            Err(e) => {
                if let Some(app) = page(g, id) {
                    app.compat_error = Some(e.message);
                }
            }
        },
    );
}

/// `path` decoded for a box of `size` device pixels, or `None` while it
/// loads and when it does not decode.
fn decoded(g: &Gui, path: &Arc<Path>, size: (u32, u32), fit: Fit) -> Option<Arc<Image>> {
    match g.artwork.get(path, size, fit) {
        Thumbnail::Ready(image) => Some(image),
        Thumbnail::Loading | Thumbnail::Failed => None,
    }
}

/// The hero with the logo over it; without a hero, the header image;
/// without either, the name (§4.3). The logo is asked for only once the
/// hero shows, the header image only once there is no hero.
fn header(g: &Gui, m: &Model, id: u32, name: &str, has_info: bool, p: Palette, cx: &ViewCx) -> El {
    let scale = cx.scale;
    let high_dpi = scale > 1.0;
    let device = |points: f32| (points * scale).round() as u32;
    let title = text(name.to_owned()).font_size(24.0);
    let hero = has_info.then(|| m.asset(id, AssetKind::Hero, high_dpi));
    match hero {
        Some(Asset::Ready(hero)) => {
            let width = device(cx.width).div_ceil(HERO_WIDTH_STEP) * HERO_WIDTH_STEP;
            let hero = decoded(g, &hero, (width, device(HERO_HEIGHT)), Fit::Cover);
            let logo = match m.asset(id, AssetKind::Logo, high_dpi) {
                // Decoded no larger than its box; drawn at that size.
                Asset::Ready(logo) => decoded(
                    g,
                    &logo,
                    (device(LOGO_BOX.0), device(LOGO_BOX.1)),
                    Fit::Contain,
                ),
                _ => None,
            };
            let over: El = match logo {
                Some(logo) => image(logo),
                None => panel()
                    .pad_xy(8.0, 2.0)
                    .radius(6.0)
                    .background(p.surface.with_alpha(0.8))
                    .child(title),
            };
            panel()
                .fill_width()
                .height(HERO_HEIGHT)
                .radius(8.0)
                .clip()
                .background(p.surface_raised)
                .when_some(hero, |d, hero| d.child(image(hero).fill().radius(8.0)))
                .child(over.absolute().inset(Edges {
                    left: Length::Px(24.0),
                    bottom: Length::Px(24.0),
                    ..Edges::all(Length::Auto)
                }))
        }
        Some(Asset::Missing) => {
            let header = match m.asset(id, AssetKind::Header, high_dpi) {
                Asset::Ready(h) => decoded(
                    g,
                    &h,
                    (device(HEADER_SIZE.0), device(HEADER_SIZE.1)),
                    Fit::Cover,
                ),
                _ => None,
            };
            match header {
                Some(header) => panel()
                    .gap(8.0)
                    .child(
                        panel()
                            .size(HEADER_SIZE.0, HEADER_SIZE.1)
                            .radius(8.0)
                            .clip()
                            .child(image(header).fill().radius(8.0)),
                    )
                    .child(title),
                None => title,
            }
        }
        _ => title,
    }
}

/// The state and main action (§4.3).
fn main(
    m: &Model,
    app: &AppPage,
    info: Option<&v1::GetAppResponse>,
    p: Palette,
    cx: &ViewCx,
) -> El {
    let store = &m.store;
    let id = app.app_id;
    let running = store.running.contains_key(&id);
    let job = store.unfinished_job(id);
    let installed = info.and_then(|i| i.installed.as_ref());
    let busy = app.busy;

    // Play, Play anyway and Stop share an id, and so a focus handle: the
    // one that replaces another keeps the focus.
    let mut out = group(p);
    if running {
        out = out.child(muted(p, "Running.")).child(
            button(p, "play-stop", "Stop")
                .danger()
                .disabled(busy)
                .on_click(|g, _| stop(g)),
        );
    } else if let Some(job) = job {
        out = out.child(job_row(m, job, None, p));
    } else if let Some(i) = installed {
        let mut buttons = panel().row().gap(8.0);
        if i.update_available {
            buttons = buttons
                .child(
                    button(p, "update", "Update")
                        .disabled(busy)
                        .on_click(|g, _| update(g)),
                )
                .child(
                    button(p, "play-stop", "Play anyway")
                        .secondary()
                        .disabled(busy)
                        .on_click(|g, cx| {
                            if let Some(app) = &mut g.app {
                                app.allow_stale = true;
                            }
                            play(g, cx);
                        }),
                );
        } else {
            let label = if busy { "Launching…" } else { "Play" };
            buttons = buttons.child(button(p, "play-stop", label).disabled(busy).on_click(play));
        }
        if busy && let Some(step) = store.launch_steps.get(&id) {
            buttons = buttons.child(muted(p, step.clone()));
        }
        out = out
            .when(i.update_available, |d| {
                d.child(muted(p, "An update is available."))
            })
            .child(buttons)
            .child(
                text_field(p, cx, args_focus(), |g: &mut Gui| {
                    &mut g.app.as_mut().expect("the App page is open").args
                })
                .width(420.0)
                .on_submit(play),
            );
        if app.open == Chooser::Launch {
            out = out.child(launch_chooser(m, app, p));
        }
    } else if info.is_some() {
        out = out.child(muted(p, "Not installed.")).child(
            button(p, "install", "Install")
                .disabled(busy)
                .on_click(|g, cx| open_chooser(g, Chooser::Install, cx)),
        );
        if app.open == Chooser::Install {
            out = out.child(install_chooser(m, app, info, p));
        }
    } else {
        out = out.child(muted(p, "Reading the app…"));
    }
    out.when_some(app.error.clone(), |d, e| d.child(error_text(p, e)))
}

/// A row of a confirm button and Cancel.
fn confirm_row(confirm: El, cancel_id: &str, p: Palette) -> El {
    panel().row().gap(8.0).child(confirm).child(
        button(p, cancel_id, "Cancel")
            .secondary()
            .on_click(|g, cx| {
                if let Some(id) = g.app.as_ref().map(|a| a.app_id) {
                    close_chooser(g, id, cx);
                }
            }),
    )
}

fn install_chooser(m: &Model, app: &AppPage, info: Option<&v1::GetAppResponse>, p: Palette) -> El {
    let folders = &m.store.folders;
    let default_folder = folders
        .iter()
        .find(|f| f.is_default)
        .map(|f| f.path.clone());
    let folder = app.folder.clone().or(default_folder);
    let mut folder_list = panel().gap(4.0);
    for (i, f) in folders.iter().enumerate() {
        let path = f.path.clone();
        let detail = if f.available {
            format!("{} free", format::size(f.free_bytes))
        } else {
            "not available".to_owned()
        };
        folder_list = folder_list.child(
            choice(
                p,
                widgets::id("folder", i),
                f.path.clone(),
                folder.as_deref() == Some(f.path.as_str()),
            )
            .detail(detail)
            .disabled(!f.available)
            .on_choose(move |g, _| {
                if let Some(app) = &mut g.app {
                    app.folder = Some(path.clone());
                }
            }),
        );
    }

    let mut branches: Vec<&v1::Branch> = info
        .map(|i| i.branches.iter().filter(|b| !b.password_required).collect())
        .unwrap_or_default();
    branches.sort_by_key(|b| (b.name != "public", b.name.clone()));
    let branch = app.branch.clone().unwrap_or_else(|| "public".to_owned());
    let mut branch_list = panel().gap(4.0);
    for (i, b) in branches.iter().enumerate() {
        let name = b.name.clone();
        let mut detail = format!("build {}", b.build_id);
        if b.time_updated != 0 {
            detail = format!("{detail}, updated {}", format::date(b.time_updated));
        }
        if !b.description.is_empty() {
            detail = format!("{}, {detail}", b.description);
        }
        branch_list = branch_list.child(
            choice(
                p,
                widgets::id("branch", i),
                b.name.clone(),
                branch == b.name,
            )
            .detail(detail)
            .on_choose(move |g, _| {
                if let Some(app) = &mut g.app {
                    app.branch = Some(name.clone());
                }
            }),
        );
    }

    panel()
        .id("install-chooser")
        .reveal_when_shown()
        .gap(12.0)
        .child(section(p, "Folder", folder_list))
        .when(branches.len() > 1, |d| {
            d.child(section(p, "Branch", branch_list))
        })
        .child(confirm_row(
            button(p, "confirm-install", "Install")
                .disabled(app.busy)
                .on_click(install)
                .into(),
            "cancel-install",
            p,
        ))
}

fn launch_chooser(m: &Model, app: &AppPage, p: Palette) -> El {
    let mut list = panel().gap(4.0);
    for o in options(m, app.app_id)
        .map(|o| o.options.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|o| o.qualifies)
    {
        let index = o.index;
        let label = if o.description.is_empty() {
            o.executable.clone()
        } else {
            o.description.clone()
        };
        let detail = if o.arguments.is_empty() {
            o.executable.clone()
        } else {
            format!("{} {}", o.executable, o.arguments)
        };
        list = list.child(
            choice(
                p,
                widgets::id("launch-option", index),
                label,
                app.option == Some(index),
            )
            .detail(detail)
            .on_choose(move |g, _| {
                if let Some(app) = &mut g.app {
                    app.option = Some(index);
                }
            }),
        );
    }
    panel()
        .id("launch-chooser")
        .reveal_when_shown()
        .child(section(p, "Launch option", list))
        .gap(12.0)
        .child(confirm_row(
            button(p, "launch", "Launch")
                .disabled(app.busy || app.option.is_none())
                .on_click(play)
                .into(),
            "cancel-launch",
            p,
        ))
}

/// A name and a muted word after it, for lists of DLC and tools.
fn named_state(p: Palette, name: String, state: &str) -> El {
    panel()
        .row()
        .align(Align::Center)
        .gap(8.0)
        .font_size(widgets::TEXT)
        .child(text(name))
        .child(text(state.to_owned()).font_size(SMALL).text_color(p.muted))
}

fn details(info: &v1::GetAppResponse, p: Palette) -> El {
    let mut out = group(p);
    if let Some(i) = &info.installed {
        out = out
            .child(row(p, "Path", i.path.clone()))
            .child(row(p, "Folder", i.folder.clone()))
            .child(row(p, "Branch", i.branch.clone()))
            .child(row(p, "Build", i.build_id.to_string()))
            .child(row(p, "Size", format::size(i.size)));
        let depots: Vec<String> = i
            .depots
            .iter()
            .map(|d| match d.shared_from {
                0 => format!("{} (manifest {})", d.depot_id, d.manifest_id),
                from => format!(
                    "{} (manifest {}, from app {from})",
                    d.depot_id, d.manifest_id
                ),
            })
            .collect();
        if !depots.is_empty() {
            out = out.child(row(p, "Depots", depots.join(", ")));
        }
    } else if !info.install_dir.is_empty() {
        out = out.child(row(p, "Directory", info.install_dir.clone()));
    }
    if !info.oslist.is_empty() {
        out = out.child(row(p, "Platforms", info.oslist.join(", ")));
    }
    if info.missing_token {
        out = out.child(muted(
            p,
            "Steam did not give this account the token for the app's full info.",
        ));
    }
    out
}

/// The DLC turned on in `content.optional_dlc.<app>` (content.md §6).
fn optional_dlc_on(m: &Model, app: u32) -> Vec<u32> {
    m.store
        .setting(&format!("content.optional_dlc.{app}"))
        .map(|s| s.value.split(',').filter_map(|t| t.parse().ok()).collect())
        .unwrap_or_default()
}

/// The account's DLC of the app; optional DLC with a switch that turns it
/// on or off (gui.md §4.3).
fn dlc(info: &v1::GetAppResponse, m: &Model, app: &AppPage, p: Palette) -> Option<El> {
    let dlc = info
        .app
        .as_ref()
        .map(|a| a.dlc.as_slice())
        .unwrap_or_default();
    if dlc.is_empty() {
        return None;
    }
    let on = optional_dlc_on(m, app.app_id);
    let list = panel().gap(4.0).children(dlc.iter().map(|d| {
        let name = if d.name.is_empty() {
            format!("App {}", d.app_id)
        } else {
            d.name.clone()
        };
        let state = if d.installed {
            "installed"
        } else {
            "not installed"
        };
        let optional = info.depots.iter().any(|x| x.optional_dlc == d.app_id);
        if !optional {
            return named_state(p, name, state);
        }
        let dlc_id = d.app_id;
        let is_on = on.contains(&dlc_id);
        panel()
            .row()
            .align(Align::Center)
            .gap(8.0)
            .child(
                switch(p, widgets::id("optional-dlc", dlc_id), name, is_on)
                    .on_toggle(move |g, _| set_optional_dlc(g, dlc_id, !is_on)),
            )
            .child(
                text(format!("optional, {state}"))
                    .font_size(SMALL)
                    .text_color(p.muted),
            )
    }));
    Some(
        group(p)
            .child(list)
            .when_some(app.dlc_error.clone(), |d, e| d.child(error_text(p, e))),
    )
}

fn set_optional_dlc(g: &mut Gui, dlc: u32, on: bool) {
    let Some(m) = g.model() else {
        return;
    };
    let Some(app) = &g.app else {
        return;
    };
    let id = app.app_id;
    let mut list = optional_dlc_on(m, id);
    list.retain(|&d| d != dlc);
    if on {
        list.push(dlc);
    }
    let value = (!list.is_empty()).then(|| {
        list.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    });
    if let Some(app) = &mut g.app {
        app.dlc_error = None;
    }
    let key = format!("content.optional_dlc.{id}");
    g.call(
        move |c| c.set_setting(&key, value.as_deref()),
        move |g, r: Result<v1::Setting, CallError>, _| match r {
            Ok(s) => {
                if let Some(m) = g.model_mut() {
                    m.store.set_setting(s);
                    m.read(Read::App(id));
                }
            }
            Err(e) => {
                if let Some(app) = page(g, id) {
                    app.dlc_error = Some(e.message);
                }
            }
        },
    );
}

/// Linux: the compatibility tool (§4.3).
fn compat(c: &v1::Compat, app: &AppPage, p: Palette) -> El {
    let tool = match &c.tool {
        Some(t) => format!("{} ({})", tool_label(t), source_text(c.source())),
        None => "none, it runs natively".to_owned(),
    };
    let mut out = group(p).child(row(p, "Tool", tool));
    if !c.chain.is_empty() {
        let chain = panel().children(c.chain.iter().map(|l| {
            let t = l.tool.as_ref().map(tool_label).unwrap_or_default();
            named_state(
                p,
                t,
                if l.installed {
                    "installed"
                } else {
                    "not installed"
                },
            )
        }));
        out = out.child(section(p, "Chain", chain));
    }
    if !c.platform.is_empty() {
        out = out.child(row(p, "Installed for", c.platform.clone()));
    }
    let user = c.source() == CompatSource::User;
    let current = c.tool.as_ref().map(|t| t.name.as_str());
    let mut choices = panel().gap(4.0).child(
        choice(p, "compat-default", "Default", !user)
            .detail("Valve's mapping, or the default tool")
            .on_choose(|g, _| set_compat(g, None)),
    );
    for (i, ch) in c.choices.iter().enumerate() {
        let name = ch.name.clone();
        let label = if ch.display_name.is_empty() || ch.display_name == ch.name {
            ch.name.clone()
        } else {
            format!("{}, {}", ch.display_name, ch.name)
        };
        choices = choices.child(
            choice(
                p,
                widgets::id("compat", i),
                label,
                user && current == Some(ch.name.as_str()),
            )
            .on_choose(move |g, _| set_compat(g, Some(name.clone()))),
        );
    }
    out.child(section(p, "Use", choices))
        .when_some(app.compat_error.clone(), |d, e| d.child(error_text(p, e)))
}

fn manage(info: &v1::GetAppResponse, app: &AppPage, p: Palette) -> Option<El> {
    info.installed.as_ref()?;
    let busy = app.busy;
    let mut out = group(p).child(
        panel()
            .row()
            .gap(8.0)
            .child(
                button(p, "verify", "Verify")
                    .secondary()
                    .disabled(busy)
                    .on_click(|g, _| verify(g)),
            )
            .child(
                button(p, "uninstall", "Uninstall…")
                    .danger()
                    .disabled(busy)
                    .on_click(|g, cx| {
                        if g.app.as_ref().is_some_and(|a| a.open != Chooser::Uninstall) {
                            open_chooser(g, Chooser::Uninstall, cx);
                            if let Some(app) = &mut g.app {
                                app.remove_compat_data = false;
                            }
                        }
                    }),
            ),
    );
    if app.open == Chooser::Uninstall {
        let name = info
            .app
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let confirm = panel()
            .id("uninstall-confirm")
            .reveal_when_shown()
            .gap(12.0)
            .child(muted(
                p,
                format!("Uninstall {name}? Its files are deleted."),
            ))
            .when(info.compat.is_some(), |d| {
                d.child(
                    switch(
                        p,
                        "remove-compat-data",
                        "Also delete the Proton prefix",
                        app.remove_compat_data,
                    )
                    .on_toggle(|g, _| {
                        if let Some(app) = &mut g.app {
                            app.remove_compat_data = !app.remove_compat_data;
                        }
                    }),
                )
            })
            .child(confirm_row(
                button(p, "confirm-uninstall", "Uninstall")
                    .danger()
                    .disabled(busy)
                    .on_click(|g, _| uninstall(g))
                    .into(),
                "cancel-uninstall",
                p,
            ));
        out = out.child(confirm);
    }
    Some(out)
}

/// How the game last ended and its last ShimReport (§4.3, §5).
fn last_run(m: &Model, id: u32, p: Palette) -> Option<El> {
    let store = &m.store;
    let exit = store.exits.get(&id);
    let report = store.shim_reports.get(&id);
    let earlier = match (exit, report) {
        (Some(e), Some(r)) => !e.reported(r),
        _ => false,
    };
    let exit = exit.map(|e| format::exited(&e.exited));
    if exit.is_none() && report.is_none() {
        return None;
    }
    let mut out = group(p).when_some(exit, |d, e| d.child(text(e).font_size(widgets::TEXT)));
    let Some(r) = report else {
        return Some(out);
    };
    let n = r.interfaces.len();
    if earlier {
        out = out.child(muted(
            p,
            "This run sent no report; the last one is from an earlier run.",
        ));
    }
    out = out.child(muted(
        p,
        format!(
            "The Steamworks library handed out {n} interface version{}.",
            if n == 1 { "" } else { "s" }
        ),
    ));
    if !r.interfaces.is_empty() {
        out = out.child(row(p, "Interfaces", r.interfaces.join(", ")));
    }
    if !r.unknown_versions.is_empty() {
        out = out.child(row(p, "Not provided", r.unknown_versions.join(", ")));
    }
    if r.unknown_versions.is_empty() && r.unimplemented.is_empty() {
        out = out.child(muted(p, "Everything the game called is implemented."));
    }
    if !r.unimplemented.is_empty() {
        let list = panel().children(r.unimplemented.iter().map(|c| {
            let name = if c.interface == "exports" {
                c.method.clone()
            } else {
                format!("{}::{}", c.interface, c.method)
            };
            panel()
                .row()
                .gap(12.0)
                .font_size(widgets::TEXT)
                .child(text(c.count.to_string()).width(64.0).text_color(p.muted))
                .child(text(name))
        }));
        out = out.child(section(
            p,
            "Unimplemented calls (they returned failure values)",
            list,
        ));
    }
    Some(out)
}

fn tool_label(t: &v1::CompatTool) -> String {
    match (t.name.as_str(), t.display_name.as_str()) {
        ("", "") => format!("app {}", t.app_id),
        ("", d) => format!("{d}, app {}", t.app_id),
        (n, d) if d.is_empty() || d == n => n.to_owned(),
        (n, d) => format!("{d}, {n}"),
    }
}

fn source_text(s: CompatSource) -> &'static str {
    match s {
        CompatSource::User => "your choice",
        CompatSource::Valve => "Valve's mapping",
        CompatSource::App => "the app's own mapping",
        CompatSource::Native => "it has a Linux version",
        CompatSource::Default => "the default for Windows titles",
        CompatSource::Unspecified => "unknown",
    }
}

pub fn view(g: &Gui, m: &Model, app: &AppPage, p: Palette, cx: &ViewCx) -> El {
    let id = app.app_id;
    let store = &m.store;
    let load_error = store
        .apps
        .get(&id)
        .and_then(|d| d.info.as_ref())
        .and_then(|r| r.as_ref().err())
        .map(|e| e.message.clone());
    let info = info(m, id);
    let name = info
        .and_then(|i| i.app.as_ref())
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| store.app_name(id).map(str::to_owned))
        .unwrap_or_else(|| format!("App {id}"));
    let has_info = info
        .and_then(|i| i.app.as_ref())
        .is_some_and(|a| a.has_info);

    let mut body = panel()
        .gap(16.0)
        .pad(24.0)
        .child(
            button(p, "back", "‹ Library")
                .ghost()
                .on_click(|g, cx| g.back_to_library(cx)),
        )
        .child(header(g, m, id, &name, has_info, p, cx))
        .when_some(load_error, |d, e| d.child(error_text(p, e)))
        .child(main(m, app, info, p, cx));
    if let Some(info) = info {
        body = body.child(section(p, "Details", details(info, p)));
        if let Some(d) = dlc(info, m, app, p) {
            body = body.child(section(p, "DLC", d));
        }
        if let Some(c) = &info.compat {
            body = body.child(section(p, "Compatibility", compat(c, app, p)));
        }
        if let Some(manage) = manage(info, app, p) {
            body = body.child(section(p, "Manage", manage));
        }
    }
    if let Some(r) = last_run(m, id, p) {
        body = body.child(section(p, "Last run", r));
    }
    // An element has one key context: Escape's is around the page's.
    panel()
        .fill()
        .key_context(CONTEXT)
        .on_action(|g: &mut Gui, _: &Back, cx: &mut Cx<Gui>| back(g, cx))
        .child(
            scrolling_page(&app.scroll)
                .id("app-page")
                .focus(page_focus())
                .child(body),
        )
}
