// SPDX-License-Identifier: GPL-2.0-or-later
//! Binary KeyValues.
//!
//! An object is a run of entries ended by an end marker. Each entry is a
//! one-byte type tag, a key, and a value whose layout the tag selects. All
//! integers are little-endian.
//!
//! | Tag  | Value                                         |
//! |------|-----------------------------------------------|
//! | 0x00 | nested object (entries up to an end marker)    |
//! | 0x01 | NUL-terminated UTF-8 string                   |
//! | 0x02 | `i32`                                         |
//! | 0x03 | `f32`                                         |
//! | 0x04 | `u32` pointer                                 |
//! | 0x05 | NUL-terminated (`u16` 0) UTF-16LE string       |
//! | 0x06 | `u32` colour                                  |
//! | 0x07 | `u64`                                         |
//! | 0x08 | end of object (no key, no value)              |
//! | 0x0A | `i64`                                         |
//! | 0x0B | alternative end of object                     |
//!
//! Keys are NUL-terminated UTF-8 strings, or, in the string-table variant
//! used by newer app info caches, a `u32` index into a table that is stored
//! elsewhere in the containing file.

use crate::{Entry, MAX_DEPTH, Object, Value};

const T_OBJECT: u8 = 0x00;
const T_STRING: u8 = 0x01;
const T_INT32: u8 = 0x02;
const T_FLOAT32: u8 = 0x03;
const T_POINTER: u8 = 0x04;
const T_WSTRING: u8 = 0x05;
const T_COLOR: u8 = 0x06;
const T_UINT64: u8 = 0x07;
const T_END: u8 = 0x08;
const T_INT64: u8 = 0x0a;
const T_END_ALT: u8 = 0x0b;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("at byte {offset}: {kind}")]
pub struct Error {
    pub offset: usize,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ErrorKind {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unknown type tag {0:#04x}")]
    UnknownType(u8),
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    #[error("wide string is not valid UTF-16")]
    InvalidUtf16,
    #[error("key index {0} is outside the string table")]
    BadKeyIndex(u32),
    #[error("nesting deeper than {MAX_DEPTH} levels")]
    TooDeep,
    #[error("missing end marker")]
    MissingEnd,
}

/// How keys are stored.
#[derive(Debug, Clone, Copy, Default)]
pub enum Keys<'t> {
    /// Each key is an inline NUL-terminated string.
    #[default]
    Inline,
    /// Each key is a `u32` index into this table.
    Table(&'t [String]),
}

/// Parses an object from the start of `input` and returns it with the
/// number of bytes consumed, which includes the object's end marker.
///
/// The top-level object may also end at the end of input without a marker,
/// because some writers omit it.
pub fn parse(input: &[u8], keys: Keys<'_>) -> Result<(Object, usize), Error> {
    let mut r = Reader {
        buf: input,
        pos: 0,
        keys,
    };
    let obj = r.entries(0)?;
    Ok((obj, r.pos))
}

/// Parses `input`, which must contain exactly one object and nothing after
/// it.
pub fn parse_all(input: &[u8], keys: Keys<'_>) -> Result<Object, Error> {
    let (obj, used) = parse(input, keys)?;
    if used != input.len() {
        return Err(Error {
            offset: used,
            kind: ErrorKind::UnknownType(input[used]),
        });
    }
    Ok(obj)
}

struct Reader<'a, 't> {
    buf: &'a [u8],
    pos: usize,
    keys: Keys<'t>,
}

impl Reader<'_, '_> {
    fn err(&self, offset: usize, kind: ErrorKind) -> Error {
        Error { offset, kind }
    }

    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let Some(end) = end else {
            return Err(self.err(self.buf.len(), ErrorKind::UnexpectedEof));
        };
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn cstr(&mut self) -> Result<String, Error> {
        let start = self.pos;
        let Some(len) = self.buf[start..].iter().position(|&b| b == 0) else {
            return Err(self.err(self.buf.len(), ErrorKind::UnexpectedEof));
        };
        let bytes = &self.buf[start..start + len];
        self.pos = start + len + 1;
        String::from_utf8(bytes.to_vec()).map_err(|_| self.err(start, ErrorKind::InvalidUtf8))
    }

    fn wstr(&mut self) -> Result<String, Error> {
        let start = self.pos;
        let mut units = Vec::new();
        loop {
            let u = u16::from_le_bytes(self.array()?);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        String::from_utf16(&units).map_err(|_| self.err(start, ErrorKind::InvalidUtf16))
    }

    fn key(&mut self) -> Result<String, Error> {
        match self.keys {
            Keys::Inline => self.cstr(),
            Keys::Table(table) => {
                let at = self.pos;
                let i = u32::from_le_bytes(self.array()?);
                table
                    .get(i as usize)
                    .cloned()
                    .ok_or_else(|| self.err(at, ErrorKind::BadKeyIndex(i)))
            }
        }
    }

    fn entries(&mut self, depth: usize) -> Result<Object, Error> {
        let mut obj = Object::new();
        loop {
            let at = self.pos;
            let Some(&tag) = self.buf.get(self.pos) else {
                if depth == 0 {
                    return Ok(obj);
                }
                return Err(self.err(at, ErrorKind::MissingEnd));
            };
            self.pos += 1;
            if tag == T_END || tag == T_END_ALT {
                return Ok(obj);
            }
            let key = self.key()?;
            let value = match tag {
                T_OBJECT => {
                    if depth + 1 >= MAX_DEPTH {
                        return Err(self.err(at, ErrorKind::TooDeep));
                    }
                    let child = self.entries(depth + 1)?;
                    Value::Object(child)
                }
                T_STRING => Value::String(self.cstr()?),
                T_INT32 => Value::Int32(i32::from_le_bytes(self.array()?)),
                T_FLOAT32 => Value::Float32(f32::from_le_bytes(self.array()?)),
                T_POINTER => Value::Pointer(u32::from_le_bytes(self.array()?)),
                T_WSTRING => Value::WideString(self.wstr()?),
                T_COLOR => Value::Color(u32::from_le_bytes(self.array()?)),
                T_UINT64 => Value::UInt64(u64::from_le_bytes(self.array()?)),
                T_INT64 => Value::Int64(i64::from_le_bytes(self.array()?)),
                other => return Err(self.err(at, ErrorKind::UnknownType(other))),
            };
            obj.entries.push(Entry {
                key,
                value,
                condition: None,
            });
        }
    }
}

/// Serialises an object with inline keys, including its final end marker.
///
/// Text conditionals have no binary form and are dropped.
pub fn to_vec(obj: &Object) -> Vec<u8> {
    let mut out = Vec::new();
    write_entries(&mut out, obj, &mut |out, key| {
        out.extend_from_slice(key.split('\0').next().unwrap_or_default().as_bytes());
        out.push(0);
    });
    out
}

/// Serialises an object in the string-table variant. Keys are interned
/// into `table` (existing entries are reused) and written as indices.
pub fn to_vec_with_table(obj: &Object, table: &mut Vec<String>) -> Vec<u8> {
    let mut out = Vec::new();
    write_entries(&mut out, obj, &mut |out, key| {
        let i = match table.iter().position(|k| k == key) {
            Some(i) => i,
            None => {
                table.push(key.to_owned());
                table.len() - 1
            }
        };
        let i = u32::try_from(i).expect("string table larger than u32::MAX entries");
        out.extend_from_slice(&i.to_le_bytes());
    });
    out
}

fn write_entries(out: &mut Vec<u8>, obj: &Object, key: &mut dyn FnMut(&mut Vec<u8>, &str)) {
    for e in &obj.entries {
        match &e.value {
            Value::Object(child) => {
                out.push(T_OBJECT);
                key(out, &e.key);
                write_entries(out, child, key);
            }
            Value::String(s) => {
                out.push(T_STRING);
                key(out, &e.key);
                // An embedded NUL would end the string early; truncate there
                // so what is written is what a reader sees.
                let s = s.split('\0').next().unwrap_or_default();
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Int32(n) => scalar(out, T_INT32, &e.key, key, &n.to_le_bytes()),
            Value::Float32(f) => scalar(out, T_FLOAT32, &e.key, key, &f.to_le_bytes()),
            Value::Pointer(n) => scalar(out, T_POINTER, &e.key, key, &n.to_le_bytes()),
            Value::WideString(s) => {
                out.push(T_WSTRING);
                key(out, &e.key);
                for u in s.encode_utf16().take_while(|&u| u != 0) {
                    out.extend_from_slice(&u.to_le_bytes());
                }
                out.extend_from_slice(&[0, 0]);
            }
            Value::Color(n) => scalar(out, T_COLOR, &e.key, key, &n.to_le_bytes()),
            Value::UInt64(n) => scalar(out, T_UINT64, &e.key, key, &n.to_le_bytes()),
            Value::Int64(n) => scalar(out, T_INT64, &e.key, key, &n.to_le_bytes()),
        }
    }
    out.push(T_END);
}

fn scalar(
    out: &mut Vec<u8>,
    tag: u8,
    name: &str,
    key: &mut dyn FnMut(&mut Vec<u8>, &str),
    bytes: &[u8],
) {
    out.push(tag);
    key(out, name);
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Object {
        let mut inner = Object::new();
        inner.push("s", "hello");
        inner.push("i", Value::Int32(-5));
        inner.push("f", Value::Float32(2.5));
        inner.push("p", Value::Pointer(0xdead_beef));
        inner.push("w", Value::WideString("wide ü".into()));
        inner.push("c", Value::Color(0x11223344));
        inner.push("u", Value::UInt64(u64::MAX));
        inner.push("l", Value::Int64(i64::MIN));
        inner.push("empty", Object::new());
        let mut root = Object::new();
        root.push("root", inner);
        root
    }

    #[test]
    fn layout_of_a_small_object() {
        let mut root = Object::new();
        root.push("a", "b");
        root.push("n", Value::Int32(1));
        assert_eq!(to_vec(&root), b"\x01a\0b\0\x02n\0\x01\0\0\0\x08");
    }

    #[test]
    fn round_trip_all_types() {
        let root = sample();
        let bytes = to_vec(&root);
        let (parsed, used) = parse(&bytes, Keys::Inline).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(parsed, root);
        assert_eq!(parse_all(&bytes, Keys::Inline).unwrap(), root);
    }

    #[test]
    fn round_trip_with_string_table() {
        let root = sample();
        let mut table = Vec::new();
        let bytes = to_vec_with_table(&root, &mut table);
        assert_eq!(table[0], "root");
        let parsed = parse_all(&bytes, Keys::Table(&table)).unwrap();
        assert_eq!(parsed, root);
        let e = parse_all(&bytes, Keys::Table(&table[..1])).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::BadKeyIndex(_)));
    }

    #[test]
    fn reports_consumed_bytes_for_concatenated_objects() {
        let mut a = Object::new();
        a.push("k", "v");
        let mut bytes = to_vec(&a);
        let first_len = bytes.len();
        bytes.extend(to_vec(&a));
        let (_, used) = parse(&bytes, Keys::Inline).unwrap();
        assert_eq!(used, first_len);
        assert!(parse_all(&bytes, Keys::Inline).is_err());
    }

    #[test]
    fn top_level_may_end_at_eof_but_nested_may_not() {
        assert_eq!(
            parse(b"\x01a\0b\0", Keys::Inline).unwrap().0.get_str("a"),
            Some("b")
        );
        let e = parse(b"\x00o\0\x01a\0b\0", Keys::Inline).unwrap_err();
        assert_eq!(e.kind, ErrorKind::MissingEnd);
        let (o, _) = parse(b"\x00o\0\x0b\x0b", Keys::Inline).unwrap();
        assert!(o.get_object("o").unwrap().is_empty());
    }

    #[test]
    fn malformed_input() {
        assert_eq!(
            parse(b"\x02n\0\x01\0", Keys::Inline).unwrap_err().kind,
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            parse(b"\x01a\0b", Keys::Inline).unwrap_err().kind,
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            parse(b"\x09a\0", Keys::Inline).unwrap_err().kind,
            ErrorKind::UnknownType(9)
        );
        assert_eq!(
            parse(b"\x01a\0\xff\0", Keys::Inline).unwrap_err().kind,
            ErrorKind::InvalidUtf8
        );
        assert_eq!(
            parse(b"\x05a\0\x00\xd8\0\0", Keys::Inline)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidUtf16
        );
        let deep: Vec<u8> = std::iter::repeat_n(*b"\x00k\0", MAX_DEPTH + 1)
            .flatten()
            .collect();
        assert_eq!(
            parse(&deep, Keys::Inline).unwrap_err().kind,
            ErrorKind::TooDeep
        );
    }

    #[test]
    fn embedded_nul_is_truncated_on_write() {
        let mut o = Object::new();
        o.push("k", "ab\0cd");
        let parsed = parse_all(&to_vec(&o), Keys::Inline).unwrap();
        assert_eq!(parsed.get_str("k"), Some("ab"));
    }
}
