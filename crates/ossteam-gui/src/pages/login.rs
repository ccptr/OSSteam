// SPDX-License-Identifier: GPL-2.0-or-later
//! The Login page (`gui.md` §4.1): remembered accounts, password and QR
//! sign-in, and Steam Guard prompts.

use ossteam_ipc::v1::{self, LoginPromptKind};
use ossteam_ipc::{Client, ClientError};
use ossteam_types::Secret;
use ossteam_ui::element::{FocusHandle, ScrollHandle, panel};
use ossteam_ui::style::{Align, Justify, Styled};
use ossteam_ui::text_field::TextField;
use ossteam_ui::window::{Cx, ViewCx};

use crate::gui::Gui;
use crate::model::Model;
use crate::store::{Login, LoginOutcome, Store};
use crate::theme::Palette;
use crate::widgets::{
    self, El, button, error_text, group, muted, qr_code, scrolling_page, section, switch,
    text_field, title,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Password,
    Qr,
}

pub struct LoginPage {
    scroll: ScrollHandle,
    tab: Tab,
    username: TextField,
    password: TextField,
    code: TextField,
    /// A code prompt was shown at the last build.
    code_shown: bool,
    /// A login was running at the last build.
    running: bool,
    remember: bool,
    /// The error of this page's last request.
    error: Option<String>,
    /// A request is in flight.
    busy: bool,
}

impl Default for LoginPage {
    fn default() -> Self {
        let mut password = TextField::new("Password", widgets::TEXT);
        password.set_secret(true);
        let mut code = TextField::new("Code", widgets::TEXT);
        code.set_secret(true);
        Self {
            scroll: ScrollHandle::new(),
            tab: Tab::Password,
            username: TextField::new("Account name", widgets::TEXT),
            password,
            code,
            code_shown: false,
            running: false,
            remember: true,
            error: None,
            busy: false,
        }
    }
}

pub fn page_focus() -> FocusHandle {
    FocusHandle::named("login-page")
}

fn username_focus() -> FocusHandle {
    FocusHandle::named("login-username")
}

fn password_focus() -> FocusHandle {
    FocusHandle::named("login-password")
}

fn code_focus() -> FocusHandle {
    FocusHandle::named("login-code")
}

/// The buttons that start a login. While it runs, its Cancel takes the
/// focus handle of the one that started it, which gets it back if the
/// form returns.
const SIGN_IN: &str = "sign-in";
const SIGN_IN_QR: &str = "sign-in-qr";

/// The field the cursor goes to: the first one still empty.
pub fn first_field(g: &Gui) -> FocusHandle {
    if g.login.username.text().is_empty() {
        username_focus()
    } else {
        password_focus()
    }
}

/// The account a successful login ended with, if the last one did.
pub fn logged_on(store: &Store) -> Option<&v1::Account> {
    match &store.login.ended {
        Some(LoginOutcome::LoggedOn(a)) => Some(a),
        _ => None,
    }
}

fn code_prompt_shown(login: &Login) -> bool {
    login.prompt(LoginPromptKind::DeviceCode).is_some()
        || login.prompt(LoginPromptKind::EmailCode).is_some()
}

/// A code prompt replaces the password form, whose field had focus: the
/// code field takes it, unless focus is elsewhere. A login that ends
/// without signing in brings the form back, and the password field (the
/// first one still empty) takes focus, so a refused password can be
/// typed again at once; on the QR tab, Show QR code takes it.
pub fn before_view(g: &mut Gui, cx: &mut Cx<Gui>) {
    let running = g.model().is_some_and(|m| m.store.login.running);
    let shown = running && g.model().is_some_and(|m| code_prompt_shown(&m.store.login));
    let here = g.showing_login() && (cx.focused().is_none() || cx.contains_focused(page_focus()));
    if shown && !g.login.code_shown && here {
        cx.focus(code_focus());
    }
    if g.login.running && !running && here {
        let target = match g.login.tab {
            Tab::Password => first_field(g),
            Tab::Qr => FocusHandle::named(SIGN_IN_QR),
        };
        if cx.focus_visible() {
            cx.focus_from_keyboard(target);
        } else {
            cx.focus(target);
        }
    }
    g.login.code_shown = shown;
    g.login.running = running;
}

fn sign_in(g: &mut Gui) {
    let running = g.model().is_some_and(|m| m.store.login.running);
    let page = &mut g.login;
    if page.busy || running {
        return;
    }
    let username = page.username.text().trim().to_owned();
    if username.is_empty() {
        page.error = Some("Enter the account name.".into());
        return;
    }
    if page.password.text().is_empty() {
        page.error = Some("Enter the password.".into());
        return;
    }
    // The field is cleared, and its buffer wiped, as the request goes
    // (IPC-10).
    let password = Secret::new(page.password.text().to_owned());
    page.password.set_text("");
    let remember = page.remember;
    start(g, move |c| {
        c.login_with_credentials(&username, &password, remember)
    });
}

fn sign_in_with_qr(g: &mut Gui) {
    let running = g.model().is_some_and(|m| m.store.login.running);
    if g.login.busy || running {
        return;
    }
    let remember = g.login.remember;
    start(g, move |c| c.login_with_qr(remember));
}

/// Starts a login. The store takes prompts from the moment the request
/// goes, because they can overtake the reply on their way (§3).
fn start(g: &mut Gui, f: impl FnOnce(&Client) -> Result<(), ClientError> + Send + 'static) {
    let Some(m) = g.model_mut() else {
        return;
    };
    m.store.login_started();
    g.login.error = None;
    g.login.busy = true;
    g.call(f, |g, r, _| {
        g.login.busy = false;
        if let Err(e) = r {
            if let Some(m) = g.model_mut() {
                m.store.login_stopped(None);
            }
            g.login.error = Some(e.message);
        }
    });
}

fn submit_code(g: &mut Gui) {
    let Some(m) = g.model() else {
        return;
    };
    let login = &m.store.login;
    let kind = if login.prompt(LoginPromptKind::DeviceCode).is_some() {
        LoginPromptKind::DeviceCode
    } else if login.prompt(LoginPromptKind::EmailCode).is_some() {
        LoginPromptKind::EmailCode
    } else {
        return;
    };
    let page = &mut g.login;
    let code = Secret::new(page.code.text().trim().to_owned());
    if code.is_empty() || page.busy {
        return;
    }
    page.code.set_text("");
    page.error = None;
    page.busy = true;
    g.call(
        move |c| c.submit_guard_code(&code, kind),
        |g, r, _| {
            g.login.busy = false;
            if let Err(e) = r {
                g.login.error = Some(e.message);
            }
        },
    );
}

fn cancel(g: &mut Gui) {
    g.login.error = None;
    g.call(
        |c| c.cancel_login(),
        |g, r, _| {
            if let Some(m) = g.model_mut() {
                m.store.login_stopped(None);
            }
            if let Err(e) = r {
                g.login.error = Some(e.message);
            }
        },
    );
}

fn switch_to(g: &mut Gui, steam_id: u64) {
    if g.login.busy {
        return;
    }
    g.login.error = None;
    g.login.busy = true;
    g.call(
        move |c| c.switch_account(steam_id),
        |g, r, _| {
            g.login.busy = false;
            if let Err(e) = r {
                g.login.error = Some(e.message);
            }
        },
    );
}

fn accounts(g: &Gui, m: &Model, p: Palette) -> Option<El> {
    let accounts = &m.store.accounts.accounts;
    if accounts.is_empty() {
        return None;
    }
    let mut list = panel().gap(4.0);
    for a in accounts {
        let id = a.steam_id;
        let label = if a.persona_name.is_empty() {
            a.account_name.clone()
        } else {
            format!("{} ({})", a.persona_name, a.account_name)
        };
        list = list.child(
            button(p, widgets::id("account", id), label)
                .secondary()
                .full_width()
                .disabled(g.login.busy)
                .on_click(move |g, _| switch_to(g, id)),
        );
    }
    Some(section(p, "Accounts", list))
}

fn form(g: &Gui, p: Palette, cx: &ViewCx) -> El {
    let page = &g.login;
    let tabs = panel()
        .row()
        .gap(4.0)
        .child(
            button(p, "tab-password", "Password")
                .ghost()
                .selected(page.tab == Tab::Password)
                .on_click(|g, _| g.login.tab = Tab::Password),
        )
        .child(
            button(p, "tab-qr", "QR code")
                .ghost()
                .selected(page.tab == Tab::Qr)
                .on_click(|g, _| g.login.tab = Tab::Qr),
        );
    let remember = switch(p, "remember", "Remember me", page.remember)
        .on_toggle(|g, _| g.login.remember = !g.login.remember);
    let body = match page.tab {
        Tab::Password => panel()
            .gap(8.0)
            .child(
                text_field(p, cx, username_focus(), |g: &mut Gui| &mut g.login.username)
                    .on_submit(|g, _| sign_in(g)),
            )
            .child(
                text_field(p, cx, password_focus(), |g: &mut Gui| &mut g.login.password)
                    .on_submit(|g, _| sign_in(g)),
            )
            .child(remember)
            .child(
                button(
                    p,
                    SIGN_IN,
                    if page.busy {
                        "Signing in…"
                    } else {
                        "Sign in"
                    },
                )
                .disabled(page.busy)
                .on_click(|g, _| sign_in(g)),
            ),
        Tab::Qr => panel()
            .gap(8.0)
            .child(muted(
                p,
                "Sign in by scanning a code with the Steam mobile app.",
            ))
            .child(remember)
            .child(
                button(p, SIGN_IN_QR, "Show QR code")
                    .disabled(page.busy)
                    .on_click(|g, _| sign_in_with_qr(g)),
            ),
    };
    panel().gap(12.0).child(tabs).child(body)
}

/// What a running login asks for (§4.1, Steam Guard).
fn prompts(g: &Gui, m: &Model, p: Palette, cx: &ViewCx) -> El {
    let login = &m.store.login;
    let mut out = panel().gap(12.0);
    if login.prompts.is_empty() {
        out = out.child(muted(p, "Signing in…"));
    }
    if let Some(pr) = login.prompt(LoginPromptKind::QrUrl) {
        out = out
            .child(
                panel()
                    .row()
                    .justify(Justify::Center)
                    .child(qr_code(p, &pr.qr_url, 220.0)),
            )
            .child(muted(p, "Scan with the Steam mobile app."));
    }
    let code_prompt = login
        .prompt(LoginPromptKind::DeviceCode)
        .or_else(|| login.prompt(LoginPromptKind::EmailCode));
    if let Some(pr) = code_prompt {
        let words = match pr.kind() {
            LoginPromptKind::EmailCode if !pr.hint.is_empty() => {
                format!("Steam emailed a code to your address at {}.", pr.hint)
            }
            LoginPromptKind::EmailCode => "Steam emailed you a code.".to_owned(),
            _ => "Enter the code from the Steam mobile app.".to_owned(),
        };
        out = out.child(muted(p, words)).child(
            panel()
                .row()
                .align(Align::Center)
                .gap(8.0)
                .child(
                    text_field(p, cx, code_focus(), |g: &mut Gui| &mut g.login.code)
                        .grow()
                        .on_submit(|g, _| submit_code(g)),
                )
                .child(
                    button(p, "submit-code", "Submit")
                        .disabled(g.login.busy)
                        .on_click(|g, _| submit_code(g)),
                ),
        );
    }
    if let Some(pr) = login.prompt(LoginPromptKind::DeviceConfirmation) {
        let words = if pr.hint == "confirm in email" {
            "confirm the sign-in through the link Steam emailed you."
        } else {
            "approve the sign-in in the Steam mobile app."
        };
        let words = if code_prompt.is_some() {
            format!("Or {words}")
        } else {
            capitalise(words)
        };
        out = out.child(muted(p, words));
    }
    let started_by = match g.login.tab {
        Tab::Password => SIGN_IN,
        Tab::Qr => SIGN_IN_QR,
    };
    out.child(
        button(p, "cancel-login", "Cancel")
            .secondary()
            .focus(FocusHandle::named(started_by))
            .on_click(|g, _| cancel(g)),
    )
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

pub fn view(g: &Gui, m: &Model, p: Palette, cx: &ViewCx) -> El {
    let store = &m.store;
    let ended = match &store.login.ended {
        Some(LoginOutcome::Failed(e)) => Some(e.clone()),
        _ => None,
    };
    let reason = (!store.status.session_reason.is_empty() && store.account().is_some())
        .then(|| store.status.session_reason.clone());
    let main = if store.login.running {
        prompts(g, m, p, cx)
    } else {
        form(g, p, cx)
    };
    let body = panel()
        .width(420.0)
        .gap(16.0)
        .child(title("Sign in to Steam"))
        .when_some(reason, |d, r| d.child(muted(p, r)))
        .when_some(accounts(g, m, p), |d, a| d.child(a))
        .child(
            group(p)
                .when_some(ended, |d, e| d.child(error_text(p, e)))
                .when_some(g.login.error.clone(), |d, e| d.child(error_text(p, e)))
                .child(main),
        );
    scrolling_page(&g.login.scroll)
        .id("login-page")
        .focus(page_focus())
        .fill()
        .child(panel().row().justify(Justify::Center).pad(24.0).child(body))
}
