// SPDX-License-Identifier: GPL-2.0-or-later
//! The App page (`gui.md` §4.3).

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ObjectFit,
    SharedString, Subscription, Window, actions, div, img, prelude::*, px,
};
use ossteam_ipc::v1::{self, AssetKind, CompatSource};

use crate::args;
use crate::format;
use crate::model::{Asset, Model};
use crate::pages::downloads::job_row;
use crate::pages::{error_text, muted, panel, row, section};
use crate::theme::palette;
use crate::widgets::{TextInput, TextInputEvent, button, choice, switch};

actions!(app_page, [Back]);

const CONTEXT: &str = "AppPage";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Back, Some(CONTEXT))]);
}

pub enum AppPageEvent {
    Close,
}

/// Which inline chooser is open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    Install,
    Launch,
    Uninstall,
}

pub struct AppPage {
    model: Entity<Model>,
    app_id: u32,
    focus: FocusHandle,
    open: Open,
    folder: Option<String>,
    branch: Option<String>,
    option: Option<u32>,
    allow_stale: bool,
    args: Entity<TextInput>,
    remove_compat_data: bool,
    error: Option<SharedString>,
    compat_error: Option<SharedString>,
    busy: bool,
    /// Ctrl-Enter came before the details: act once they arrive.
    pending_main_action: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AppPageEvent> for AppPage {}

impl AppPage {
    pub fn new(
        model: Entity<Model>,
        app_id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        model.update(cx, |m, cx| m.open_app(app_id, cx));
        let args = TextInput::build("Arguments", window, cx);
        let subscriptions = vec![
            cx.observe_in(&model, window, |page, _, window, cx| {
                if page.pending_main_action && page.info(cx).is_some() {
                    page.pending_main_action = false;
                    page.main_action(window, cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(&args, window, |page, _, e, window, cx| {
                if *e == TextInputEvent::Submitted {
                    page.play(window, cx);
                }
            }),
        ];
        Self {
            model,
            app_id,
            focus: cx.focus_handle(),
            open: Open::None,
            folder: None,
            branch: None,
            option: None,
            allow_stale: false,
            args,
            remove_compat_data: false,
            error: None,
            compat_error: None,
            busy: false,
            pending_main_action: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn app_id(&self) -> u32 {
        self.app_id
    }

    fn info<'a>(&self, cx: &'a App) -> Option<&'a v1::GetAppResponse> {
        self.model
            .read(cx)
            .store
            .apps
            .get(&self.app_id)?
            .info
            .as_ref()?
            .as_ref()
            .ok()
    }

    fn options<'a>(&self, cx: &'a App) -> Option<&'a v1::GetLaunchOptionsResponse> {
        self.model
            .read(cx)
            .store
            .apps
            .get(&self.app_id)?
            .options
            .as_ref()?
            .as_ref()
            .ok()
    }

    fn back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        if self.open != Open::None {
            self.open = Open::None;
            cx.notify();
        } else {
            cx.emit(AppPageEvent::Close);
        }
    }

    /// Ctrl-Enter (§6): plays, updates, or opens and then confirms the
    /// install chooser, as the page's main button would.
    pub fn main_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.info(cx) else {
            self.pending_main_action = true;
            return;
        };
        let store = &self.model.read(cx).store;
        if store.running.contains_key(&self.app_id) || store.unfinished_job(self.app_id).is_some() {
            return;
        }
        match &info.installed {
            None if self.open == Open::Install => self.install(cx),
            None => self.open_install(cx),
            Some(i) if i.update_available => self.update(cx),
            Some(_) => self.play(window, cx),
        }
    }

    fn open_install(&mut self, cx: &mut Context<Self>) {
        self.open = Open::Install;
        cx.notify();
    }

    /// Runs a call whose error shows by the main action.
    fn act<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&ossteam_ipc::Client) -> Result<T, ossteam_ipc::ClientError> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let call = self.model.read(cx).daemon.call(f);
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                match r {
                    Ok(v) => done(page, v, cx),
                    Err(e) => page.error = Some(e.message.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_jobs(&mut self, jobs: Vec<v1::Job>, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            for j in jobs {
                m.store.add_job(j);
            }
            cx.notify();
        });
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let app = self.app_id;
        let folder = self.folder.clone().unwrap_or_default();
        let branch = self.branch.clone().unwrap_or_default();
        self.act(
            cx,
            move |c| c.install(app, &folder, &branch),
            |page, job, cx| {
                page.open = Open::None;
                page.add_jobs(vec![job], cx);
            },
        );
    }

    fn update(&mut self, cx: &mut Context<Self>) {
        let app = self.app_id;
        self.act(
            cx,
            move |c| c.update(&[app], false),
            |page, jobs, cx| page.add_jobs(jobs, cx),
        );
    }

    fn verify(&mut self, cx: &mut Context<Self>) {
        let app = self.app_id;
        self.act(
            cx,
            move |c| c.verify(app),
            |page, job, cx| page.add_jobs(vec![job], cx),
        );
    }

    fn uninstall(&mut self, cx: &mut Context<Self>) {
        let app = self.app_id;
        let remove = self.remove_compat_data;
        self.act(
            cx,
            move |c| c.uninstall(app, remove),
            |page, job, cx| {
                page.open = Open::None;
                page.add_jobs(vec![job], cx);
            },
        );
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let app = self.app_id;
        self.act(cx, move |c| c.stop_game(app), |_, (), _| {});
    }

    /// Play: launches the only option that qualifies, or opens the chooser
    /// when there are several, then launches the chosen one (§4.3).
    fn play(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let single = self.options(cx).is_some_and(|o| o.single);
        if !single && self.open != Open::Launch {
            self.open = Open::Launch;
            if self.option.is_none() {
                self.option = self
                    .options(cx)
                    .and_then(|o| o.options.iter().find(|o| o.qualifies))
                    .map(|o| o.index);
            }
            cx.notify();
            return;
        }
        let extra = match args::split(self.args.read(cx).text()) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(format!("Arguments: {e}").into());
                cx.notify();
                return;
            }
        };
        if self.busy {
            return;
        }
        let app = self.app_id;
        let option = if single { None } else { self.option };
        let stale = self.allow_stale;
        self.busy = true;
        self.error = None;
        // Launch waits without a timeout while tools install (§2.1).
        let call = self
            .model
            .read(cx)
            .daemon
            .call_unbounded(move |c| c.launch(app, option, &extra, stale));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                page.allow_stale = false;
                match r {
                    Ok(_) => page.open = Open::None,
                    Err(e) => page.error = Some(e.message.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_compat(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.compat_error = None;
        let key = format!("compat.tool.{}", self.app_id);
        let call = self
            .model
            .read(cx)
            .daemon
            .call(move |c| c.set_setting(&key, value.as_deref()));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                match r {
                    Ok(s) => {
                        let app = page.app_id;
                        page.model.update(cx, |m, cx| {
                            m.store.set_setting(s);
                            m.read(crate::store::Read::App(app), cx);
                        });
                    }
                    Err(e) => page.compat_error = Some(e.message.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn header(
        &self,
        name: &str,
        has_info: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = self.app_id;
        let high_dpi = window.scale_factor() > 1.;
        let (hero, logo, header) = self.model.update(cx, |m, cx| {
            if !has_info {
                return (None, None, None);
            }
            m.want_asset(id, AssetKind::Hero, high_dpi, cx);
            let hero = m.asset(id, AssetKind::Hero).cloned();
            let mut logo = None;
            let mut header = None;
            if let Some(Asset::Ready(_)) = hero {
                m.want_asset(id, AssetKind::Logo, high_dpi, cx);
                logo = m.asset(id, AssetKind::Logo).cloned();
            } else if let Some(Asset::Missing) = hero {
                m.want_asset(id, AssetKind::Header, high_dpi, cx);
                header = m.asset(id, AssetKind::Header).cloned();
            }
            (hero, logo, header)
        });
        let title = div().text_2xl().text_color(p.text).child(name.to_owned());
        match (hero, logo, header) {
            (Some(Asset::Ready(hero)), logo, _) => div()
                .relative()
                .w_full()
                .h(px(240.))
                .rounded_lg()
                .overflow_hidden()
                .child(img(hero).size_full().object_fit(ObjectFit::Cover))
                .child(
                    div().absolute().left_6().bottom_6().child(match logo {
                        Some(Asset::Ready(logo)) => img(logo)
                            .max_w(px(320.))
                            .max_h(px(140.))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        _ => div()
                            .px_2()
                            .rounded_md()
                            .bg(p.surface.opacity(0.8))
                            .child(title)
                            .into_any_element(),
                    }),
                )
                .into_any_element(),
            (_, _, Some(Asset::Ready(header))) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    img(header)
                        .w(px(460.))
                        .h(px(215.))
                        .rounded_lg()
                        .object_fit(ObjectFit::Cover),
                )
                .child(title)
                .into_any_element(),
            _ => title.into_any_element(),
        }
    }

    /// The state and main action (§4.3).
    fn main(&self, info: Option<&v1::GetAppResponse>, cx: &mut Context<Self>) -> AnyElement {
        let store = &self.model.read(cx).store;
        let app = self.app_id;
        let running = store.running.contains_key(&app);
        let job = store.unfinished_job(app).cloned();
        let installed = info.and_then(|i| i.installed.clone());
        let busy = self.busy;

        let mut out = panel(cx);
        if running {
            out = out.child(muted("Running.", cx)).child(
                div().flex().child(
                    button("stop", "Stop")
                        .danger()
                        .disabled(busy)
                        .on_click(cx.listener(|page, _, _, cx| page.stop(cx))),
                ),
            );
        } else if let Some(job) = job {
            out = out.child(job_row(&self.model, &job, None, cx));
        } else if let Some(i) = &installed {
            let mut buttons = div().flex().gap_2();
            let play_label = if busy && self.open != Open::Install {
                "Launching…"
            } else {
                "Play"
            };
            if i.update_available {
                buttons = buttons
                    .child(
                        button("update", "Update")
                            .disabled(busy)
                            .on_click(cx.listener(|page, _, _, cx| page.update(cx))),
                    )
                    .child(
                        button("play-anyway", "Play anyway")
                            .secondary()
                            .disabled(busy)
                            .on_click(cx.listener(|page, _, window, cx| {
                                page.allow_stale = true;
                                page.play(window, cx);
                            })),
                    );
            } else {
                buttons = buttons.child(
                    button("play", play_label)
                        .disabled(busy)
                        .on_click(cx.listener(|page, _, window, cx| page.play(window, cx))),
                );
            }
            out = out
                .when(i.update_available, |d| {
                    d.child(muted("An update is available.", cx))
                })
                .child(buttons)
                .child(div().w(px(420.)).child(self.args.clone()));
            if self.open == Open::Launch {
                out = out.child(self.launch_chooser(cx));
            }
        } else if info.is_some() {
            out = out.child(muted("Not installed.", cx)).child(
                div().flex().child(
                    button("install", "Install")
                        .disabled(busy || self.open == Open::Install)
                        .on_click(cx.listener(|page, _, _, cx| page.open_install(cx))),
                ),
            );
            if self.open == Open::Install {
                out = out.child(self.install_chooser(info, cx));
            }
        } else {
            out = out.child(muted("Reading the app…", cx));
        }
        out.when_some(self.error.clone(), |d, e| d.child(error_text(e, cx)))
            .into_any_element()
    }

    fn install_chooser(
        &self,
        info: Option<&v1::GetAppResponse>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folders = self.model.read(cx).store.folders.clone();
        let default_folder = folders
            .iter()
            .find(|f| f.is_default)
            .map(|f| f.path.clone());
        let folder = self.folder.clone().or(default_folder);
        let mut folder_list = div().flex().flex_col().gap_1();
        for (i, f) in folders.iter().enumerate() {
            let path = f.path.clone();
            let detail = if f.available {
                format!("{} free", format::size(f.free_bytes))
            } else {
                "not available".to_owned()
            };
            folder_list = folder_list.child(
                choice(
                    ("folder", i),
                    f.path.clone(),
                    folder.as_deref() == Some(&f.path),
                )
                .detail(detail)
                .disabled(!f.available)
                .on_choose(cx.listener(move |page, _, _, cx| {
                    page.folder = Some(path.clone());
                    cx.notify();
                })),
            );
        }

        let mut branches: Vec<&v1::Branch> = info
            .map(|i| i.branches.iter().filter(|b| !b.password_required).collect())
            .unwrap_or_default();
        branches.sort_by_key(|b| (b.name != "public", b.name.clone()));
        let branch = self.branch.clone().unwrap_or_else(|| "public".to_owned());
        let mut branch_list = div().flex().flex_col().gap_1();
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
                choice(("branch", i), b.name.clone(), branch == b.name)
                    .detail(detail)
                    .on_choose(cx.listener(move |page, _, _, cx| {
                        page.branch = Some(name.clone());
                        cx.notify();
                    })),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section("Folder", cx).child(folder_list))
            .when(branches.len() > 1, |d| {
                d.child(section("Branch", cx).child(branch_list))
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("confirm-install", "Install")
                            .disabled(self.busy)
                            .on_click(cx.listener(|page, _, _, cx| page.install(cx))),
                    )
                    .child(
                        button("cancel-install", "Cancel")
                            .secondary()
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.open = Open::None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn launch_chooser(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = self
            .options(cx)
            .map(|o| o.options.clone())
            .unwrap_or_default();
        let mut list = div().flex().flex_col().gap_1();
        for o in options.iter().filter(|o| o.qualifies) {
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
                    ("launch-option", index as usize),
                    label,
                    self.option == Some(index),
                )
                .detail(detail)
                .on_choose(cx.listener(move |page, _, _, cx| {
                    page.option = Some(index);
                    cx.notify();
                })),
            );
        }
        section("Launch option", cx)
            .child(list)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("launch", "Launch")
                            .disabled(self.busy || self.option.is_none())
                            .on_click(cx.listener(|page, _, window, cx| page.play(window, cx))),
                    )
                    .child(
                        button("cancel-launch", "Cancel")
                            .secondary()
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.open = Open::None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn details(&self, info: &v1::GetAppResponse, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mut out = panel(cx);
        if let Some(i) = &info.installed {
            out = out
                .child(row("Path", i.path.clone(), cx))
                .child(row("Folder", i.folder.clone(), cx))
                .child(row("Branch", i.branch.clone(), cx))
                .child(row("Build", i.build_id.to_string(), cx))
                .child(row("Size", format::size(i.size), cx));
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
                out = out.child(row("Depots", depots.join(", "), cx));
            }
        } else if !info.install_dir.is_empty() {
            out = out.child(row("Directory", info.install_dir.clone(), cx));
        }
        if !info.oslist.is_empty() {
            out = out.child(row("Platforms", info.oslist.join(", "), cx));
        }
        if info.missing_token {
            out = out.child(muted(
                "Steam did not give this account the token for the app's full info.",
                cx,
            ));
        }
        let dlc = info.app.as_ref().map(|a| a.dlc.clone()).unwrap_or_default();
        if !dlc.is_empty() {
            let mut list = div().flex().flex_col();
            for d in dlc {
                let name = if d.name.is_empty() {
                    format!("App {}", d.app_id)
                } else {
                    d.name.clone()
                };
                list = list.child(div().flex().gap_2().text_sm().child(name).child(
                    div().text_xs().text_color(p.muted).child(if d.installed {
                        "installed"
                    } else {
                        "not installed"
                    }),
                ));
            }
            out = out.child(section("DLC", cx).child(list));
        }
        out.into_any_element()
    }

    /// Linux: the compatibility tool (§4.3).
    fn compat(&self, c: &v1::Compat, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let tool = match &c.tool {
            Some(t) => format!("{} ({})", tool_label(t), source_text(c.source())),
            None => "none, it runs natively".to_owned(),
        };
        let mut out = panel(cx).child(row("Tool", tool, cx));
        if !c.chain.is_empty() {
            let mut chain = div().flex().flex_col();
            for l in &c.chain {
                let t = l.tool.as_ref().map(tool_label).unwrap_or_default();
                chain = chain.child(div().flex().gap_2().text_sm().child(t).child(
                    div().text_xs().text_color(p.muted).child(if l.installed {
                        "installed"
                    } else {
                        "not installed"
                    }),
                ));
            }
            out = out.child(section("Chain", cx).child(chain));
        }
        if !c.platform.is_empty() {
            out = out.child(row("Installed for", c.platform.clone(), cx));
        }
        let user = c.source() == CompatSource::User;
        let current = c.tool.as_ref().map(|t| t.name.clone());
        let mut choices = div().flex().flex_col().gap_1().child(
            choice("compat-default", "Default", !user)
                .detail("Valve's mapping, or the default tool")
                .on_choose(cx.listener(|page, _, _, cx| page.set_compat(None, cx))),
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
                    ("compat", i),
                    label,
                    user && current.as_deref() == Some(&ch.name),
                )
                .on_choose(
                    cx.listener(move |page, _, _, cx| page.set_compat(Some(name.clone()), cx)),
                ),
            );
        }
        out.child(section("Use", cx).child(choices))
            .when_some(self.compat_error.clone(), |d, e| d.child(error_text(e, cx)))
            .into_any_element()
    }

    fn manage(&self, info: &v1::GetAppResponse, cx: &mut Context<Self>) -> Option<AnyElement> {
        info.installed.as_ref()?;
        let busy = self.busy;
        let mut out = panel(cx).child(
            div()
                .flex()
                .gap_2()
                .child(
                    button("verify", "Verify")
                        .secondary()
                        .disabled(busy)
                        .on_click(cx.listener(|page, _, _, cx| page.verify(cx))),
                )
                .child(
                    button("uninstall", "Uninstall…")
                        .danger()
                        .disabled(busy || self.open == Open::Uninstall)
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.open = Open::Uninstall;
                            page.remove_compat_data = false;
                            cx.notify();
                        })),
                ),
        );
        if self.open == Open::Uninstall {
            let name = info
                .app
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_default();
            out = out
                .child(muted(
                    format!("Uninstall {name}? Its files are deleted."),
                    cx,
                ))
                .when(info.compat.is_some(), |d| {
                    d.child(
                        switch(
                            "remove-compat-data",
                            "Also delete the Proton prefix",
                            self.remove_compat_data,
                        )
                        .on_toggle(cx.listener(|page, _, _, cx| {
                            page.remove_compat_data = !page.remove_compat_data;
                            cx.notify();
                        })),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button("confirm-uninstall", "Uninstall")
                                .danger()
                                .disabled(busy)
                                .on_click(cx.listener(|page, _, _, cx| page.uninstall(cx))),
                        )
                        .child(button("cancel-uninstall", "Cancel").secondary().on_click(
                            cx.listener(|page, _, _, cx| {
                                page.open = Open::None;
                                cx.notify();
                            }),
                        )),
                );
        }
        Some(out.into_any_element())
    }

    /// The app's last ShimReport (§4.3, §5).
    fn last_run(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self
            .model
            .read(cx)
            .store
            .shim_reports
            .get(&self.app_id)?
            .clone();
        let p = palette(cx);
        let n = r.interfaces.len();
        let mut out = panel(cx).child(muted(
            format!(
                "The Steamworks library handed out {n} interface version{}.",
                if n == 1 { "" } else { "s" }
            ),
            cx,
        ));
        if !r.interfaces.is_empty() {
            out = out.child(row("Interfaces", r.interfaces.join(", "), cx));
        }
        if !r.unknown_versions.is_empty() {
            out = out.child(row("Not provided", r.unknown_versions.join(", "), cx));
        }
        if r.unknown_versions.is_empty() && r.unimplemented.is_empty() {
            out = out.child(muted("Everything the game called is implemented.", cx));
        }
        if !r.unimplemented.is_empty() {
            let mut list = div().flex().flex_col();
            for c in &r.unimplemented {
                let name = if c.interface == "exports" {
                    c.method.clone()
                } else {
                    format!("{}::{}", c.interface, c.method)
                };
                list = list.child(
                    div()
                        .flex()
                        .gap_3()
                        .text_sm()
                        .child(
                            div()
                                .w(px(64.))
                                .text_color(p.muted)
                                .child(c.count.to_string()),
                        )
                        .child(name),
                );
            }
            out = out.child(
                section("Unimplemented calls (they returned failure values)", cx).child(list),
            );
        }
        Some(out.into_any_element())
    }
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

impl Focusable for AppPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = &self.model.read(cx).store;
        let details = store.apps.get(&self.app_id);
        let load_error = details
            .and_then(|d| d.info.as_ref())
            .and_then(|r| r.as_ref().err())
            .map(|e| e.message.clone());
        let info = self.info(cx).cloned();
        let name = info
            .as_ref()
            .and_then(|i| i.app.as_ref())
            .map(|a| a.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| store.app_name(self.app_id).map(str::to_owned))
            .unwrap_or_else(|| format!("App {}", self.app_id));
        let has_info = info
            .as_ref()
            .and_then(|i| i.app.as_ref())
            .is_some_and(|a| a.has_info);

        let header = self.header(&name, has_info, window, cx);
        let main = self.main(info.as_ref(), cx);
        let mut body = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div().flex().child(
                    button("back", "‹ Library")
                        .ghost()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(AppPageEvent::Close))),
                ),
            )
            .child(header)
            .when_some(load_error, |d, e| d.child(error_text(e, cx)))
            .child(main);
        if let Some(info) = &info {
            body = body.child(section("Details", cx).child(self.details(info, cx)));
            if let Some(c) = &info.compat {
                body = body.child(section("Compatibility", cx).child(self.compat(c, cx)));
            }
            if let Some(m) = self.manage(info, cx) {
                body = body.child(section("Manage", cx).child(m));
            }
        }
        if let Some(r) = self.last_run(cx) {
            body = body.child(section("Last run", cx).child(r));
        }
        div()
            .id("app-page")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::back))
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .child(body)
    }
}
