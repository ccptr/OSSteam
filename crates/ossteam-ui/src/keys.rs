// SPDX-License-Identifier: GPL-2.0-or-later
//! Key bindings (ADR 0004, "Focus and keys"). A [`KeyBinding`] maps a
//! keystroke to an [`Action`], everywhere or only inside elements with a
//! key context. The action then goes from the focused element up to the
//! root, to the first element that handles it.

use std::any::{Any, TypeId};
use std::fmt;

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Something a key binding asks for: any `Clone + 'static` type, usually
/// a unit struct (`#[derive(Clone)] struct Open;`).
pub trait Action: Any {
    fn boxed_clone(&self) -> Box<dyn Action>;
    fn as_any(&self) -> &dyn Any;
    fn name(&self) -> &'static str;
}

impl<T: Any + Clone> Action for T {
    fn boxed_clone(&self) -> Box<dyn Action> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

impl dyn Action {
    pub fn action_type(&self) -> TypeId {
        self.as_any().type_id()
    }
}

/// A key with its modifiers, as bindings write it: `"primary+f"`,
/// `"shift+tab"`, `"page_down"`. `primary` is Command on macOS and Ctrl
/// elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Command on macOS, the Windows or Super key elsewhere.
    pub platform: bool,
}

/// A binding's keystroke could not be read.
#[derive(Debug, PartialEq, Eq)]
pub struct BadKeystroke(pub String);

impl fmt::Display for BadKeystroke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a keystroke: {:?}", self.0)
    }
}

impl std::error::Error for BadKeystroke {}

impl Keystroke {
    /// Reads `"ctrl+shift+a"` and the like. Modifiers are `ctrl`, `alt`,
    /// `shift`, `super` (Command on macOS) and `primary`.
    pub fn parse(source: &str) -> Result<Self, BadKeystroke> {
        let mut k = Keystroke::default();
        let mut parts: Vec<&str> = source.split('+').collect();
        // "ctrl++" binds the plus key.
        if source.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let Some((key, mods)) = parts.split_last() else {
            return Err(BadKeystroke(source.into()));
        };
        for m in mods {
            match *m {
                "ctrl" => k.ctrl = true,
                "alt" => k.alt = true,
                "shift" => k.shift = true,
                "super" => k.platform = true,
                "primary" if cfg!(target_os = "macos") => k.platform = true,
                "primary" => k.ctrl = true,
                _ => return Err(BadKeystroke(source.into())),
            }
        }
        if key.is_empty() {
            return Err(BadKeystroke(source.into()));
        }
        k.key = key.to_lowercase();
        Ok(k)
    }

    /// The keystroke a key press makes, if the key has a name bindings
    /// can use.
    pub fn from_key(key: &Key, mods: ModifiersState) -> Option<Self> {
        let name = match key {
            Key::Named(named) => named_key(*named)?.to_owned(),
            Key::Character(c) => c.to_lowercase(),
            _ => return None,
        };
        Some(Keystroke {
            key: name,
            ctrl: mods.control_key(),
            alt: mods.alt_key(),
            shift: mods.shift_key(),
            platform: mods.super_key(),
        })
    }
}

fn named_key(key: NamedKey) -> Option<&'static str> {
    Some(match key {
        NamedKey::Enter => "enter",
        NamedKey::Tab => "tab",
        NamedKey::Space => "space",
        NamedKey::Escape => "escape",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "page_up",
        NamedKey::PageDown => "page_down",
        NamedKey::Insert => "insert",
        NamedKey::F1 => "f1",
        NamedKey::F2 => "f2",
        NamedKey::F3 => "f3",
        NamedKey::F4 => "f4",
        NamedKey::F5 => "f5",
        NamedKey::F6 => "f6",
        NamedKey::F7 => "f7",
        NamedKey::F8 => "f8",
        NamedKey::F9 => "f9",
        NamedKey::F10 => "f10",
        NamedKey::F11 => "f11",
        NamedKey::F12 => "f12",
        _ => return None,
    })
}

/// A keystroke bound to an action, everywhere or only inside elements
/// with the key context `context`.
pub struct KeyBinding {
    pub keystroke: Keystroke,
    action: Box<dyn Action>,
    pub context: Option<&'static str>,
}

impl KeyBinding {
    /// Panics if `keystroke` cannot be read: bindings are written in the
    /// source, so a bad one is a bug.
    pub fn new(keystroke: &str, action: impl Action, context: Option<&'static str>) -> Self {
        Self {
            keystroke: Keystroke::parse(keystroke).unwrap_or_else(|e| panic!("{e}")),
            action: Box::new(action),
            context,
        }
    }

    pub fn action(&self) -> &dyn Action {
        &*self.action
    }
}

/// Every binding, in the order they were added; a later binding wins
/// over an earlier one at the same depth.
#[derive(Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}

impl Keymap {
    pub fn bind(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        self.bindings.extend(bindings);
    }

    /// The actions `keystroke` asks for, best first. `contexts` lists the
    /// key contexts from the root to the focused element, one entry per
    /// element; a binding whose context is deeper in that list wins, and
    /// one with no context counts as the root's.
    pub fn matches(
        &self,
        keystroke: &Keystroke,
        contexts: &[Vec<&'static str>],
    ) -> Vec<Box<dyn Action>> {
        let mut found: Vec<(usize, usize, &KeyBinding)> = Vec::new();
        for (order, b) in self.bindings.iter().enumerate() {
            if b.keystroke != *keystroke {
                continue;
            }
            let depth = match b.context {
                None => Some(0),
                Some(c) => contexts
                    .iter()
                    .rposition(|cs| cs.contains(&c))
                    .map(|d| d + 1),
            };
            if let Some(depth) = depth {
                found.push((depth, order, b));
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse((f.0, f.1)));
        found
            .into_iter()
            .map(|(_, _, b)| b.action.boxed_clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Up;
    #[derive(Clone)]
    struct Down;
    #[derive(Clone)]
    struct Close;

    #[test]
    fn keystrokes_parse() {
        let k = Keystroke::parse("shift+tab").unwrap();
        assert!(k.shift && !k.ctrl && k.key == "tab");
        let k = Keystroke::parse("primary+f").unwrap();
        assert_eq!(k.key, "f");
        assert_eq!(k.ctrl, !cfg!(target_os = "macos"));
        assert_eq!(k.platform, cfg!(target_os = "macos"));
        assert_eq!(Keystroke::parse("ctrl++").unwrap().key, "+");
        assert_eq!(Keystroke::parse("Page_Down").unwrap().key, "page_down");
        assert!(Keystroke::parse("hyper+a").is_err());
        assert!(Keystroke::parse("").is_err());
    }

    #[test]
    fn a_deeper_context_wins_then_a_later_binding() {
        let mut map = Keymap::default();
        map.bind([
            KeyBinding::new("escape", Close, None),
            KeyBinding::new("escape", Up, Some("Page")),
            KeyBinding::new("escape", Down, Some("Menu")),
            KeyBinding::new("home", Up, Some("Page")),
            KeyBinding::new("home", Down, Some("Page")),
        ]);
        let esc = Keystroke::parse("escape").unwrap();
        let is = |a: &dyn Action, t: TypeId| a.action_type() == t;
        let ctx = vec![vec![], vec!["Page"], vec!["Menu"]];
        let got = map.matches(&esc, &ctx);
        assert!(is(&*got[0], TypeId::of::<Down>()));
        assert!(is(&*got[1], TypeId::of::<Up>()));
        assert!(is(&*got[2], TypeId::of::<Close>()));
        // Outside the menu, the page's binding wins and the menu's is gone.
        let got = map.matches(&esc, &ctx[..2]);
        assert_eq!(got.len(), 2);
        assert!(is(&*got[0], TypeId::of::<Up>()));
        let home = Keystroke::parse("home").unwrap();
        assert!(is(&*map.matches(&home, &ctx)[0], TypeId::of::<Down>()));
    }
}
