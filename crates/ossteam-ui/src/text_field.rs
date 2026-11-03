// SPDX-License-Identifier: GPL-2.0-or-later
//! A single-line text field: its editing model, the key bindings, IME
//! composition and painting (ADR 0004: the text field is ours).
//!
//! The text is edited by byte offsets that always sit on grapheme
//! boundaries. An IME's composition (the preedit) is shown at the cursor
//! but is not part of the text until the IME commits it. The caret does
//! not blink: a blinking caret would wake an idle window (GUI-5).

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use winit::event::Ime;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::scene::{Color, Quad, Rect, Scene};
use crate::text::{ShapedText, TextSystem};

/// Where a cursor movement goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
}

/// An edit the keyboard asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Insert(String),
    /// Moves the cursor; with `extend`, the selection's far end.
    Move {
        motion: Motion,
        extend: bool,
    },
    /// Backspace: the selection, else the grapheme (or word) before the cursor.
    DeleteBack {
        word: bool,
    },
    /// Delete: the selection, else the grapheme (or word) after the cursor.
    DeleteForward {
        word: bool,
    },
    SelectAll,
    Copy,
    Cut,
    Paste,
}

/// The clipboard as the field uses it.
pub trait TextClipboard {
    fn read_text(&mut self) -> Option<String>;
    fn write_text(&mut self, text: &str);
}

impl TextClipboard for Option<String> {
    fn read_text(&mut self) -> Option<String> {
        self.clone()
    }

    fn write_text(&mut self, text: &str) {
        *self = Some(text.to_owned());
    }
}

/// The key bindings: what a key press means to a focused field, if
/// anything. Shortcuts use Command on macOS and Ctrl elsewhere; word
/// movement uses Option on macOS and Ctrl elsewhere.
pub fn command_for(key: &Key, text: Option<&str>, mods: ModifiersState) -> Option<Command> {
    let mac = cfg!(target_os = "macos");
    let primary = if mac {
        mods.super_key()
    } else {
        mods.control_key()
    };
    let word = if mac {
        mods.alt_key()
    } else {
        mods.control_key()
    };
    let extend = mods.shift_key();
    let motion = |motion| Some(Command::Move { motion, extend });
    match key {
        Key::Named(NamedKey::ArrowLeft) if mac && mods.super_key() => motion(Motion::Home),
        Key::Named(NamedKey::ArrowRight) if mac && mods.super_key() => motion(Motion::End),
        Key::Named(NamedKey::ArrowLeft) if word => motion(Motion::WordLeft),
        Key::Named(NamedKey::ArrowRight) if word => motion(Motion::WordRight),
        Key::Named(NamedKey::ArrowLeft) => motion(Motion::Left),
        Key::Named(NamedKey::ArrowRight) => motion(Motion::Right),
        Key::Named(NamedKey::Home) => motion(Motion::Home),
        Key::Named(NamedKey::End) => motion(Motion::End),
        Key::Named(NamedKey::Backspace) => Some(Command::DeleteBack { word }),
        Key::Named(NamedKey::Delete) => Some(Command::DeleteForward { word }),
        // Ctrl+Alt is AltGr on Windows, which types characters.
        Key::Character(c) if primary && !mods.alt_key() => match c.to_lowercase().as_str() {
            "a" => Some(Command::SelectAll),
            "c" => Some(Command::Copy),
            "x" => Some(Command::Cut),
            "v" => Some(Command::Paste),
            _ => None,
        },
        Key::Named(NamedKey::Copy) => Some(Command::Copy),
        Key::Named(NamedKey::Cut) => Some(Command::Cut),
        Key::Named(NamedKey::Paste) => Some(Command::Paste),
        _ => {
            if primary && !mods.alt_key() {
                return None;
            }
            let text: String = text?.chars().filter(|c| !c.is_control()).collect();
            (!text.is_empty()).then_some(Command::Insert(text))
        }
    }
}

/// An IME composition in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The IME's cursor or highlighted clause, as byte offsets into
    /// `text`; `None` hides the caret.
    pub cursor: Option<(usize, usize)>,
}

/// Colours a field paints with.
#[derive(Clone, Copy, Debug)]
pub struct FieldStyle {
    pub text: Color,
    pub placeholder: Color,
    pub selection: Color,
    pub caret: Color,
}

/// A single-line text field.
pub struct TextField {
    text: String,
    /// Byte offsets: the selection is between `anchor` and `cursor`.
    cursor: usize,
    anchor: usize,
    preedit: Option<Preedit>,
    placeholder: String,
    /// The font size, in logical pixels.
    size: f32,
    shaped: Option<ShapedText>,
    placeholder_shaped: Option<ShapedText>,
    /// How far the text is scrolled left to keep the caret in view, in
    /// logical pixels.
    scroll_x: f32,
    /// Counts changes to what the field shows.
    revision: u64,
}

impl TextField {
    pub fn new(placeholder: &str, size: f32) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: 0,
            preedit: None,
            placeholder: placeholder.to_owned(),
            size,
            shaped: None,
            placeholder_shaped: None,
            scroll_x: 0.0,
            revision: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Whether an IME is composing: keys then belong to the IME.
    pub fn composing(&self) -> bool {
        self.preedit.is_some()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = single_line(text);
        self.cursor = self.text.len();
        self.anchor = self.cursor;
        self.preedit = None;
        self.changed();
    }

    /// Forgets the shaped text and the placeholder's shape, after a
    /// change of scale or font.
    pub fn reshape(&mut self) {
        self.shaped = None;
        self.placeholder_shaped = None;
    }

    /// Changes whenever the text or the preedit does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self) {
        self.shaped = None;
        self.revision += 1;
    }

    /// Replaces the selection with `text`.
    pub fn insert(&mut self, text: &str) {
        let text = single_line(text);
        let sel = self.selection();
        self.text.replace_range(sel.clone(), &text);
        self.cursor = sel.start + text.len();
        self.anchor = self.cursor;
        self.changed();
    }

    fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.is_empty() {
            return false;
        }
        self.text.replace_range(sel.clone(), "");
        self.cursor = sel.start;
        self.anchor = sel.start;
        self.changed();
        true
    }

    fn target(&self, motion: Motion) -> usize {
        let t = &self.text;
        match motion {
            Motion::Left => prev_grapheme(t, self.cursor),
            Motion::Right => next_grapheme(t, self.cursor),
            Motion::WordLeft => word_left(t, self.cursor),
            Motion::WordRight => word_right(t, self.cursor),
            Motion::Home => 0,
            Motion::End => t.len(),
        }
    }

    pub fn move_cursor(&mut self, motion: Motion, extend: bool) {
        let sel = self.selection();
        self.cursor = match motion {
            // Left and Right without Shift collapse a selection to its edge.
            Motion::Left if !extend && !sel.is_empty() => sel.start,
            Motion::Right if !extend && !sel.is_empty() => sel.end,
            m => self.target(m),
        };
        if !extend {
            self.anchor = self.cursor;
        }
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    /// Applies a command. Returns whether the text changed.
    pub fn apply(&mut self, command: Command, clipboard: &mut dyn TextClipboard) -> bool {
        let before = self.revision;
        match command {
            Command::Insert(text) => self.insert(&text),
            Command::Move { motion, extend } => self.move_cursor(motion, extend),
            Command::DeleteBack { word } => {
                if !self.delete_selection() {
                    let to = if word {
                        word_left(&self.text, self.cursor)
                    } else {
                        prev_grapheme(&self.text, self.cursor)
                    };
                    self.anchor = to;
                    self.delete_selection();
                }
            }
            Command::DeleteForward { word } => {
                if !self.delete_selection() {
                    let to = if word {
                        word_right(&self.text, self.cursor)
                    } else {
                        next_grapheme(&self.text, self.cursor)
                    };
                    self.anchor = to;
                    self.delete_selection();
                }
            }
            Command::SelectAll => self.select_all(),
            Command::Copy => {
                if !self.selection().is_empty() {
                    clipboard.write_text(self.selected_text());
                }
            }
            Command::Cut => {
                if !self.selection().is_empty() {
                    clipboard.write_text(self.selected_text());
                    self.delete_selection();
                }
            }
            Command::Paste => {
                if let Some(text) = clipboard.read_text() {
                    self.insert(&text);
                }
            }
        }
        self.revision != before
    }

    /// Applies an IME event. Returns whether what the field shows changed.
    pub fn ime(&mut self, event: &Ime) -> bool {
        match event {
            Ime::Enabled => false,
            Ime::Preedit(text, cursor) => {
                let next = (!text.is_empty()).then(|| Preedit {
                    text: text.clone(),
                    cursor: *cursor,
                });
                if next == self.preedit {
                    return false;
                }
                // A composition replaces the selection, as it would on commit.
                if next.is_some() {
                    self.delete_selection();
                }
                self.preedit = next;
                self.changed();
                true
            }
            Ime::Commit(text) => {
                self.preedit = None;
                self.insert(text);
                true
            }
            Ime::Disabled => {
                let had = self.preedit.take().is_some();
                if had {
                    self.changed();
                }
                had
            }
        }
    }

    /// The text as shown, with the preedit at the cursor, and where the
    /// preedit sits in it.
    pub fn display(&self) -> (String, Option<Range<usize>>) {
        match &self.preedit {
            None => (self.text.clone(), None),
            Some(p) => {
                let mut s = String::with_capacity(self.text.len() + p.text.len());
                s.push_str(&self.text[..self.cursor]);
                s.push_str(&p.text);
                s.push_str(&self.text[self.cursor..]);
                (s, Some(self.cursor..self.cursor + p.text.len()))
            }
        }
    }

    /// Where the caret is in the displayed text, if it is shown.
    fn caret_in_display(&self) -> Option<usize> {
        match &self.preedit {
            None => Some(self.cursor),
            Some(p) => p.cursor.map(|(start, _)| self.cursor + start),
        }
    }

    fn shaped(&mut self, text: &mut TextSystem) -> &ShapedText {
        if self.shaped.is_none() {
            self.shaped = Some(text.shape(&self.display().0, self.size));
        }
        self.shaped.as_ref().expect("just shaped")
    }

    /// Places the cursor at `x` logical pixels from the text area's left
    /// edge (a click, or with `extend` a Shift-click or a drag).
    pub fn place_cursor(&mut self, text: &mut TextSystem, x: f32, extend: bool) {
        if self.preedit.is_some() {
            return;
        }
        let scroll = self.scroll_x;
        let at = self.shaped(text).offset_at(x + scroll);
        self.cursor = snap_to_grapheme(&self.text, at);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    /// Paints the field's text into `area` (device pixels): selection,
    /// text or placeholder, the preedit's underline and the caret.
    /// Returns the caret's rectangle, in device pixels, for the IME's
    /// candidate window.
    pub fn paint(
        &mut self,
        text: &mut TextSystem,
        scene: &mut Scene,
        area: Rect,
        scale: f32,
        style: &FieldStyle,
        focused: bool,
    ) -> Option<Rect> {
        let (shown, preedit) = self.display();
        let size = self.size;
        let caret = self.caret_in_display();
        let selection = self.selection();
        self.shaped(text);
        let shaped = self.shaped.take().expect("just shaped");

        // Scroll to keep the caret in view.
        let inner = area.w / scale;
        let caret_x = caret.map(|c| shaped.x_of(c));
        if let Some(x) = caret_x {
            if x - self.scroll_x > inner - 1.0 {
                self.scroll_x = x - inner + 1.0;
            }
            if x < self.scroll_x {
                self.scroll_x = x;
            }
        }
        self.scroll_x = self
            .scroll_x
            .min((shaped.width + 1.0 - inner).max(0.0))
            .max(0.0);

        let x0 = area.x - (self.scroll_x * scale).round();
        let y0 = (area.y + (area.h - shaped.height * scale) / 2.0).round();
        let px = |x: f32| x0 + (x * scale).round();
        scene.push_clip(area);
        if focused && !selection.is_empty() && preedit.is_none() {
            let (a, b) = (
                px(shaped.x_of(selection.start)),
                px(shaped.x_of(selection.end)),
            );
            scene.quad(Quad::filled(
                Rect::new(a.min(b), y0, (a - b).abs(), shaped.height * scale),
                style.selection,
            ));
        }
        if shown.is_empty() {
            let placeholder = &self.placeholder;
            let p = self
                .placeholder_shaped
                .get_or_insert_with(|| text.shape(placeholder, size));
            text.paint(scene, p, x0, y0, scale, style.placeholder);
        } else {
            text.paint(scene, &shaped, x0, y0, scale, style.text);
        }
        let line = scale.round().max(1.0);
        let underline_y = y0 + ((shaped.baseline + 2.0) * scale).round();
        if let (Some(range), Some(p)) = (&preedit, &self.preedit) {
            let (a, b) = (px(shaped.x_of(range.start)), px(shaped.x_of(range.end)));
            scene.quad(Quad::filled(
                Rect::new(a.min(b), underline_y, (a - b).abs(), line),
                style.text,
            ));
            // The clause the IME is converting, underlined twice as thick.
            if let Some((s, e)) = p.cursor.filter(|(s, e)| s != e) {
                let (a, b) = (
                    px(shaped.x_of(range.start + s)),
                    px(shaped.x_of(range.start + e)),
                );
                scene.quad(Quad::filled(
                    Rect::new(a.min(b), underline_y, (a - b).abs(), 2.0 * line),
                    style.text,
                ));
            }
        }
        let caret_rect = caret_x.map(|x| {
            Rect::new(
                px(x).min(area.right() - line),
                y0,
                line.max((1.5 * scale).round()),
                shaped.height * scale,
            )
        });
        if focused && let Some(r) = caret_rect {
            scene.quad(Quad::filled(r, style.caret));
        }
        scene.pop_clip();
        self.shaped = Some(shaped);
        caret_rect
    }
}

/// The text without line breaks: a single-line field turns them into spaces.
fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

fn prev_grapheme(text: &str, at: usize) -> usize {
    text[..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_grapheme(text: &str, at: usize) -> usize {
    text[at..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |g| at + g.len())
}

fn snap_to_grapheme(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= at)
        .last()
        .map_or(0, |i| if at == text.len() { at } else { i })
}

fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

/// The start of the word before `at`.
fn word_left(text: &str, at: usize) -> usize {
    text.split_word_bound_indices()
        .take_while(|(i, _)| *i < at)
        .filter(|(_, w)| is_word(w))
        .last()
        .map_or(0, |(i, _)| i)
}

/// The end of the word after `at`.
fn word_right(text: &str, at: usize) -> usize {
    text.split_word_bound_indices()
        .map(|(i, w)| (i + w.len(), w))
        .find(|(end, w)| *end > at && is_word(w))
        .map_or(text.len(), |(end, _)| end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> TextField {
        let mut f = TextField::new("", 14.0);
        f.set_text(text);
        f
    }

    fn run(f: &mut TextField, commands: &[Command], clip: &mut Option<String>) {
        for c in commands {
            f.apply(c.clone(), clip);
        }
    }

    fn mv(motion: Motion, extend: bool) -> Command {
        Command::Move { motion, extend }
    }

    #[test]
    fn graphemes_move_and_delete_whole() {
        // "e" + combining acute, a flag (two code points), "x".
        let mut f = field("e\u{301}🇫🇷x");
        let mut clip = None;
        f.move_cursor(Motion::Home, false);
        f.move_cursor(Motion::Right, false);
        assert_eq!(f.cursor(), 3);
        f.move_cursor(Motion::Right, false);
        assert_eq!(f.cursor(), 11);
        run(&mut f, &[Command::DeleteBack { word: false }], &mut clip);
        assert_eq!(f.text(), "e\u{301}x");
        run(&mut f, &[Command::DeleteBack { word: false }], &mut clip);
        assert_eq!(f.text(), "x");
        run(&mut f, &[Command::DeleteBack { word: false }], &mut clip);
        assert_eq!(f.text(), "x");
    }

    #[test]
    fn words() {
        let mut f = field("hollow  kingdom: remastered");
        let mut clip = None;
        f.move_cursor(Motion::WordLeft, false);
        assert_eq!(f.cursor(), 17);
        f.move_cursor(Motion::WordLeft, false);
        assert_eq!(f.cursor(), 8);
        f.move_cursor(Motion::WordRight, false);
        assert_eq!(f.cursor(), 15);
        f.move_cursor(Motion::Home, false);
        f.move_cursor(Motion::WordRight, false);
        assert_eq!(f.cursor(), 6);
        run(&mut f, &[Command::DeleteForward { word: true }], &mut clip);
        assert_eq!(f.text(), "hollow: remastered");
        f.move_cursor(Motion::End, false);
        run(&mut f, &[Command::DeleteBack { word: true }], &mut clip);
        assert_eq!(f.text(), "hollow: ");
    }

    #[test]
    fn selection_clipboard() {
        let mut f = field("hollow kingdom");
        let mut clip = None;
        run(
            &mut f,
            &[
                mv(Motion::WordLeft, true),
                Command::Cut,
                mv(Motion::Home, false),
                Command::Paste,
            ],
            &mut clip,
        );
        assert_eq!(clip.as_deref(), Some("kingdom"));
        assert_eq!(f.text(), "kingdomhollow ");
        assert_eq!(f.cursor(), 7);
        run(&mut f, &[Command::SelectAll, Command::Copy], &mut clip);
        assert_eq!(clip.as_deref(), Some("kingdomhollow "));
        // Left collapses the selection to its start, typing replaces it.
        run(&mut f, &[mv(Motion::Left, false)], &mut clip);
        assert_eq!((f.cursor(), f.selection()), (0, 0..0));
        run(
            &mut f,
            &[Command::SelectAll, Command::Insert("a\nb".into())],
            &mut clip,
        );
        assert_eq!(f.text(), "a b");
    }

    #[test]
    fn preedit_shows_at_cursor_and_commits() {
        let mut f = field("ab");
        f.move_cursor(Motion::Left, false);
        f.move_cursor(Motion::Left, true);
        assert!(f.ime(&Ime::Preedit("にほ".into(), Some((3, 6)))));
        // The composition replaced the selection.
        assert_eq!(f.text(), "b");
        assert_eq!(f.display(), ("にほb".into(), Some(0..6)));
        assert_eq!(f.caret_in_display(), Some(3));
        assert!(f.composing());
        f.ime(&Ime::Preedit(String::new(), None));
        assert!(!f.composing());
        f.ime(&Ime::Commit("日本".into()));
        assert_eq!(f.text(), "日本b");
        assert_eq!(f.cursor(), 6);
        f.ime(&Ime::Preedit("x".into(), None));
        assert_eq!(f.caret_in_display(), None);
        assert!(f.ime(&Ime::Disabled));
        assert_eq!(f.display().0, "日本b");
    }

    #[test]
    fn key_bindings() {
        let ctrl = ModifiersState::CONTROL;
        let shift = ModifiersState::SHIFT;
        let ch = |c: &str| Key::Character(c.into());
        let primary = if cfg!(target_os = "macos") {
            ModifiersState::SUPER
        } else {
            ctrl
        };
        assert_eq!(
            command_for(&ch("v"), Some("v"), primary),
            Some(Command::Paste)
        );
        assert_eq!(
            command_for(&ch("A"), Some("A"), primary | shift),
            Some(Command::SelectAll)
        );
        assert_eq!(
            command_for(&ch("q"), Some("q"), primary),
            None,
            "unbound shortcuts type nothing"
        );
        assert_eq!(
            command_for(&ch("é"), Some("é"), ctrl | ModifiersState::ALT),
            Some(Command::Insert("é".into())),
            "AltGr types"
        );
        assert_eq!(
            command_for(&Key::Named(NamedKey::ArrowLeft), None, shift),
            Some(mv(Motion::Left, true))
        );
        assert_eq!(
            command_for(
                &Key::Named(NamedKey::Enter),
                Some("\r"),
                ModifiersState::empty()
            ),
            None
        );
    }

    #[test]
    fn snapping() {
        let t = "e\u{301}x";
        assert_eq!(snap_to_grapheme(t, 1), 0);
        assert_eq!(snap_to_grapheme(t, 3), 3);
        assert_eq!(snap_to_grapheme(t, 4), 4);
    }
}
