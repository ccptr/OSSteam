// SPDX-License-Identifier: GPL-2.0-or-later
//! The Settings page (`gui.md` §4.5).

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, PathPromptOptions, ScrollHandle,
    SharedString, Subscription, Window, div, prelude::*,
};
use ossteam_ipc::v1::{self, ErrorCode, PersonaState, SettingKind, SettingSource};

use crate::format;
use crate::model::Model;
use crate::pages::{error_text, muted, panel, row, section};
use crate::store::Read;
use crate::theme::palette;
use crate::widgets::{TextInput, TextInputEvent, button, choice, page_scroll, reveal, switch};

const DOWNLOAD_KEYS: [&str; 5] = [
    "content.bandwidth_limit",
    "content.concurrent_jobs",
    "content.download_threads",
    "content.auto_update",
    "content.language",
];
const DEFAULT_TOOL: &str = "compat.default_tool";
const TOOL_PREFIX: &str = "compat.tool.";

pub struct SettingsPage {
    model: Entity<Model>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Text fields of integer and string settings, made when first shown.
    inputs: HashMap<String, Entity<TextInput>>,
    errors: HashMap<String, SharedString>,
    folder_error: Option<SharedString>,
    account_error: Option<SharedString>,
    daemon_error: Option<SharedString>,
    confirm_logout: bool,
    /// Shutdown failed with CONFLICT: offer to stop the games too.
    offer_force: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_in(&model, window, |page, _, window, cx| {
            page.sync_inputs(window, cx);
            cx.notify();
        })];
        Self {
            model,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            inputs: HashMap::new(),
            errors: HashMap::new(),
            folder_error: None,
            account_error: None,
            daemon_error: None,
            confirm_logout: false,
            offer_force: false,
            _subscriptions: subscriptions,
        }
    }

    /// Fields that do not have focus show the daemon's value.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.model.read(cx).store.settings.clone();
        for (key, input) in &self.inputs {
            let Some(s) = settings.get(key) else {
                continue;
            };
            let focused = input.focus_handle(cx).is_focused(window);
            if !focused && input.read(cx).text() != s.value {
                input.update(cx, |i, cx| i.set_text(&s.value, cx));
            }
        }
    }

    fn input(
        &mut self,
        s: &v1::Setting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(i) = self.inputs.get(&s.key) {
            return i.clone();
        }
        let input = TextInput::build(s.default_value.clone(), window, cx);
        input.update(cx, |i, cx| i.set_text(&s.value, cx));
        let key = s.key.clone();
        cx.subscribe_in(&input, window, move |page, input, e, _, cx| {
            if matches!(e, TextInputEvent::Submitted | TextInputEvent::Blurred) {
                let text = input.read(cx).text().to_owned();
                let current = page
                    .model
                    .read(cx)
                    .store
                    .setting(&key)
                    .map(|s| s.value.clone());
                if current.as_deref() != Some(text.as_str()) {
                    page.set(key.clone(), Some(text), cx);
                }
            }
        })
        .detach();
        self.inputs.insert(s.key.clone(), input.clone());
        input
    }

    /// `SetSetting`, or with `None` a reset; the error shows by the
    /// setting.
    fn set(&mut self, key: String, value: Option<String>, cx: &mut Context<Self>) {
        self.errors.remove(&key);
        let k = key.clone();
        let call = self
            .model
            .read(cx)
            .daemon
            .call(move |c| c.set_setting(&k, value.as_deref()));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                match r {
                    Ok(s) => page.model.update(cx, |m, cx| {
                        m.store.set_setting(s);
                        cx.notify();
                    }),
                    Err(e) => {
                        page.errors.insert(key, e.message.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn setting(
        &mut self,
        s: &v1::Setting,
        label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let key = s.key.clone();
        let source = match s.source() {
            SettingSource::Default => "default".to_owned(),
            SettingSource::File => "from ossteamd.toml".to_owned(),
            SettingSource::Override => "override".to_owned(),
            SettingSource::Unspecified => "unknown".to_owned(),
        };
        let control = match s.kind() {
            SettingKind::Bool => {
                let on = s.value == "true";
                let k = key.clone();
                switch(
                    SharedString::from(format!("setting-{key}")),
                    label.clone().unwrap_or_else(|| key.clone()),
                    on,
                )
                .on_toggle(cx.listener(move |page, _, _, cx| {
                    page.set(k.clone(), Some((!on).to_string()), cx)
                }))
                .into_any_element()
            }
            _ => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .child(label.clone().unwrap_or_else(|| key.clone())),
                )
                .child(div().w_64().child(self.input(s, window, cx)))
                .into_any_element(),
        };
        let reset = (s.source() == SettingSource::Override).then(|| {
            let k = key.clone();
            button(SharedString::from(format!("reset-{key}")), "Reset")
                .ghost()
                .on_click(cx.listener(move |page, _, _, cx| page.set(k.clone(), None, cx)))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(control)
                    .child(div().text_xs().text_color(p.muted).child(source))
                    .when_some(reset, |d, r| d.child(r)),
            )
            .when(label.is_some() || !s.description.is_empty(), |d| {
                let mut text = s.description.clone();
                if label.is_some() {
                    text = if text.is_empty() {
                        key.clone()
                    } else {
                        format!("{key}: {text}")
                    };
                }
                d.child(div().text_xs().text_color(p.muted).child(text))
            })
            .when_some(self.errors.get(&key).cloned(), |d, e| {
                d.child(error_text(e, cx))
            })
            .into_any_element()
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        self.folder_error = None;
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add library folder".into()),
        });
        let daemon = self.model.read(cx).daemon.clone();
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |page, cx| {
                        page.folder_error =
                            Some(format!("cannot open the folder picker: {e}").into());
                        cx.notify();
                    });
                    None
                }
            };
            let Some(path) = path else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            let r = daemon.call(move |c| c.add_library_folder(&path)).await;
            let _ = this.update(cx, |page, cx| {
                if let Err(e) = r {
                    page.folder_error = Some(e.message.into());
                }
                page.model.update(cx, |m, cx| m.read(Read::Folders, cx));
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_folder(&mut self, path: String, cx: &mut Context<Self>) {
        self.folder_error = None;
        let call = self
            .model
            .read(cx)
            .daemon
            .call(move |c| c.remove_library_folder(&path));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                if let Err(e) = r {
                    page.folder_error = Some(e.message.into());
                }
                page.model.update(cx, |m, cx| m.read(Read::Folders, cx));
                cx.notify();
            });
        })
        .detach();
    }

    fn folders(&self, cx: &mut Context<Self>) -> AnyElement {
        let folders = self.model.read(cx).store.folders.clone();
        let mut list = div().flex().flex_col().gap_2();
        for (i, f) in folders.iter().enumerate() {
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
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().child(f.path.clone()))
                            .child(muted(detail, cx)),
                    )
                    .child(button(("remove-folder", i), "Remove").danger().on_click(
                        cx.listener(move |page, _, _, cx| page.remove_folder(path.clone(), cx)),
                    )),
            );
        }
        panel(cx)
            .child(list)
            .child(
                div().flex().child(
                    button("add-folder", "Add folder…")
                        .secondary()
                        .on_click(cx.listener(|page, _, _, cx| page.add_folder(cx))),
                ),
            )
            .when_some(self.folder_error.clone(), |d, e| d.child(error_text(e, cx)))
            .into_any_element()
    }

    fn account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let store = &self.model.read(cx).store;
        let account = store.account()?.clone();
        let state = store.status.persona_state();
        let mut states = div().flex().flex_col().gap_1();
        for s in [
            PersonaState::Online,
            PersonaState::Invisible,
            PersonaState::Offline,
        ] {
            states = states.child(
                choice(
                    ("persona", s as usize),
                    format::persona_state(s),
                    state == s,
                )
                .on_choose(cx.listener(move |page, _, _, cx| {
                    page.account_error = None;
                    page.model.update(cx, |m, cx| {
                        m.run(cx, move |c| c.set_persona_state(s), |_, (), _| {})
                    });
                    cx.notify();
                })),
            );
        }
        let logout = if self.confirm_logout {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(muted("Log out and forget the stored login?", cx))
                .child(
                    button("confirm-logout", "Log out")
                        .danger()
                        .on_click(cx.listener(|page, _, _, cx| page.logout(cx))),
                )
                .child(
                    button("cancel-logout", "Cancel")
                        .secondary()
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.confirm_logout = false;
                            cx.notify();
                        })),
                )
        } else {
            div()
                .flex()
                .child(button("logout", "Log out…").danger().on_click(cx.listener(
                    |page, _, _, cx| {
                        page.confirm_logout = true;
                        cx.notify();
                    },
                )))
        };
        Some(
            panel(cx)
                .child(row(
                    "Account",
                    format!("{} ({})", account.persona_name, account.account_name),
                    cx,
                ))
                .child(section("Persona state", cx).child(states))
                .child(logout)
                .when_some(self.account_error.clone(), |d, e| {
                    d.child(error_text(e, cx))
                })
                .into_any_element(),
        )
    }

    fn logout(&mut self, cx: &mut Context<Self>) {
        self.confirm_logout = false;
        self.account_error = None;
        let call = self.model.read(cx).daemon.call(|c| c.logout());
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                if let Err(e) = r {
                    page.account_error = Some(e.message.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn shutdown(&mut self, force: bool, cx: &mut Context<Self>) {
        self.daemon_error = None;
        self.offer_force = false;
        let call = self.model.read(cx).daemon.call(move |c| c.shutdown(force));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                match r {
                    Err(e) if e.is(ErrorCode::Conflict) => {
                        page.offer_force = true;
                        page.daemon_error = Some(e.message.into());
                    }
                    // The connection closing first is the shutdown working.
                    Err(e) if e.code.is_some() => page.daemon_error = Some(e.message.into()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn daemon(
        &mut self,
        others: Vec<v1::Setting>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = self.model.read(cx);
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
        let mut out = panel(cx);
        for (k, v) in info {
            out = out.child(row(k, v, cx));
        }
        for s in &others {
            out = out.child(self.setting(s, None, window, cx));
        }
        let stop = if self.offer_force {
            div()
                .flex()
                .gap_2()
                .child(
                    button("force-stop", "Stop the games and ossteamd")
                        .danger()
                        .on_click(cx.listener(|page, _, _, cx| page.shutdown(true, cx))),
                )
                .child(
                    button("cancel-stop", "Cancel")
                        .secondary()
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.offer_force = false;
                            page.daemon_error = None;
                            cx.notify();
                        })),
                )
        } else {
            div().flex().child(
                button("stop-daemon", "Stop ossteamd")
                    .danger()
                    .on_click(cx.listener(|page, _, _, cx| page.shutdown(false, cx))),
            )
        };
        out.child(stop)
            .when_some(self.daemon_error.clone(), |d, e| d.child(error_text(e, cx)))
            .into_any_element()
    }
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

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        reveal::scroll_to_focus(&self.focus, &self.scroll, window, cx);
        let settings: Vec<v1::Setting> = self
            .model
            .read(cx)
            .store
            .settings
            .values()
            .cloned()
            .collect();
        let linux = cfg!(target_os = "linux");
        let mut downloads = panel(cx);
        for key in DOWNLOAD_KEYS {
            if let Some(s) = settings.iter().find(|s| s.key == key) {
                downloads =
                    downloads.child(self.setting(s, label(key).map(str::to_owned), window, cx));
            }
        }

        let mut compat = panel(cx);
        if let Some(s) = settings.iter().find(|s| s.key == DEFAULT_TOOL) {
            compat =
                compat.child(self.setting(s, label(DEFAULT_TOOL).map(str::to_owned), window, cx));
        }
        let overrides: Vec<&v1::Setting> = settings
            .iter()
            .filter(|s| s.key.starts_with(TOOL_PREFIX) && s.source() == SettingSource::Override)
            .collect();
        for s in &overrides {
            let app = s.key[TOOL_PREFIX.len()..].parse::<u32>().ok();
            let name = app
                .and_then(|a| self.model.read(cx).store.app_name(a).map(str::to_owned))
                .unwrap_or_else(|| s.key[TOOL_PREFIX.len()..].to_owned());
            compat = compat.child(self.setting(s, Some(name), window, cx));
        }

        let named =
            |k: &str| DOWNLOAD_KEYS.contains(&k) || k == DEFAULT_TOOL || k.starts_with(TOOL_PREFIX);
        let others: Vec<v1::Setting> = settings
            .iter()
            .filter(|s| !named(&s.key) || (!linux && s.key.starts_with("compat.")))
            .cloned()
            .collect();

        let folders = self.folders(cx);
        let account = self.account(cx);
        let daemon = self.daemon(others, window, cx);
        page_scroll::by_keyboard(div().id("settings-page"), &self.scroll, None)
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_xl().child("Settings"))
            .child(section("Library folders", cx).child(folders))
            .child(section("Downloads", cx).child(downloads))
            .when(linux, |d| {
                d.child(section("Compatibility", cx).child(compat))
            })
            .when_some(account, |d, a| d.child(section("Account", cx).child(a)))
            .child(section("Daemon", cx).child(daemon))
    }
}
