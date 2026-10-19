// SPDX-License-Identifier: GPL-2.0-or-later
//! The Login page (`gui.md` §4.1): remembered accounts, password and QR
//! sign-in, and Steam Guard prompts.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use ossteam_ipc::v1::{self, LoginPromptKind};
use ossteam_types::Secret;

use crate::model::Model;
use crate::pages::{error_text, muted, panel, section};
use crate::store::LoginOutcome;
use crate::theme::palette;
use crate::widgets::{TextInput, TextInputEvent, button, qr, reveal, switch};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Password,
    Qr,
}

pub struct LoginPage {
    model: Entity<Model>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    tab: Tab,
    username: Entity<TextInput>,
    password: Entity<TextInput>,
    code: Entity<TextInput>,
    /// A code prompt was drawn last frame.
    code_shown: bool,
    remember: bool,
    /// The error of this page's last request.
    error: Option<SharedString>,
    /// A request is in flight.
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl LoginPage {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username = TextInput::build("Account name", window, cx);
        let password = cx.new(|cx| TextInput::new("Password", cx).secret());
        let code = cx.new(|cx| TextInput::new("Code", cx).secret());
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(&password, window, |this, _, e, window, cx| {
                if *e == TextInputEvent::Submitted {
                    this.sign_in(window, cx);
                }
            }),
            cx.subscribe_in(&username, window, |this, _, e, window, cx| {
                if *e == TextInputEvent::Submitted {
                    this.sign_in(window, cx);
                }
            }),
            cx.subscribe_in(&code, window, |this, _, e, _, cx| {
                if *e == TextInputEvent::Submitted {
                    this.submit_code(cx);
                }
            }),
        ];
        Self {
            model,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            tab: Tab::Password,
            username,
            password,
            code,
            code_shown: false,
            remember: true,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    /// The field the cursor goes to: the first one still empty.
    pub fn first_field(&self, cx: &App) -> FocusHandle {
        if self.username.read(cx).text().is_empty() {
            self.username.focus_handle(cx)
        } else {
            self.password.focus_handle(cx)
        }
    }

    fn sign_in(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.model.read(cx).store.login.running {
            return;
        }
        let username = self.username.read(cx).text().trim().to_owned();
        if username.is_empty() {
            self.error = Some("Enter the account name.".into());
            cx.notify();
            return;
        }
        if self.password.read(cx).text().is_empty() {
            self.error = Some("Enter the password.".into());
            cx.notify();
            return;
        }
        // The field is cleared, and its buffer zeroed, as the request goes.
        let password = Secret::new(self.password.read(cx).text().to_owned());
        self.password.update(cx, |p, cx| p.clear(cx));
        let remember = self.remember;
        self.start(cx, move |c| {
            c.login_with_credentials(&username, &password, remember)
        });
    }

    fn sign_in_with_qr(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.model.read(cx).store.login.running {
            return;
        }
        let remember = self.remember;
        self.start(cx, move |c| c.login_with_qr(remember));
    }

    /// Starts a login. The store takes prompts from the moment the request
    /// goes, because they can overtake the reply on their way (§3).
    fn start(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&ossteam_ipc::Client) -> Result<(), ossteam_ipc::ClientError> + Send + 'static,
    ) {
        self.error = None;
        self.busy = true;
        let call = self.model.update(cx, |m, cx| {
            m.store.login_started();
            cx.notify();
            m.daemon.call(f)
        });
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                if let Err(e) = r {
                    page.model.update(cx, |m, cx| {
                        m.store.login_stopped(None);
                        cx.notify();
                    });
                    page.error = Some(e.message.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_code(&mut self, cx: &mut Context<Self>) {
        let store = &self.model.read(cx).store;
        let kind = if store.login.prompt(LoginPromptKind::DeviceCode).is_some() {
            LoginPromptKind::DeviceCode
        } else if store.login.prompt(LoginPromptKind::EmailCode).is_some() {
            LoginPromptKind::EmailCode
        } else {
            return;
        };
        let code = Secret::new(self.code.read(cx).text().trim().to_owned());
        if code.is_empty() || self.busy {
            return;
        }
        self.code.update(cx, |c, cx| c.clear(cx));
        self.error = None;
        self.busy = true;
        let call = self
            .model
            .read(cx)
            .daemon
            .call(move |c| c.submit_guard_code(&code, kind));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                if let Err(e) = r {
                    page.error = Some(e.message.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let call = self.model.read(cx).daemon.call(|c| c.cancel_login());
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.model.update(cx, |m, cx| {
                    m.store.login_stopped(None);
                    cx.notify();
                });
                if let Err(e) = r {
                    page.error = Some(e.message.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_to(&mut self, steam_id: u64, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.error = None;
        self.busy = true;
        let call = self
            .model
            .read(cx)
            .daemon
            .call(move |c| c.switch_account(steam_id));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                page.busy = false;
                if let Err(e) = r {
                    page.error = Some(e.message.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn accounts(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let store = &self.model.read(cx).store;
        if store.accounts.accounts.is_empty() {
            return None;
        }
        let mut list = div().flex().flex_col().gap_1();
        for a in store.accounts.accounts.clone() {
            let id = a.steam_id;
            let label = if a.persona_name.is_empty() {
                a.account_name.clone()
            } else {
                format!("{} ({})", a.persona_name, a.account_name)
            };
            list = list.child(
                button(SharedString::from(format!("account-{id}")), label)
                    .secondary()
                    .full_width()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |page, _, _, cx| page.switch_to(id, cx))),
            );
        }
        Some(section("Accounts", cx).child(list).into_any_element())
    }

    fn form(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tabs = div()
            .flex()
            .gap_1()
            .child(
                button("tab-password", "Password")
                    .ghost()
                    .selected(self.tab == Tab::Password)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.tab = Tab::Password;
                        cx.notify();
                    })),
            )
            .child(
                button("tab-qr", "QR code")
                    .ghost()
                    .selected(self.tab == Tab::Qr)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.tab = Tab::Qr;
                        cx.notify();
                    })),
            );
        let remember = switch("remember", "Remember me", self.remember).on_toggle(cx.listener(
            |page, _, _, cx| {
                page.remember = !page.remember;
                cx.notify();
            },
        ));
        let body = match self.tab {
            Tab::Password => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.username.clone())
                .child(self.password.clone())
                .child(remember)
                .child(
                    button(
                        "sign-in",
                        if self.busy {
                            "Signing in…"
                        } else {
                            "Sign in"
                        },
                    )
                    .disabled(self.busy)
                    .on_click(cx.listener(|page, _, window, cx| page.sign_in(window, cx))),
                ),
            Tab::Qr => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(muted(
                    "Sign in by scanning a code with the Steam mobile app.",
                    cx,
                ))
                .child(remember)
                .child(
                    button("sign-in-qr", "Show QR code")
                        .disabled(self.busy)
                        .on_click(cx.listener(|page, _, _, cx| page.sign_in_with_qr(cx))),
                ),
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    /// What a running login asks for (§4.1, Steam Guard).
    fn prompts(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let login = &self.model.read(cx).store.login;
        let mut out = div().flex().flex_col().gap_3();
        if login.prompts.is_empty() {
            out = out.child(muted("Signing in…", cx));
        }
        if let Some(p) = login.prompt(LoginPromptKind::QrUrl) {
            out = out
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .child(qr::qr_code(&p.qr_url, 220., cx)),
                )
                .child(muted("Scan with the Steam mobile app.", cx));
        }
        let code_prompt = login
            .prompt(LoginPromptKind::DeviceCode)
            .or_else(|| login.prompt(LoginPromptKind::EmailCode))
            .cloned();
        if let Some(p) = code_prompt {
            let text = match p.kind() {
                LoginPromptKind::EmailCode if !p.hint.is_empty() => {
                    format!("Steam emailed a code to your address at {}.", p.hint)
                }
                LoginPromptKind::EmailCode => "Steam emailed you a code.".to_owned(),
                _ => "Enter the code from the Steam mobile app.".to_owned(),
            };
            out = out.child(muted(text, cx)).child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.code.clone()))
                    .child(
                        button("submit-code", "Submit")
                            .disabled(self.busy)
                            .on_click(cx.listener(|page, _, _, cx| page.submit_code(cx))),
                    ),
            );
        }
        if let Some(p) = login.prompt(LoginPromptKind::DeviceConfirmation) {
            let text = if p.hint == "confirm in email" {
                "Or confirm the sign-in through the link Steam emailed you."
            } else {
                "Or approve the sign-in in the Steam mobile app."
            };
            let text = if code_prompt_shown(login) {
                text
            } else {
                text.trim_start_matches("Or ")
            };
            out = out.child(muted(capitalise(text), cx));
        }
        out.child(
            div().flex().child(
                button("cancel-login", "Cancel")
                    .secondary()
                    .on_click(cx.listener(|page, _, _, cx| page.cancel(cx))),
            ),
        )
        .into_any_element()
    }
}

fn code_prompt_shown(login: &crate::store::Login) -> bool {
    login.prompt(LoginPromptKind::DeviceCode).is_some()
        || login.prompt(LoginPromptKind::EmailCode).is_some()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

impl Focusable for LoginPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LoginPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        reveal::scroll_to_focus(&self.focus, &self.scroll, window, cx);
        let p = palette(cx);
        let store = &self.model.read(cx).store;
        let running = store.login.running;
        let ended = match &store.login.ended {
            Some(LoginOutcome::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        let reason = (!store.status.session_reason.is_empty() && store.account().is_some())
            .then(|| store.status.session_reason.clone());
        // A code prompt replaces the password form, whose field had
        // focus: the code field takes it, unless focus is elsewhere.
        let code_shown = running && code_prompt_shown(&store.login);
        if code_shown && !self.code_shown {
            let here = window.focused(cx).is_none() || self.focus.contains_focused(window, cx);
            if here {
                window.focus(&self.code.focus_handle(cx), cx);
            }
        }
        self.code_shown = code_shown;
        let accounts = self.accounts(cx);
        let main = if running {
            self.prompts(cx)
        } else {
            self.form(cx)
        };
        div()
            .id("login-page")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .justify_center()
            .p_6()
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_xl().text_color(p.text).child("Sign in to Steam"))
                    .when_some(reason, |d, r| d.child(muted(r, cx)))
                    .when_some(accounts, |d, a| d.child(a))
                    .child(
                        panel(cx)
                            .when_some(ended, |d, e| d.child(error_text(e, cx)))
                            .when_some(self.error.clone(), |d, e| d.child(error_text(e, cx)))
                            .child(main),
                    ),
            )
    }
}

/// The account a successful login ended with, if the last one did.
pub fn logged_on(store: &crate::store::Store) -> Option<&v1::Account> {
    match &store.login.ended {
        Some(LoginOutcome::LoggedOn(a)) => Some(a),
        _ => None,
    }
}
