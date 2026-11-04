// SPDX-License-Identifier: GPL-2.0-or-later
//! The Settings page (`gui.md` §4.5): library folders, the daemon's
//! settings, the account and the daemon.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use ossteam_ipc::v1::{self, ErrorCode, PersonaState, SettingKind, SettingSource};
use ossteam_ui::element::{FocusHandle, ScrollHandle, panel, text};
use ossteam_ui::folder_picker::{self, pick_folder};
use ossteam_ui::style::{Align, Styled};
use ossteam_ui::text_field::TextField;
use ossteam_ui::window::{Cx, ViewCx};

use crate::format;
use crate::gui::Gui;
use crate::model::Model;
use crate::store::{self, Read};
use crate::theme::Palette;
use crate::widgets::{
    self, El, SMALL, button, choice, error_text, group, muted, row, scrolling_page, section,
    switch, text_field, title,
};

const DOWNLOAD_KEYS: [&str; 5] = [
    "content.bandwidth_limit",
    "content.concurrent_jobs",
    "content.download_threads",
    "content.auto_update",
    "content.language",
];
const DEFAULT_TOOL: &str = "compat.default_tool";
const TOOL_PREFIX: &str = "compat.tool.";
const PICKER_TITLE: &str = "Add library folder";

#[derive(Default)]
pub struct SettingsPage {
    scroll: ScrollHandle,
    /// Text fields of integer and string settings.
    inputs: HashMap<String, TextField>,
    errors: HashMap<String, String>,
    folder_error: Option<String>,
    /// The folder picker is open.
    picking: bool,
    account_error: Option<String>,
    daemon_error: Option<String>,
    confirm_logout: bool,
    /// Shutdown failed with CONFLICT: offer to stop the games too.
    offer_force: bool,
}

pub fn page_focus() -> FocusHandle {
    FocusHandle::named("settings-page")
}

/// A setting's control, its switch or its field: one per setting.
fn control_id(key: &str) -> String {
    format!("setting-{key}")
}

fn new_field() -> TextField {
    // Not the default: an empty field is an empty value, which for
    // compat.default_tool means no tool.
    TextField::new("empty", widgets::TEXT)
}

/// Fields that do not have focus show the daemon's value.
pub fn before_view(g: &mut Gui, cx: &mut Cx<Gui>) {
    let Some(m) = g.model() else {
        return;
    };
    let settings: Vec<(String, String)> = m
        .store
        .settings
        .values()
        .filter(|s| s.kind() != SettingKind::Bool)
        .map(|s| (s.key.clone(), s.value.clone()))
        .collect();
    for (key, value) in settings {
        let focused = cx.is_focused(FocusHandle::named(control_id(&key)));
        let field = g.settings.inputs.entry(key).or_insert_with(|| {
            let mut f = new_field();
            f.set_text(&value);
            f
        });
        if !focused && field.text() != value {
            field.set_text(&value);
        }
    }
}

/// `SetSetting`, or with `None` a reset; the error shows by the setting.
fn set(g: &mut Gui, key: String, value: Option<String>) {
    g.settings.errors.remove(&key);
    let reset = value.is_none();
    let k = key.clone();
    g.call(
        move |c| c.set_setting(&k, value.as_deref()),
        move |g, r, _| match r {
            Ok(s) => {
                // The field has focus after a reset, so it does not
                // follow the store: it would send the old value back when
                // it loses focus.
                if reset && let Some(f) = g.settings.inputs.get_mut(&s.key) {
                    f.set_text(&s.value);
                }
                if let Some(m) = g.model_mut() {
                    m.store.set_setting(s);
                }
            }
            Err(e) => {
                g.settings.errors.insert(key, e.message);
            }
        },
    );
}

/// Enter in a field, or focus leaving it: sends a changed value.
fn commit(g: &mut Gui, key: &str) {
    let Some(text) = g.settings.inputs.get(key).map(|f| f.text().to_owned()) else {
        return;
    };
    let current = g
        .model()
        .and_then(|m| m.store.setting(key))
        .map(|s| s.value.as_str());
    if current != Some(text.as_str()) {
        set(g, key.to_owned(), Some(text));
    }
}

/// Reset goes away once it works: focus goes to the setting's own
/// control, or for a tool override, whose row goes too, to the default
/// tool.
fn focus_for_reset(key: &str, cx: &mut Cx<Gui>) {
    let key = if key.starts_with(TOOL_PREFIX) {
        DEFAULT_TOOL
    } else {
        key
    };
    cx.focus(FocusHandle::named(control_id(key)));
}

fn setting(g: &Gui, s: &v1::Setting, label: Option<String>, p: Palette, cx: &ViewCx) -> El {
    let key = s.key.clone();
    let id = control_id(&key);
    let source = match s.source() {
        SettingSource::Default => "default",
        SettingSource::File => "from ossteamd.toml",
        SettingSource::Override => "override",
        SettingSource::Unspecified => "unknown",
    };
    let name = label.clone().unwrap_or_else(|| key.clone());
    let control: El = match s.kind() {
        SettingKind::Bool => {
            let on = s.value == "true";
            let k = key.clone();
            switch(p, id, name, on)
                .on_toggle(move |g, _| set(g, k.clone(), Some((!on).to_string())))
                .into()
        }
        _ => {
            let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
            panel()
                .gap(4.0)
                .child(text(name).font_size(widgets::TEXT))
                .child(
                    text_field(p, cx, FocusHandle::named(id), move |g: &mut Gui| {
                        g.settings
                            .inputs
                            .entry(k1.clone())
                            .or_insert_with(new_field)
                    })
                    .width(256.0)
                    .on_submit(move |g, _| commit(g, &k2))
                    .on_focus_out(move |g, _| commit(g, &k3)),
                )
        }
    };
    let reset = (s.source() == SettingSource::Override).then(|| {
        let k = key.clone();
        button(p, format!("reset-{key}"), "Reset")
            .ghost()
            .on_click(move |g, cx| {
                focus_for_reset(&k, cx);
                set(g, k.clone(), None);
            })
    });
    let mut description = s.description.clone();
    if label.is_some() {
        description = if description.is_empty() {
            key.clone()
        } else {
            format!("{key}: {description}")
        };
    }
    panel()
        .gap(4.0)
        .child(
            panel()
                .row()
                .align(Align::End)
                .gap(12.0)
                .child(control)
                .child(text(source).font_size(SMALL).text_color(p.muted))
                // A button aligns itself to the start; its box takes the row's
                // alignment instead.
                .when_some(reset, |d, r| d.child(panel().child(r))),
        )
        .when(!description.is_empty(), |d| {
            d.child(text(description).font_size(SMALL).text_color(p.muted))
        })
        .when_some(g.settings.errors.get(&key).cloned(), |d, e| {
            d.child(error_text(p, e))
        })
}

/// Asks for a folder and adds it. The picker blocks: on a thread of its
/// own, except on macOS, where AppKit's panels run on the main thread
/// and keep the window's events flowing.
fn add_folder(g: &mut Gui) {
    if g.settings.picking {
        return;
    }
    g.settings.folder_error = None;
    if cfg!(target_os = "macos") {
        let r = pick_folder(PICKER_TITLE);
        picked(g, r);
    } else {
        g.settings.picking = true;
        g.background(
            "ossteam-gui-folder-picker",
            || pick_folder(PICKER_TITLE),
            |g, r, _| {
                g.settings.picking = false;
                picked(g, r);
            },
        );
    }
}

fn picked(g: &mut Gui, r: Result<Option<PathBuf>, folder_picker::Error>) {
    let path = match r {
        Ok(Some(path)) => path.to_string_lossy().into_owned(),
        Ok(None) => return,
        Err(e) => {
            g.settings.folder_error = Some(format!("cannot open the folder picker: {e}"));
            return;
        }
    };
    g.call(
        move |c| c.add_library_folder(&path),
        |g, r, _| folders_changed(g, r.err().map(|e| e.message)),
    );
}

fn remove_folder(g: &mut Gui, path: String) {
    g.settings.folder_error = None;
    g.call(
        move |c| c.remove_library_folder(&path),
        |g, r, _| folders_changed(g, r.err().map(|e| e.message)),
    );
}

fn folders_changed(g: &mut Gui, error: Option<String>) {
    g.settings.folder_error = error;
    if let Some(m) = g.model_mut() {
        m.read(Read::Folders);
    }
}

fn folders(g: &Gui, m: &Model, p: Palette) -> El {
    let mut list = panel().gap(8.0);
    for (i, f) in m.store.folders.iter().enumerate() {
        let space = if f.available {
            format!("{} free", format::size(f.free_bytes))
        } else {
            "not available".to_owned()
        };
        let n = f.app_ids.len();
        let mut detail = format!("{space}, {n} app{}", if n == 1 { "" } else { "s" });
        if f.is_default {
            detail = format!("default, {detail}");
        }
        let path = f.path.clone();
        list = list.child(
            panel()
                .row()
                .align(Align::Center)
                .gap(12.0)
                .child(
                    panel()
                        .grow()
                        .child(text(f.path.clone()).font_size(widgets::TEXT))
                        .child(muted(p, detail)),
                )
                .child(
                    button(p, widgets::id("remove-folder", i), "Remove")
                        .danger()
                        .on_click(move |g, _| remove_folder(g, path.clone())),
                ),
        );
    }
    group(p)
        .child(list)
        .child(
            button(p, "add-folder", "Add folder…")
                .secondary()
                .disabled(g.settings.picking)
                .on_click(|g, _| add_folder(g)),
        )
        .when_some(g.settings.folder_error.clone(), |d, e| {
            d.child(error_text(p, e))
        })
}

fn logout(g: &mut Gui) {
    g.settings.confirm_logout = false;
    g.settings.account_error = None;
    g.call(
        |c| c.logout(),
        |g, r, _| {
            if let Err(e) = r {
                g.settings.account_error = Some(e.message);
            }
        },
    );
}

fn account(g: &Gui, m: &Model, p: Palette) -> Option<El> {
    let store = &m.store;
    let account = store.account()?;
    let state = store.status.persona_state();
    let mut states = panel().gap(4.0);
    for s in [
        PersonaState::Online,
        PersonaState::Invisible,
        PersonaState::Offline,
    ] {
        states = states.child(
            choice(
                p,
                widgets::id("persona", s as i32),
                format::persona_state(s),
                state == s,
            )
            .on_choose(move |g, _| {
                g.settings.account_error = None;
                if let Some(m) = g.model_mut() {
                    m.run(move |c| c.set_persona_state(s), |_, ()| {});
                }
            }),
        );
    }
    // Cancel takes the focus handle of Log out…, which it replaces: a
    // second Enter cancels, and Log out… gets the focus back.
    let logout_row = if g.settings.confirm_logout {
        panel()
            .row()
            .align(Align::Center)
            .gap(8.0)
            .child(muted(p, "Log out and forget the stored login?"))
            .child(
                button(p, "confirm-logout", "Log out")
                    .danger()
                    .on_click(|g, _| logout(g)),
            )
            .child(
                button(p, "cancel-logout", "Cancel")
                    .secondary()
                    .focus(FocusHandle::named("logout"))
                    .on_click(|g, _| g.settings.confirm_logout = false),
            )
    } else {
        button(p, "logout", "Log out…")
            .danger()
            .on_click(|g, _| g.settings.confirm_logout = true)
            .into()
    };
    Some(
        group(p)
            .child(row(
                p,
                "Account",
                format!("{} ({})", account.persona_name, account.account_name),
            ))
            .child(section(p, "Persona state", states))
            .child(logout_row)
            .when_some(g.settings.account_error.clone(), |d, e| {
                d.child(error_text(p, e))
            }),
    )
}

fn shutdown(g: &mut Gui, force: bool) {
    g.settings.daemon_error = None;
    g.settings.offer_force = false;
    g.call(
        move |c| c.shutdown(force),
        |g, r, _| match r {
            Err(e) if e.is(ErrorCode::Conflict) => {
                g.settings.offer_force = true;
                g.settings.daemon_error = Some(e.message);
            }
            // The connection closing first is the shutdown working.
            Err(e) if e.code.is_some() => g.settings.daemon_error = Some(e.message),
            _ => {}
        },
    );
}

fn daemon(g: &Gui, m: &Model, others: &[&v1::Setting], p: Palette, cx: &ViewCx) -> El {
    let status = &m.store.status;
    let protocol = m.store.version.protocol.unwrap_or_default();
    let uptime = Duration::from_secs(status.uptime_secs) + m.status_read.elapsed();
    let info = [
        ("Version", status.daemon_version.clone()),
        ("Process", status.pid.to_string()),
        ("Running for", format::duration(uptime)),
        ("Protocol", format!("{}.{}", protocol.major, protocol.minor)),
        (
            "Platform",
            format!("{} {}", m.store.version.os, m.store.version.arch),
        ),
    ];
    let mut out = group(p);
    for (k, v) in info {
        out = out.child(row(p, k, v));
    }
    for s in others {
        out = out.child(setting(g, s, None, p, cx));
    }
    // As with Log out…: Cancel takes the focus of Stop ossteamd.
    let stop: El = if g.settings.offer_force {
        panel()
            .row()
            .gap(8.0)
            .child(
                button(p, "force-stop", "Stop the games and ossteamd")
                    .danger()
                    .on_click(|g, _| shutdown(g, true)),
            )
            .child(
                button(p, "cancel-stop", "Cancel")
                    .secondary()
                    .focus(FocusHandle::named("stop-daemon"))
                    .on_click(|g, _| {
                        g.settings.offer_force = false;
                        g.settings.daemon_error = None;
                    }),
            )
    } else {
        button(p, "stop-daemon", "Stop ossteamd")
            .danger()
            .on_click(|g, _| shutdown(g, false))
            .into()
    };
    out.child(stop)
        .when_some(g.settings.daemon_error.clone(), |d, e| {
            d.child(error_text(p, e))
        })
}

/// Whether launched games get a private `~/.steam` (`gui.md` §4.5).
fn discovery(g: &v1::GameIsolation, p: Palette) -> El {
    let isolated = if g.available { "Yes" } else { "No" };
    let held = if g.holds_official_entries {
        "Held by ossteam"
    } else {
        "Not held by ossteam"
    };
    group(p)
        .child(row(p, "Private ~/.steam", isolated))
        .when_some(store::isolation_warning(g), |d, w| {
            d.child(text(w).font_size(widgets::TEXT).text_color(p.warning))
        })
        .child(row(p, "Official entries", held))
}

/// A label for the settings this page names.
fn label(key: &str) -> Option<&'static str> {
    Some(match key {
        "content.bandwidth_limit" => "Bandwidth limit",
        "content.concurrent_jobs" => "Jobs at once",
        "content.download_threads" => "Download threads",
        "content.auto_update" => "Update automatically",
        "content.language" => "Content language",
        DEFAULT_TOOL => "Default compatibility tool",
        _ => return None,
    })
}

pub fn view(g: &Gui, m: &Model, p: Palette, cx: &ViewCx) -> El {
    let settings = &m.store.settings;
    let linux = cfg!(target_os = "linux");
    let labelled = |key: &str| label(key).map(str::to_owned);

    let mut downloads = group(p);
    for key in DOWNLOAD_KEYS {
        if let Some(s) = settings.get(key) {
            downloads = downloads.child(setting(g, s, labelled(key), p, cx));
        }
    }

    let mut compat = group(p);
    if let Some(s) = settings.get(DEFAULT_TOOL) {
        compat = compat.child(setting(g, s, labelled(DEFAULT_TOOL), p, cx));
    }
    for s in settings
        .values()
        .filter(|s| s.key.starts_with(TOOL_PREFIX) && s.source() == SettingSource::Override)
    {
        let app = &s.key[TOOL_PREFIX.len()..];
        let name = app
            .parse::<u32>()
            .ok()
            .and_then(|a| m.store.app_name(a))
            .unwrap_or(app)
            .to_owned();
        compat = compat.child(setting(g, s, Some(name), p, cx));
    }

    let named =
        |k: &str| DOWNLOAD_KEYS.contains(&k) || k == DEFAULT_TOOL || k.starts_with(TOOL_PREFIX);
    let others: Vec<&v1::Setting> = settings
        .values()
        .filter(|s| !named(&s.key) || (!linux && s.key.starts_with("compat.")))
        .collect();

    let body = panel()
        .gap(16.0)
        .pad(24.0)
        .child(title("Settings"))
        .child(section(p, "Library folders", folders(g, m, p)))
        .child(section(p, "Downloads", downloads))
        .when(linux, |d| d.child(section(p, "Compatibility", compat)))
        .when_some(m.store.status.game_isolation.as_ref(), |d, i| {
            d.child(section(p, "Discovery", discovery(i, p)))
        })
        .when_some(account(g, m, p), |d, a| d.child(section(p, "Account", a)))
        .child(section(p, "Daemon", daemon(g, m, &others, p, cx)));
    scrolling_page(&g.settings.scroll)
        .id("settings-page")
        .focus(page_focus())
        .fill()
        .child(body)
}
