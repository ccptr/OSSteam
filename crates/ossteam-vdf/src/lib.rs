// SPDX-License-Identifier: LGPL-2.1-or-later
//! KeyValues ("VDF"), the tree format Steam uses for app info, manifests,
//! library and configuration files.
//!
//! Two encodings exist and both are supported, for reading and writing:
//!
//! - [`text`]: the human-readable form (`.vdf`, `.acf`, `installscript.vdf`,
//!   `toolmanifest.vdf`, ...).
//! - [`binary`]: the typed binary form (PICS buffers, `appinfo.vdf` entries,
//!   `shortcuts.vdf`), including the string-table variant for keys.
//!
//! The format is described in our own words in `docs/spec/keyvalues.md`.
//!
//! An [`Object`] keeps its entries in file order and allows duplicate keys,
//! because real files rely on both. Lookups by key are ASCII
//! case-insensitive, as Steam's own lookups are.

pub mod binary;
pub mod text;

use std::fmt;

/// A KeyValues value.
///
/// Text KeyValues only ever produce [`Value::String`] and [`Value::Object`].
/// The other variants come from the binary encoding.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Int32(i32),
    Float32(f32),
    /// A 32-bit pointer-typed integer. It carries no meaning outside the
    /// process that wrote it and is kept only so files round-trip.
    Pointer(u32),
    /// A string stored as UTF-16 in the binary encoding.
    WideString(String),
    /// An RGBA colour packed into 32 bits.
    Color(u32),
    UInt64(u64),
    Int64(i64),
    Object(Object),
}

impl Value {
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The value as a string slice, for the two string variants.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::WideString(s) => Some(s),
            _ => None,
        }
    }

    /// The value as an integer. Integer variants convert when they fit, and
    /// strings are parsed as decimal (text KeyValues stores numbers as
    /// strings).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int32(n) => Some(i64::from(*n)),
            Value::Pointer(n) | Value::Color(n) => Some(i64::from(*n)),
            Value::Int64(n) => Some(*n),
            Value::UInt64(n) => i64::try_from(*n).ok(),
            Value::String(s) | Value::WideString(s) => s.trim().parse().ok(),
            Value::Float32(_) | Value::Object(_) => None,
        }
    }

    /// The value as an unsigned 64-bit integer, following the same rules as
    /// [`Value::as_i64`].
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt64(n) => Some(*n),
            Value::String(s) | Value::WideString(s) => s.trim().parse().ok(),
            other => other.as_i64().and_then(|n| u64::try_from(n).ok()),
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float32(f) => Some(*f),
            Value::String(s) | Value::WideString(s) => s.trim().parse().ok(),
            Value::Int32(n) => Some(*n as f32),
            _ => None,
        }
    }

    /// The value as a boolean in the KeyValues convention: a non-zero
    /// integer is true.
    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|n| n != 0)
    }

    /// The value rendered as it appears in text KeyValues. Objects have no
    /// scalar rendering and return `None`.
    pub fn to_text_scalar(&self) -> Option<String> {
        Some(match self {
            Value::String(s) | Value::WideString(s) => s.clone(),
            Value::Int32(n) => n.to_string(),
            Value::Float32(f) => format_f32(*f),
            Value::Pointer(n) | Value::Color(n) => n.to_string(),
            Value::UInt64(n) => n.to_string(),
            Value::Int64(n) => n.to_string(),
            Value::Object(_) => return None,
        })
    }
}

fn format_f32(f: f32) -> String {
    // Steam writes floats with six decimals ("%f").
    format!("{f:.6}")
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<Object> for Value {
    fn from(o: Object) -> Self {
        Value::Object(o)
    }
}

/// One key/value pair inside an [`Object`].
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub value: Value,
    /// A text-format conditional such as `[$WIN32]` or `[!$X360]`, stored
    /// without the brackets. ossteam does not evaluate conditionals here;
    /// callers that care read them.
    pub condition: Option<String>,
}

impl Entry {
    pub fn new(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            condition: None,
        }
    }
}

/// An ordered list of entries. Duplicate keys are allowed and kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    pub entries: Vec<Entry>,
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|e| (e.key.as_str(), &e.value))
    }

    /// The first value whose key matches `key`, ignoring ASCII case.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(key))
            .map(|e| &e.value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|e| e.key.eq_ignore_ascii_case(key))
            .map(|e| &mut e.value)
    }

    /// Every value whose key matches `key`, ignoring ASCII case, in order.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.entries
            .iter()
            .filter(move |e| e.key.eq_ignore_ascii_case(key))
            .map(|e| &e.value)
    }

    /// Follows a path of keys through nested objects.
    pub fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let (last, parents) = path.split_last()?;
        let mut obj = self;
        for key in parents {
            obj = obj.get(key)?.as_object()?;
        }
        obj.get(last)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn get_object(&self, key: &str) -> Option<&Object> {
        self.get(key)?.as_object()
    }

    /// Appends an entry, keeping any existing entries with the same key.
    pub fn push(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.entries.push(Entry::new(key, value));
    }

    /// Replaces the first entry with a matching key, or appends one if
    /// there is none. Later duplicates are left alone.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        match self
            .entries
            .iter_mut()
            .find(|e| e.key.eq_ignore_ascii_case(&key))
        {
            Some(e) => e.value = value,
            None => self.entries.push(Entry::new(key, value)),
        }
    }

    /// Removes every entry with a matching key and returns how many there
    /// were.
    pub fn remove(&mut self, key: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !e.key.eq_ignore_ascii_case(key));
        before - self.entries.len()
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Object {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().map(|(k, v)| Entry::new(k, v)).collect(),
        }
    }
}

impl fmt::Display for Object {
    /// Formats the object as text KeyValues.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&text::to_string(self))
    }
}

/// Nesting deeper than this is rejected by both parsers, so hostile input
/// cannot exhaust the stack.
pub const MAX_DEPTH: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_are_case_insensitive_and_ordered() {
        let mut o = Object::new();
        o.push("Name", "first");
        o.push("name", "second");
        assert_eq!(o.get_str("NAME"), Some("first"));
        let all: Vec<_> = o.get_all("name").filter_map(Value::as_str).collect();
        assert_eq!(all, ["first", "second"]);
        o.set("NAME", "replaced");
        assert_eq!(o.get_str("name"), Some("replaced"));
        assert_eq!(o.len(), 2);
        assert_eq!(o.remove("name"), 2);
        assert!(o.is_empty());
    }

    #[test]
    fn scalar_conversions() {
        assert_eq!(Value::from("42").as_i64(), Some(42));
        assert_eq!(Value::from(" 7 ").as_u64(), Some(7));
        assert_eq!(Value::UInt64(u64::MAX).as_i64(), None);
        assert_eq!(Value::UInt64(u64::MAX).as_u64(), Some(u64::MAX));
        assert_eq!(Value::Int32(-1).as_u64(), None);
        assert_eq!(Value::from("1").as_bool(), Some(true));
        assert_eq!(Value::from("0.5").as_f32(), Some(0.5));
        assert_eq!(
            Value::Float32(1.5).to_text_scalar().as_deref(),
            Some("1.500000")
        );
    }

    #[test]
    fn get_path_walks_objects() {
        let inner: Object = [("installdir", "Spacewar")].into_iter().collect();
        let mut root = Object::new();
        root.push("config", inner);
        assert_eq!(
            root.get_path(&["config", "installdir"])
                .and_then(Value::as_str),
            Some("Spacewar")
        );
        assert!(root.get_path(&["config", "missing"]).is_none());
        assert!(root.get_path(&[]).is_none());
    }
}
