// SPDX-License-Identifier: GPL-2.0-or-later
//! The toolkit's pieces in one window: buttons with hover and focus
//! styles, a text input, a menu overlay, a 10,000-row list with images,
//! a scrolling page, a tooltip, key bindings and a timer.
//!
//! `cargo run -p ossteam-ui --example gallery -- [--renderer auto|vulkan|software]
//! [--exit-after-ms N]`. It prints one line of JSON when the first frame
//! has been presented.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ossteam_ui::app::{self, Options, Sender};
use ossteam_ui::appearance::Appearance;
use ossteam_ui::element::{
    Element, FocusHandle, ScrollHandle, image, list, overlay, panel, text, text_input,
};
use ossteam_ui::image::Image;
use ossteam_ui::keys::KeyBinding;
use ossteam_ui::render::Choice;
use ossteam_ui::scene::Color;
use ossteam_ui::style::{Align, Edges, Justify, Shadow, Styled};
use ossteam_ui::text_field::{FieldStyle, TextField};
use ossteam_ui::window::{App, Cx, Timer, ViewCx};

const ROWS: usize = 10_000;

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    surface: Color,
    raised: Color,
    border: Color,
    text: Color,
    muted: Color,
    accent: Color,
    on_accent: Color,
    ring: Color,
}

impl Palette {
    fn for_appearance(a: Option<Appearance>) -> Self {
        match a {
            Some(Appearance::Dark) => Self {
                background: Color::rgb(0x16181d),
                surface: Color::rgb(0x1f2229),
                raised: Color::rgb(0x2a2e37),
                border: Color::rgb(0x3a3f4b),
                text: Color::rgb(0xe6e8ec),
                muted: Color::rgb(0x9aa1ad),
                accent: Color::rgb(0x4c8dff),
                on_accent: Color::rgb(0xffffff),
                ring: Color::rgb(0x8ab4ff),
            },
            _ => Self {
                background: Color::rgb(0xf4f5f7),
                surface: Color::rgb(0xffffff),
                raised: Color::rgb(0xeceef2),
                border: Color::rgb(0xd0d4dc),
                text: Color::rgb(0x1b1d22),
                muted: Color::rgb(0x5f6673),
                accent: Color::rgb(0x2f6fe4),
                on_accent: Color::rgb(0xffffff),
                ring: Color::rgb(0x1a4fb8),
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    List,
    Page,
}

#[derive(Clone)]
struct Quit;
#[derive(Clone)]
struct CloseMenu;
#[derive(Clone)]
struct FocusSearch;
#[derive(Clone)]
struct RowUp;
#[derive(Clone)]
struct RowDown;
#[derive(Clone)]
struct Open;

enum Msg {
    Started,
}

struct Gallery {
    started: Instant,
    exit_after: Option<Duration>,
    tab: Tab,
    menu: bool,
    search: TextField,
    search_focus: FocusHandle,
    list_focus: FocusHandle,
    menu_focus: FocusHandle,
    list: ScrollHandle,
    page: ScrollHandle,
    selected: usize,
    opened: Option<usize>,
    notice: Option<String>,
    notice_timer: Option<Timer>,
    switches: Vec<bool>,
    icons: Vec<Arc<Image>>,
}

/// A small image drawn from a formula: a diagonal gradient with a dot.
fn icon(size: u32, hue: usize) -> Arc<Image> {
    let colours = [
        (0x2f, 0x6f, 0xe4),
        (0xe4, 0x6f, 0x2f),
        (0x2f, 0xb0, 0x6f),
        (0xa0, 0x4f, 0xd0),
    ];
    let (r, g, b) = colours[hue % colours.len()];
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    let c = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let t = (x + y) as f32 / (2 * size) as f32;
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let dot = (size as f32 * 0.22 - d).clamp(0.0, 1.0);
            let mix = |v: u8| ((v as f32 * (0.6 + 0.4 * t)) * (1.0 - dot) + 255.0 * dot) as u8;
            // Round corners through alpha.
            let corner =
                (c - 0.5 - (x as f32 + 0.5 - c).abs()).min(c - 0.5 - (y as f32 + 0.5 - c).abs());
            let a = if corner < 0.0 { 0 } else { 255 };
            px.extend_from_slice(&[mix(r), mix(g), mix(b), a]);
        }
    }
    Image::from_rgba(size, size, px).expect("size matches")
}

impl Gallery {
    fn button(&self, id: &'static str, label: &str, primary: bool, p: Palette) -> Element<Self> {
        let (bg, fg, hover) = if primary {
            (p.accent, p.on_accent, p.accent.with_alpha(0.85))
        } else {
            (p.raised, p.text, p.border)
        };
        panel()
            .id(id)
            .focus(FocusHandle::named(id))
            .tab_stop(true)
            .row()
            .center()
            .pad_xy(12.0, 4.0)
            .radius(6.0)
            .border(2.0, bg)
            .background(bg)
            .text_color(fg)
            .font_size(14.0)
            .nowrap()
            .cursor(winit::window::CursorIcon::Pointer)
            .hover_style(move |s| s.background(hover))
            .focus_style(move |s| s.border_color(p.ring))
            .child(text(label))
    }

    fn notify(&mut self, cx: &mut Cx<Self>, msg: String) {
        self.notice = Some(msg);
        self.notice_timer = Some(cx.after(Duration::from_secs(3), |s, _| s.notice = None));
    }

    fn header(&self, p: Palette) -> Element<Self> {
        let field = FieldStyle {
            text: p.text,
            placeholder: p.muted,
            selection: p.accent.with_alpha(0.35),
            caret: p.text,
        };
        let tab = |me: &Self, id: &'static str, label: &str, t: Tab| {
            me.button(id, label, me.tab == t, p)
                .on_click(move |s: &mut Self, _| s.tab = t)
        };
        panel()
            .row()
            .align(Align::Center)
            .gap(8.0)
            .pad(12.0)
            .background(p.surface)
            .border_sides(
                Edges {
                    bottom: 1.0,
                    ..Edges::default()
                },
                p.border,
            )
            .child(tab(self, "tab-list", "Library", Tab::List))
            .child(tab(self, "tab-page", "Settings", Tab::Page))
            .child(
                panel()
                    .grow()
                    .height(32.0)
                    .pad_xy(8.0, 0.0)
                    .justify(Justify::Center)
                    .radius(6.0)
                    .border(1.0, p.border)
                    .background(p.background)
                    .focus_style(move |s| s.border_color(p.ring))
                    .child(
                        text_input(self.search_focus, field, |s: &mut Self| &mut s.search)
                            .id("search")
                            .fill_width()
                            .font_size(14.0)
                            .on_change(|s: &mut Self, _| {
                                s.selected = 0;
                                s.list.set_offset(0.0);
                            })
                            .on_submit(|s: &mut Self, cx| {
                                let q = s.search.text().to_owned();
                                s.notify(cx, format!("Searched for {q:?}"));
                            }),
                    ),
            )
            .child(
                panel()
                    .child(
                        self.button("account", "Account ▾", false, p)
                            .tooltip("Switch account or log out")
                            .on_click(|s: &mut Self, cx| {
                                s.menu = !s.menu;
                                if s.menu {
                                    cx.focus_from_keyboard(s.menu_focus);
                                }
                            }),
                    )
                    .child(overlay(self.menu(p)).hidden(!self.menu)),
            )
    }

    fn menu(&self, p: Palette) -> Element<Self> {
        let item = |me: &Self, id: &'static str, label: &'static str| {
            me.button(id, label, false, p)
                .justify(Justify::Start)
                .background(p.surface)
                .border(2.0, p.surface)
                .on_click(move |s: &mut Self, cx| {
                    s.menu = false;
                    s.notify(cx, format!("{label} chosen"));
                })
        };
        panel()
            .id("menu")
            .key_context("Menu")
            .focus(self.menu_focus)
            .width(220.0)
            .margin(Edges {
                top: 4.0,
                ..Edges::default()
            })
            .pad(6.0)
            .gap(2.0)
            .radius(8.0)
            .border(1.0, p.border)
            .background(p.surface)
            .shadow(Shadow {
                color: Color::rgb(0).with_alpha(0.18),
                offset_x: 0.0,
                offset_y: 6.0,
                blur: 8.0,
                spread: 0.0,
            })
            .on_action(|s: &mut Self, _: &CloseMenu, cx| {
                s.menu = false;
                cx.focus_from_keyboard(FocusHandle::named("account"));
            })
            .on_press_outside(|s: &mut Self, _| s.menu = false)
            .child(item(self, "menu-switch", "Switch account"))
            .child(item(self, "menu-add", "Add account"))
            .child(item(self, "menu-logout", "Log out"))
    }

    fn rows(&self, p: Palette) -> Element<Self> {
        let count = if self.search.text().is_empty() {
            ROWS
        } else {
            ROWS / 7
        };
        panel()
            .grow()
            .key_context("List")
            .focus(self.list_focus)
            .tab_stop(true)
            .on_action(move |s: &mut Self, _: &RowUp, _| {
                s.selected = s.selected.saturating_sub(1);
                s.list.reveal_row(s.selected);
            })
            .on_action(move |s: &mut Self, _: &RowDown, _| {
                s.selected = (s.selected + 1).min(count - 1);
                s.list.reveal_row(s.selected);
            })
            .on_action(|s: &mut Self, _: &Open, cx| {
                let i = s.selected;
                s.opened = Some(i);
                s.notify(cx, format!("Opened game {i}"));
            })
            .child(
                list(&self.list, count, move |s: &Self, i, _| {
                    let selected = i == s.selected;
                    panel()
                        .id(("row", i as u64))
                        .row()
                        .align(Align::Center)
                        .gap(10.0)
                        .height(36.0)
                        .pad_xy(12.0, 0.0)
                        .background(if selected { p.raised } else { p.background })
                        .hover_style(move |st| st.background(p.raised))
                        .on_click(move |s: &mut Self, _| s.selected = i)
                        .child(image(s.icons[i % s.icons.len()].clone()).no_shrink())
                        .child(text(format!("Game {i}")).grow())
                        .child(
                            panel()
                                .text_color(p.muted)
                                .font_size(12.0)
                                .child(text(format!("{} MB", (i * 37) % 9000 + 120))),
                        )
                })
                .fill(),
            )
    }

    fn page(&self, p: Palette) -> Element<Self> {
        let mut page = panel()
            .grow()
            .scroll_y(&self.page)
            .pad(16.0)
            .gap(12.0)
            .text_color(p.text);
        page = page.child(
            panel()
                .font_size(20.0)
                .font_weight(700)
                .child(text("Settings")),
        );
        page = page.child(panel().text_color(p.muted).font_size(13.0).child(text(
            "A long page that scrolls. Tab moves through the switches, and the \
             page scrolls to keep the focused one in view; the wheel scrolls it too. \
             This paragraph wraps to the width of the window.",
        )));
        for (i, on) in self.switches.iter().enumerate() {
            let on = *on;
            page = page.child(
                panel()
                    .id(("switch", i as u64))
                    .focus(FocusHandle::named(("switch", i as u64)))
                    .tab_stop(true)
                    .row()
                    .align(Align::Center)
                    .justify(Justify::Between)
                    .pad(10.0)
                    .radius(6.0)
                    .border(2.0, p.surface)
                    .background(p.surface)
                    .focus_style(move |s| s.border_color(p.ring))
                    .on_click(move |s: &mut Self, _| s.switches[i] = !s.switches[i])
                    .child(text(format!("Option {}", i + 1)))
                    .child(
                        panel()
                            .size(36.0, 20.0)
                            .radius(10.0)
                            .pad(2.0)
                            .background(if on { p.accent } else { p.border })
                            .align(if on { Align::End } else { Align::Start })
                            .child(panel().size(16.0, 16.0).radius(8.0).background(p.on_accent)),
                    ),
            );
        }
        page
    }
}

impl App for Gallery {
    type Message = Msg;

    fn background(&self, cx: &ViewCx) -> Color {
        Palette::for_appearance(cx.appearance).background
    }

    fn view(&self, cx: &ViewCx) -> Element<Self> {
        let p = Palette::for_appearance(cx.appearance);
        let body = match self.tab {
            Tab::List => self.rows(p),
            Tab::Page => self.page(p),
        };
        let status = match (&self.notice, self.opened) {
            (Some(n), _) => n.clone(),
            (None, Some(i)) => format!("Last opened: game {i}"),
            (None, None) => format!(
                "{ROWS} games · {:.0}×{:.0} at {}×",
                cx.width, cx.height, cx.scale
            ),
        };
        panel()
            .fill()
            .text_color(p.text)
            .font_size(14.0)
            .on_action(|_: &mut Self, _: &Quit, cx| cx.quit())
            .on_action(|s: &mut Self, _: &FocusSearch, cx| cx.focus_from_keyboard(s.search_focus))
            .child(self.header(p))
            .child(body)
            .child(
                panel()
                    .pad_xy(12.0, 6.0)
                    .background(p.surface)
                    .text_color(p.muted)
                    .font_size(12.0)
                    .border_sides(
                        Edges {
                            top: 1.0,
                            ..Edges::default()
                        },
                        p.border,
                    )
                    .child(text(status)),
            )
    }

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            Msg::Started => {
                let started = self.started;
                cx.on_next_frame(move |_, cx| {
                    println!(
                        "{{\"first_frame_ms\":{:.1},\"scale\":{}}}",
                        started.elapsed().as_secs_f64() * 1000.0,
                        cx.scale()
                    );
                });
                if let Some(after) = self.exit_after {
                    cx.after(after, |_, cx| cx.quit()).detach();
                }
            }
        }
    }

    fn focus_lost(&mut self, cx: &mut Cx<Self>) {
        cx.focus(self.list_focus);
    }
}

fn main() {
    let started = Instant::now();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut renderer = Choice::Auto;
    let mut exit_after = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--renderer" => match args.next().map(|v| v.parse()) {
                Some(Ok(c)) => renderer = c,
                _ => return eprintln!("--renderer auto|vulkan|software"),
            },
            "--exit-after-ms" => match args.next().and_then(|v| v.parse().ok()) {
                Some(ms) => exit_after = Some(Duration::from_millis(ms)),
                None => return eprintln!("--exit-after-ms N"),
            },
            other => return eprintln!("unknown argument {other:?}"),
        }
    }
    let bindings = vec![
        KeyBinding::new("primary+q", Quit, None),
        KeyBinding::new("primary+f", FocusSearch, None),
        KeyBinding::new("escape", CloseMenu, Some("Menu")),
        KeyBinding::new("up", RowUp, Some("List")),
        KeyBinding::new("down", RowDown, Some("List")),
        KeyBinding::new("enter", Open, Some("List")),
    ];
    let options = Options {
        title: "ossteam-ui gallery".into(),
        app_id: Some("ossteam-ui-gallery".into()),
        renderer,
        ..Options::default()
    };
    let run = app::run(options, bindings, |sender: Sender<Msg>| {
        sender.send(Msg::Started);
        Gallery {
            started,
            exit_after,
            tab: Tab::List,
            menu: false,
            search: TextField::new("Search games", 14.0),
            search_focus: FocusHandle::new(),
            list_focus: FocusHandle::new(),
            menu_focus: FocusHandle::new(),
            list: ScrollHandle::new(),
            page: ScrollHandle::new(),
            selected: 0,
            opened: None,
            notice: None,
            notice_timer: None,
            switches: vec![false; 24],
            // Device pixels: drawn 1:1, so a 2x window would want 48.
            icons: (0..4).map(|h| icon(24, h)).collect(),
        }
    });
    match run {
        Ok(s) => eprintln!(
            "renderer {} ({}), appearance from {}",
            s.renderer, s.renderer_why, s.appearance_source
        ),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
