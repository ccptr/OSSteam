// SPDX-License-Identifier: GPL-2.0-or-later
//! The GUI's controls and the pieces pages share. Every control is a tab
//! stop with a focus ring while focus came from the keyboard, and Enter
//! or Space clicks the focused one (the toolkit does that), so all of it
//! is reachable from the keyboard (GUI-4).
//!
//! A control's focus handle is named after its id, so a control that
//! replaces another with the same id (Pause and Resume, Play and Stop)
//! keeps the focus, and so does one disabled while its own action runs.

use ossteam_ui::element::{Element, FocusHandle, ScrollHandle, canvas, panel, text, text_input};
use ossteam_ui::scene::{Color, Quad, Rect};
use ossteam_ui::style::{Align, CursorIcon, Edges, Justify, Length, Styled};
use ossteam_ui::text_field::{FieldStyle, TextField};
use ossteam_ui::window::{Cx, ViewCx};

use crate::gui::Gui;
use crate::theme::Palette;

pub type El = Element<Gui>;
type Handler = Box<dyn Fn(&mut Gui, &mut Cx<Gui>)>;

/// Text sizes, logical pixels.
pub const TEXT: f32 = 14.0;
pub const SMALL: f32 = 12.0;
pub const TITLE: f32 = 20.0;
pub const SEMIBOLD: u16 = 600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Primary,
    Secondary,
    Danger,
    /// No background until hovered; for tabs and menu items.
    Ghost,
}

/// A push button.
pub struct Button {
    p: Palette,
    id: String,
    label: String,
    kind: Kind,
    disabled: bool,
    selected: bool,
    full_width: bool,
    focus: Option<FocusHandle>,
    on_click: Option<Handler>,
}

pub fn button(p: Palette, id: impl Into<String>, label: impl Into<String>) -> Button {
    Button {
        p,
        id: id.into(),
        label: label.into(),
        kind: Kind::Primary,
        disabled: false,
        selected: false,
        full_width: false,
        focus: None,
        on_click: None,
    }
}

impl Button {
    pub fn secondary(mut self) -> Self {
        self.kind = Kind::Secondary;
        self
    }

    pub fn danger(mut self) -> Self {
        self.kind = Kind::Danger;
        self
    }

    pub fn ghost(mut self) -> Self {
        self.kind = Kind::Ghost;
        self
    }

    /// Drawn greyed out, skipped by Tab, and not clickable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Marks the current tab or chosen item.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Takes focus through `focus` instead of the handle named after its
    /// id.
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl From<Button> for El {
    fn from(b: Button) -> El {
        let p = b.p;
        let (bg, fg, border) = match b.kind {
            Kind::Primary => (p.accent, p.on_accent, p.accent),
            Kind::Secondary => (p.surface_raised, p.text, p.border),
            Kind::Danger => (p.surface_raised, p.error, p.border),
            Kind::Ghost if b.selected => (p.selection, p.text, p.selection),
            Kind::Ghost => (Color::TRANSPARENT, p.text, Color::TRANSPARENT),
        };
        let hover = match b.kind {
            Kind::Primary => p.accent.with_alpha(0.85),
            _ => p.border,
        };
        let enabled = !b.disabled;
        let focus = b.focus.unwrap_or_else(|| FocusHandle::named(b.id.as_str()));
        let mut e = panel()
            .id(b.id.as_str())
            .focus(focus)
            .tab_stop(enabled)
            .row()
            .center()
            .pad_xy(12.0, 4.0)
            .radius(6.0)
            .border(2.0, border)
            .background(bg)
            .text_color(fg)
            .font_size(TEXT)
            .nowrap()
            .focus_style(move |s| s.border_color(p.focus_ring))
            .when(b.full_width, |e| e.fill_width().justify(Justify::Start))
            .when(!b.full_width, |e| e.align_self(Align::Start))
            .when(b.disabled, |e| e.opacity(0.5))
            .child(text(b.label));
        if enabled && let Some(f) = b.on_click {
            e = e
                .cursor(CursorIcon::Pointer)
                .hover_style(move |s| s.background(hover))
                .on_click(f);
        }
        e
    }
}

/// An on/off switch with its label. It shows the daemon's value, so it
/// changes when that does.
pub struct Switch {
    p: Palette,
    id: String,
    label: String,
    on: bool,
    on_toggle: Option<Handler>,
}

pub fn switch(p: Palette, id: impl Into<String>, label: impl Into<String>, on: bool) -> Switch {
    Switch {
        p,
        id: id.into(),
        label: label.into(),
        on,
        on_toggle: None,
    }
}

impl Switch {
    pub fn on_toggle(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

impl From<Switch> for El {
    fn from(s: Switch) -> El {
        let p = s.p;
        let track = if s.on { p.accent } else { p.border };
        let knob = panel().size(14.0, 14.0).radius(7.0).background(p.surface);
        let mut e = panel()
            .id(s.id.as_str())
            .focus(FocusHandle::named(s.id.as_str()))
            .tab_stop(true)
            .row()
            .align(Align::Center)
            .align_self(Align::Start)
            .gap(8.0)
            .pad(2.0)
            .radius(6.0)
            .border(2.0, Color::TRANSPARENT)
            .focus_style(move |st| st.border_color(p.focus_ring))
            .cursor(CursorIcon::Pointer)
            .child(
                panel()
                    .row()
                    .size(32.0, 18.0)
                    .pad(2.0)
                    .radius(9.0)
                    .background(track)
                    .justify(if s.on { Justify::End } else { Justify::Start })
                    .child(knob),
            )
            .child(text(s.label).font_size(TEXT))
            .child(
                text(if s.on { "on" } else { "off" })
                    .font_size(SMALL)
                    .text_color(p.muted),
            );
        if let Some(f) = s.on_toggle {
            e = e.on_click(f);
        }
        e
    }
}

/// One option of a list where one is chosen: a marker, a label and a
/// detail line.
pub struct Choice {
    p: Palette,
    id: String,
    label: String,
    detail: Option<String>,
    chosen: bool,
    disabled: bool,
    on_choose: Option<Handler>,
}

pub fn choice(p: Palette, id: impl Into<String>, label: impl Into<String>, chosen: bool) -> Choice {
    Choice {
        p,
        id: id.into(),
        label: label.into(),
        detail: None,
        chosen,
        disabled: false,
        on_choose: None,
    }
}

impl Choice {
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_choose(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.on_choose = Some(Box::new(f));
        self
    }
}

impl From<Choice> for El {
    fn from(c: Choice) -> El {
        let p = c.p;
        let enabled = !c.disabled;
        let marker = panel()
            .size(12.0, 12.0)
            .no_shrink()
            .margin(Edges {
                top: 3.0,
                ..Edges::default()
            })
            .radius(6.0)
            .border(2.0, if c.chosen { p.accent } else { p.muted })
            .when(c.chosen, |m| m.background(p.accent));
        let mut words = panel().grow().child(text(c.label).font_size(TEXT));
        if let Some(d) = c.detail {
            words = words.child(text(d).font_size(SMALL).text_color(p.muted));
        }
        let mut e = panel()
            .id(c.id.as_str())
            .focus(FocusHandle::named(c.id.as_str()))
            .tab_stop(enabled)
            .row()
            .align(Align::Start)
            .gap(8.0)
            .pad_xy(8.0, 4.0)
            .radius(6.0)
            .border(2.0, Color::TRANSPARENT)
            .focus_style(move |s| s.border_color(p.focus_ring))
            .when(c.chosen, |e| e.background(p.selection))
            .when(c.disabled, |e| e.opacity(0.5))
            .child(marker)
            .child(words);
        if enabled && let Some(f) = c.on_choose {
            let hover = p.surface_raised;
            e = e
                .cursor(CursorIcon::Pointer)
                .when(!c.chosen, |e| e.hover_style(move |s| s.background(hover)))
                .on_click(f);
        }
        e
    }
}

/// A bar filled to `fraction` (0 to 1); `None` draws an empty track, for
/// progress not known yet.
pub fn progress_bar(p: Palette, fraction: Option<f32>) -> El {
    let f = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    panel()
        .row()
        .fill_width()
        .height(6.0)
        .radius(3.0)
        .background(p.border)
        .child(
            panel()
                .width(Length::Fraction(f))
                .fill_height()
                .radius(3.0)
                .background(p.accent),
        )
}

/// A single-line text field. It shows its focus ring whenever it has
/// focus (GUI-4), so it takes the view's context. Sizing applies to its
/// box; [`Field::on_submit`] and [`Field::on_change`] to its text.
pub struct Field {
    outer: El,
    input: El,
}

pub fn text_field(
    p: Palette,
    cx: &ViewCx,
    focus: FocusHandle,
    field: impl Fn(&mut Gui) -> &mut TextField + 'static,
) -> Field {
    let style = FieldStyle {
        text: p.text,
        placeholder: p.muted,
        selection: p.accent.with_alpha(0.35),
        caret: p.text,
    };
    let focused = cx.is_focused(focus);
    Field {
        outer: panel()
            .height(32.0)
            .pad_xy(8.0, 0.0)
            .justify(Justify::Center)
            .radius(6.0)
            .border(2.0, if focused { p.focus_ring } else { p.border })
            .background(p.surface),
        input: text_input(focus, style, field).fill_width().font_size(TEXT),
    }
}

impl Field {
    /// Enter in the field.
    pub fn on_submit(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.input = self.input.on_submit(f);
        self
    }

    /// The text changed.
    pub fn on_change(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.input = self.input.on_change(f);
        self
    }

    /// Focus left the field.
    pub fn on_focus_out(mut self, f: impl Fn(&mut Gui, &mut Cx<Gui>) + 'static) -> Self {
        self.input = self.input.on_focus_out(f);
        self
    }
}

impl Styled for Field {
    fn style_mut(&mut self) -> &mut ossteam_ui::style::Style {
        self.outer.style_mut()
    }
}

impl From<Field> for El {
    fn from(f: Field) -> El {
        f.outer.child(f.input)
    }
}

/// The modules of `data` as rows of dark (true) and light cells, with the
/// four-module quiet zone the standard asks for.
pub fn qr_matrix(data: &str) -> Option<Vec<Vec<bool>>> {
    use qrcode::{Color, EcLevel, QrCode};
    const QUIET: usize = 4;
    let code = QrCode::with_error_correction_level(data, EcLevel::M).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let full = width + 2 * QUIET;
    let mut rows = vec![vec![false; full]; full];
    for (i, c) in colors.iter().enumerate() {
        rows[QUIET + i / width][QUIET + i % width] = *c == Color::Dark;
    }
    Some(rows)
}

/// The QR code of a login URL in a square of `side` logical pixels, dark
/// on white whatever the appearance, as scanners expect (`gui.md` §4.1).
/// The matrix is computed here; nothing is fetched.
pub fn qr_code(p: Palette, data: &str, side: f32) -> El {
    let Some(rows) = qr_matrix(data) else {
        return error_text(p, "this login URL does not fit in a QR code");
    };
    panel()
        .size(side, side)
        .background(Color::rgb(0xffffff))
        .child(
            canvas(move |b: Rect, scene, _| {
                // Whole device pixels per module, centred: a scanner wants
                // sharp edges.
                let n = rows.len() as f32;
                let cell = (b.w / n).floor().max(1.0);
                let x0 = b.x + ((b.w - cell * n) / 2.0).floor();
                let y0 = b.y + ((b.h - cell * n) / 2.0).floor();
                for (y, row) in rows.iter().enumerate() {
                    for (x, &dark) in row.iter().enumerate() {
                        if dark {
                            let r =
                                Rect::new(x0 + cell * x as f32, y0 + cell * y as f32, cell, cell);
                            scene.quad(Quad::filled(r, Color::rgb(0)));
                        }
                    }
                }
            })
            .fill(),
        )
}

/// A page section: a heading over its content.
pub fn section(p: Palette, title: impl Into<String>, content: impl Into<El>) -> El {
    panel()
        .gap(8.0)
        .child(
            text(title)
                .font_size(13.0)
                .font_weight(SEMIBOLD)
                .text_color(p.muted),
        )
        .child(content)
}

/// A bordered box for a group of controls.
pub fn group(p: Palette) -> El {
    panel()
        .gap(12.0)
        .pad(16.0)
        .radius(8.0)
        .border(1.0, p.border)
        .background(p.surface)
}

/// A label and its value on one line.
pub fn row(p: Palette, label: impl Into<String>, value: impl Into<String>) -> El {
    panel()
        .row()
        .gap(12.0)
        .font_size(TEXT)
        .child(text(label).width(128.0).no_shrink().text_color(p.muted))
        .child(text(value).grow())
}

pub fn muted(p: Palette, s: impl Into<String>) -> El {
    text(s).font_size(TEXT).text_color(p.muted)
}

/// An error; the page scrolls it into view when it appears near focus
/// (GUI-4).
pub fn error_text(p: Palette, s: impl Into<String>) -> El {
    text(s)
        .font_size(TEXT)
        .text_color(p.error)
        .reveal_when_shown()
}

/// A page title.
pub fn title(s: impl Into<String>) -> El {
    text(s).font_size(TITLE)
}

/// Page Up, Page Down, Home and End on a page that scrolls (GUI-4). Tab
/// shows each control, but what comes after the last one, such as the App
/// page's last run, is seen only by scrolling. A text field and the
/// library grid keep these keys: the field takes editing keys first, and
/// the grid's context is deeper.
pub mod page_keys {
    use ossteam_ui::keys::KeyBinding;

    pub const CONTEXT: &str = "ScrollingPage";
    /// What a step keeps in sight of the view it leaves.
    const OVERLAP: f32 = 48.0;

    #[derive(Clone, Debug)]
    pub struct PageUp;
    #[derive(Clone, Debug)]
    pub struct PageDown;
    #[derive(Clone, Debug)]
    pub struct Top;
    #[derive(Clone, Debug)]
    pub struct Bottom;

    pub fn bindings() -> Vec<KeyBinding> {
        let ctx = Some(CONTEXT);
        vec![
            KeyBinding::new("page_up", PageUp, ctx),
            KeyBinding::new("page_down", PageDown, ctx),
            KeyBinding::new("home", Top, ctx),
            KeyBinding::new("end", Bottom, ctx),
        ]
    }

    /// The offset a page up leaves a view of `height` at `y` (0 at the
    /// top) with.
    pub fn page_up(y: f32, height: f32) -> f32 {
        (y - step(height)).max(0.0)
    }

    pub fn page_down(y: f32, max: f32, height: f32) -> f32 {
        (y + step(height)).min(max)
    }

    fn step(height: f32) -> f32 {
        (height - OVERLAP).max(OVERLAP)
    }
}

/// A page that scrolls, by wheel and by Page Up, Page Down, Home and End.
pub fn scrolling_page(scroll: &ScrollHandle) -> El {
    use page_keys::{Bottom, PageDown, PageUp, Top};
    let s = scroll.clone();
    let (up, down, top, bottom) = (s.clone(), s.clone(), s.clone(), s);
    panel()
        .grow()
        .min_height(0.0)
        .scroll_y(scroll)
        .key_context(page_keys::CONTEXT)
        .on_action(move |_: &mut Gui, _: &PageUp, _| {
            up.set_offset(page_keys::page_up(up.offset(), up.viewport().h));
        })
        .on_action(move |_: &mut Gui, _: &PageDown, _| {
            let to = page_keys::page_down(down.offset(), down.max_offset(), down.viewport().h);
            down.set_offset(to);
        })
        .on_action(move |_: &mut Gui, _: &Top, _| top.set_offset(0.0))
        .on_action(move |_: &mut Gui, _: &Bottom, _| bottom.set_offset(bottom.max_offset()))
}

/// An id for a control about `n` (a job, an app): `name-n`.
pub fn id(name: &str, n: impl std::fmt::Display) -> String {
    format!("{name}-{n}")
}

#[cfg(test)]
mod tests {
    use super::page_keys::{page_down, page_up};
    use super::qr_matrix;

    #[test]
    fn a_page_keeps_some_of_the_view_it_leaves() {
        assert_eq!(page_down(0., 1000., 400.), 352.);
        assert_eq!(page_up(500., 400.), 148.);
    }

    #[test]
    fn a_page_stops_at_either_end() {
        assert_eq!(page_down(900., 1000., 400.), 1000.);
        assert_eq!(page_down(0., 0., 400.), 0.);
        assert_eq!(page_up(100., 400.), 0.);
    }

    #[test]
    fn a_view_shorter_than_the_overlap_still_moves() {
        assert_eq!(page_down(0., 1000., 20.), 48.);
    }

    #[test]
    fn a_login_url_has_a_square_matrix_with_a_quiet_zone() {
        let m = qr_matrix("https://s.team/q/1/1234567890123456789").unwrap();
        assert!(m.iter().all(|row| row.len() == m.len()));
        assert!(m[0].iter().all(|&c| !c));
        // The top left finder pattern starts after the quiet zone.
        assert!(m[4][4] && m[4][10] && m[10][4]);
        assert!(!m[5][5]);
    }
}
