// SPDX-License-Identifier: GPL-2.0-or-later
//! The D-Bus wire format (the D-Bus specification, "Message Protocol"):
//! types and signatures, values, and whole messages. Every type is read,
//! so any message can be parsed (and its parts we do not use ignored);
//! we write little-endian and read both byte orders.

use std::fmt;

/// The longest message the specification allows (128 MiB).
const MAX_MESSAGE: u64 = 128 << 20;
/// The longest array the specification allows (64 MiB).
const MAX_ARRAY: usize = 64 << 20;
/// 32 nested arrays and 32 nested structs.
const MAX_NESTING: usize = 32;
/// How deep values may nest, variants included.
const MAX_DEPTH: usize = 2 * MAX_NESTING;

/// A message that breaks the wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(&'static str);

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a malformed D-Bus message: {}", self.0)
    }
}

impl std::error::Error for WireError {}

type Result<T> = std::result::Result<T, WireError>;

/// One complete type, as a signature spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Byte,
    Bool,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F64,
    Str,
    Path,
    Sig,
    Fd,
    Variant,
    Array(Box<Type>),
    Struct(Vec<Type>),
    /// A dictionary entry; only ever an array's element.
    Entry(Box<Type>, Box<Type>),
}

impl Type {
    /// The types a signature lists, in order.
    pub fn parse(signature: &str) -> Result<Vec<Type>> {
        let s = signature.as_bytes();
        if s.len() > 255 {
            return Err(WireError("a signature longer than 255 bytes"));
        }
        let mut at = 0;
        let mut types = Vec::new();
        while at < s.len() {
            types.push(Self::one(s, &mut at, 0, 0)?);
        }
        Ok(types)
    }

    fn one(s: &[u8], at: &mut usize, arrays: usize, structs: usize) -> Result<Type> {
        let Some(&code) = s.get(*at) else {
            return Err(WireError("a signature that ends inside a type"));
        };
        *at += 1;
        Ok(match code {
            b'y' => Type::Byte,
            b'b' => Type::Bool,
            b'n' => Type::I16,
            b'q' => Type::U16,
            b'i' => Type::I32,
            b'u' => Type::U32,
            b'x' => Type::I64,
            b't' => Type::U64,
            b'd' => Type::F64,
            b's' => Type::Str,
            b'o' => Type::Path,
            b'g' => Type::Sig,
            b'h' => Type::Fd,
            b'v' => Type::Variant,
            b'a' if arrays >= MAX_NESTING => return Err(WireError("arrays nested too deep")),
            b'a' if s.get(*at) == Some(&b'{') => {
                if structs >= MAX_NESTING {
                    return Err(WireError("structs nested too deep"));
                }
                *at += 1;
                let key = Self::one(s, at, arrays + 1, structs + 1)?;
                if !key.is_basic() {
                    return Err(WireError("a dictionary key that is not a basic type"));
                }
                let value = Self::one(s, at, arrays + 1, structs + 1)?;
                if s.get(*at) != Some(&b'}') {
                    return Err(WireError("a dictionary entry without exactly two types"));
                }
                *at += 1;
                Type::Array(Box::new(Type::Entry(Box::new(key), Box::new(value))))
            }
            b'a' => Type::Array(Box::new(Self::one(s, at, arrays + 1, structs)?)),
            b'(' if structs >= MAX_NESTING => return Err(WireError("structs nested too deep")),
            b'(' => {
                let mut fields = Vec::new();
                loop {
                    match s.get(*at) {
                        Some(b')') => break,
                        None => return Err(WireError("a struct that is not closed")),
                        Some(_) => fields.push(Self::one(s, at, arrays, structs + 1)?),
                    }
                }
                *at += 1;
                if fields.is_empty() {
                    return Err(WireError("an empty struct"));
                }
                Type::Struct(fields)
            }
            _ => return Err(WireError("an unknown type code in a signature")),
        })
    }

    fn is_basic(&self) -> bool {
        !matches!(
            self,
            Type::Variant | Type::Array(_) | Type::Struct(_) | Type::Entry(..)
        )
    }

    /// The boundary a value of this type starts on.
    fn align(&self) -> usize {
        match self {
            Type::Byte | Type::Sig | Type::Variant => 1,
            Type::I16 | Type::U16 => 2,
            Type::Bool | Type::I32 | Type::U32 | Type::Str | Type::Path | Type::Fd => 4,
            Type::Array(_) => 4,
            Type::I64 | Type::U64 | Type::F64 | Type::Struct(_) | Type::Entry(..) => 8,
        }
    }

    fn write_signature(&self, out: &mut String) {
        let code = match self {
            Type::Byte => 'y',
            Type::Bool => 'b',
            Type::I16 => 'n',
            Type::U16 => 'q',
            Type::I32 => 'i',
            Type::U32 => 'u',
            Type::I64 => 'x',
            Type::U64 => 't',
            Type::F64 => 'd',
            Type::Str => 's',
            Type::Path => 'o',
            Type::Sig => 'g',
            Type::Fd => 'h',
            Type::Variant => 'v',
            Type::Array(element) => {
                out.push('a');
                return element.write_signature(out);
            }
            Type::Struct(fields) => {
                out.push('(');
                fields.iter().for_each(|f| f.write_signature(out));
                out.push(')');
                return;
            }
            Type::Entry(key, value) => {
                out.push('{');
                key.write_signature(out);
                value.write_signature(out);
                out.push('}');
                return;
            }
        };
        out.push(code);
    }
}

/// The signature of values of these types, one after another.
pub fn signature<'a>(types: impl IntoIterator<Item = &'a Type>) -> String {
    let mut out = String::new();
    types.into_iter().for_each(|t| t.write_signature(&mut out));
    out
}

/// A value of any D-Bus type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Path(String),
    Sig(String),
    /// An index into the message's file descriptors; we never ask for any.
    Fd(u32),
    Variant(Box<Value>),
    /// The element type, so that an empty array has a signature too.
    Array(Type, Vec<Value>),
    Struct(Vec<Value>),
    Entry(Box<Value>, Box<Value>),
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Value::Byte(_) => Type::Byte,
            Value::Bool(_) => Type::Bool,
            Value::I16(_) => Type::I16,
            Value::U16(_) => Type::U16,
            Value::I32(_) => Type::I32,
            Value::U32(_) => Type::U32,
            Value::I64(_) => Type::I64,
            Value::U64(_) => Type::U64,
            Value::F64(_) => Type::F64,
            Value::Str(_) => Type::Str,
            Value::Path(_) => Type::Path,
            Value::Sig(_) => Type::Sig,
            Value::Fd(_) => Type::Fd,
            Value::Variant(_) => Type::Variant,
            Value::Array(element, _) => Type::Array(Box::new(element.clone())),
            Value::Struct(fields) => Type::Struct(fields.iter().map(Value::ty).collect()),
            Value::Entry(key, value) => Type::Entry(Box::new(key.ty()), Box::new(value.ty())),
        }
    }

    /// An `a{sv}`, the options and results dictionary of most interfaces.
    pub fn dict(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        let entry = Type::Entry(Box::new(Type::Str), Box::new(Type::Variant));
        Value::Array(
            entry,
            entries
                .into_iter()
                .map(|(key, value)| {
                    Value::Entry(
                        Box::new(Value::Str(key.into())),
                        Box::new(Value::Variant(Box::new(value))),
                    )
                })
                .collect(),
        )
    }

    /// The value under `key` in an `a{sv}`, out of its variant.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let Value::Array(_, entries) = self else {
            return None;
        };
        entries.iter().find_map(|e| match e {
            Value::Entry(k, v) if matches!(&**k, Value::Str(s) if s == key) => Some(v.unwrap()),
            _ => None,
        })
    }

    /// The value inside any number of variants.
    pub fn unwrap(&self) -> &Value {
        let mut v = self;
        while let Value::Variant(inner) = v {
            v = inner;
        }
        v
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Path(s) | Value::Sig(s) => Some(s),
            _ => None,
        }
    }
}

/// Values written little-endian, aligned from the start of the buffer.
#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn pad(&mut self, align: usize) {
        let len = self.buf.len().next_multiple_of(align);
        self.buf.resize(len, 0);
    }

    fn fixed<const N: usize>(&mut self, bytes: [u8; N]) {
        self.pad(N);
        self.buf.extend_from_slice(&bytes);
    }

    fn string(&mut self, s: &str) {
        self.fixed((s.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, s: &str) {
        self.buf.push(s.len() as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn value(&mut self, v: &Value) {
        match v {
            Value::Byte(b) => self.buf.push(*b),
            Value::Bool(b) => self.fixed(u32::from(*b).to_le_bytes()),
            Value::I16(n) => self.fixed(n.to_le_bytes()),
            Value::U16(n) => self.fixed(n.to_le_bytes()),
            Value::I32(n) => self.fixed(n.to_le_bytes()),
            Value::U32(n) | Value::Fd(n) => self.fixed(n.to_le_bytes()),
            Value::I64(n) => self.fixed(n.to_le_bytes()),
            Value::U64(n) => self.fixed(n.to_le_bytes()),
            Value::F64(n) => self.fixed(n.to_le_bytes()),
            Value::Str(s) | Value::Path(s) => self.string(s),
            Value::Sig(s) => self.signature(s),
            Value::Variant(inner) => {
                self.signature(&signature([&inner.ty()]));
                self.value(inner);
            }
            Value::Array(element, items) => {
                self.pad(4);
                let at = self.buf.len();
                self.buf.extend_from_slice(&[0; 4]);
                // The length leaves out the padding before the first element.
                self.pad(element.align());
                let start = self.buf.len();
                items.iter().for_each(|i| self.value(i));
                let len = (self.buf.len() - start) as u32;
                self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
            }
            Value::Struct(fields) => {
                self.pad(8);
                fields.iter().for_each(|f| self.value(f));
            }
            Value::Entry(key, value) => {
                self.pad(8);
                self.value(key);
                self.value(value);
            }
        }
    }
}

/// Values read in either byte order, aligned from the start of `buf`.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
    big_endian: bool,
}

const SHORT: WireError = WireError("it ends early");

impl<'a> Reader<'a> {
    fn align(&mut self, align: usize) -> Result<()> {
        let to = self.at.next_multiple_of(align);
        if to > self.buf.len() {
            return Err(SHORT);
        }
        self.at = to;
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.buf.len())
            .ok_or(SHORT)?;
        let bytes = &self.buf[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    /// `N` bytes on an `N`-byte boundary, as little-endian.
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.align(N)?;
        let mut bytes = [0; N];
        bytes.copy_from_slice(self.take(N)?);
        if self.big_endian {
            bytes.reverse();
        }
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.fixed().map(u32::from_le_bytes)
    }

    fn text(&mut self, len: usize) -> Result<String> {
        let bytes = self.take(len)?;
        if self.take(1)? != [0] {
            return Err(WireError("a string without its terminating nul"));
        }
        String::from_utf8(bytes.to_vec()).map_err(|_| WireError("a string that is not UTF-8"))
    }

    fn value(&mut self, ty: &Type, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(WireError("values nested too deep"));
        }
        Ok(match ty {
            Type::Byte => Value::Byte(self.take(1)?[0]),
            Type::Bool => match self.u32()? {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(WireError("a boolean other than 0 or 1")),
            },
            Type::I16 => Value::I16(i16::from_le_bytes(self.fixed()?)),
            Type::U16 => Value::U16(u16::from_le_bytes(self.fixed()?)),
            Type::I32 => Value::I32(i32::from_le_bytes(self.fixed()?)),
            Type::U32 => Value::U32(self.u32()?),
            Type::I64 => Value::I64(i64::from_le_bytes(self.fixed()?)),
            Type::U64 => Value::U64(u64::from_le_bytes(self.fixed()?)),
            Type::F64 => Value::F64(f64::from_le_bytes(self.fixed()?)),
            Type::Str => Value::Str(self.u32().and_then(|n| self.text(n as usize))?),
            Type::Path => Value::Path(self.u32().and_then(|n| self.text(n as usize))?),
            Type::Sig => Value::Sig(self.take(1).and_then(|n| self.text(n[0].into()))?),
            Type::Fd => Value::Fd(self.u32()?),
            Type::Variant => {
                let sig = self.take(1).and_then(|n| self.text(n[0].into()))?;
                let mut types = Type::parse(&sig)?;
                let (Some(inner), true) = (types.pop(), types.is_empty()) else {
                    return Err(WireError("a variant whose signature is not one type"));
                };
                Value::Variant(Box::new(self.value(&inner, depth + 1)?))
            }
            Type::Array(element) => {
                let len = self.u32()? as usize;
                if len > MAX_ARRAY {
                    return Err(WireError("an array longer than 64 MiB"));
                }
                self.align(element.align())?;
                let end = self.at + len;
                if end > self.buf.len() {
                    return Err(SHORT);
                }
                let mut items = Vec::new();
                while self.at < end {
                    items.push(self.value(element, depth + 1)?);
                }
                if self.at != end {
                    return Err(WireError("an array whose elements overrun its length"));
                }
                Value::Array((**element).clone(), items)
            }
            Type::Struct(fields) => {
                self.align(8)?;
                Value::Struct(
                    fields
                        .iter()
                        .map(|f| self.value(f, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            Type::Entry(key, value) => {
                self.align(8)?;
                let key = self.value(key, depth + 1)?;
                Value::Entry(Box::new(key), Box::new(self.value(value, depth + 1)?))
            }
        })
    }
}

/// What a message is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Call,
    Return,
    Error,
    Signal,
    /// A type this version of the protocol does not know; ignored.
    Other(u8),
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::Call => 1,
            Kind::Return => 2,
            Kind::Error => 3,
            Kind::Signal => 4,
            Kind::Other(code) => code,
        }
    }
}

/// The message flag that says no reply is wanted.
pub const NO_REPLY_EXPECTED: u8 = 0x1;

/// A message: its header fields and its body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub kind: Kind,
    pub flags: u8,
    /// Set by the connection that sends it; never 0 on the wire.
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub body: Vec<Value>,
}

/// A header field: its code and its value in a variant.
fn header_type() -> Type {
    Type::Array(Box::new(Type::Struct(vec![Type::Byte, Type::Variant])))
}

impl Message {
    /// A method call.
    pub fn call(
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        body: Vec<Value>,
    ) -> Self {
        Message {
            destination: Some(destination.into()),
            path: Some(path.into()),
            interface: Some(interface.into()),
            member: Some(member.into()),
            body,
            ..Message::default()
        }
    }

    /// Whether this is the signal `interface.member` from `path`.
    pub fn is_signal(&self, path: &str, interface: &str, member: &str) -> bool {
        self.kind == Kind::Signal
            && self.path.as_deref() == Some(path)
            && self.interface.as_deref() == Some(interface)
            && self.member.as_deref() == Some(member)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Writer::default();
        self.body.iter().for_each(|v| body.value(v));

        let mut fields = Vec::new();
        let mut field = |code, value| {
            fields.push(Value::Struct(vec![
                Value::Byte(code),
                Value::Variant(Box::new(value)),
            ]));
        };
        let text = |s: &Option<String>| s.clone();
        if let Some(p) = text(&self.path) {
            field(1, Value::Path(p));
        }
        if let Some(s) = text(&self.interface) {
            field(2, Value::Str(s));
        }
        if let Some(s) = text(&self.member) {
            field(3, Value::Str(s));
        }
        if let Some(s) = text(&self.error_name) {
            field(4, Value::Str(s));
        }
        if let Some(n) = self.reply_serial {
            field(5, Value::U32(n));
        }
        if let Some(s) = text(&self.destination) {
            field(6, Value::Str(s));
        }
        if let Some(s) = text(&self.sender) {
            field(7, Value::Str(s));
        }
        if !self.body.is_empty() {
            field(
                8,
                Value::Sig(signature(
                    &self.body.iter().map(Value::ty).collect::<Vec<_>>(),
                )),
            );
        }

        let mut w = Writer::default();
        w.buf
            .extend_from_slice(&[b'l', self.kind.code(), self.flags, 1]);
        w.fixed((body.buf.len() as u32).to_le_bytes());
        w.fixed(self.serial.to_le_bytes());
        w.value(&Value::Array(
            Type::Struct(vec![Type::Byte, Type::Variant]),
            fields,
        ));
        w.pad(8);
        w.buf.extend_from_slice(&body.buf);
        w.buf
    }

    /// A message's whole length, from its first 16 bytes.
    pub fn len_from_start(start: &[u8; 16]) -> Result<usize> {
        let big_endian = match start[0] {
            b'l' => false,
            b'B' => true,
            _ => return Err(WireError("an unknown byte order")),
        };
        let mut r = Reader {
            buf: start,
            at: 4,
            big_endian,
        };
        let body = u64::from(r.u32()?);
        r.at = 12;
        let fields = u64::from(r.u32()?);
        let len = (16 + fields).next_multiple_of(8) + body;
        if len > MAX_MESSAGE {
            return Err(WireError("a message longer than 128 MiB"));
        }
        Ok(len as usize)
    }

    pub fn decode(buf: &[u8]) -> Result<Message> {
        let big_endian = match buf.first() {
            Some(b'l') => false,
            Some(b'B') => true,
            _ => return Err(WireError("an unknown byte order")),
        };
        if buf.len() < 16 {
            return Err(SHORT);
        }
        if buf[3] != 1 {
            return Err(WireError("a protocol version other than 1"));
        }
        let mut r = Reader {
            buf,
            at: 4,
            big_endian,
        };
        let body_len = r.u32()? as usize;
        let serial = r.u32()?;
        if serial == 0 {
            return Err(WireError("serial 0"));
        }
        let kind = match buf[1] {
            1 => Kind::Call,
            2 => Kind::Return,
            3 => Kind::Error,
            4 => Kind::Signal,
            code => Kind::Other(code),
        };
        let mut m = Message {
            kind,
            flags: buf[2],
            serial,
            ..Message::default()
        };

        let mut body_signature = String::new();
        let Value::Array(_, fields) = r.value(&header_type(), 0)? else {
            unreachable!("an array type reads as an array");
        };
        for f in fields {
            let Value::Struct(f) = f else { continue };
            let [Value::Byte(code), Value::Variant(value)] = &f[..] else {
                continue;
            };
            let wrong = WireError("a header field of the wrong type");
            let text = || value.as_str().map(str::to_owned);
            match (code, &**value) {
                (1, Value::Path(_)) => m.path = text(),
                (2, Value::Str(_)) => m.interface = text(),
                (3, Value::Str(_)) => m.member = text(),
                (4, Value::Str(_)) => m.error_name = text(),
                (5, Value::U32(n)) => m.reply_serial = Some(*n),
                (6, Value::Str(_)) => m.destination = text(),
                (7, Value::Str(_)) => m.sender = text(),
                (8, Value::Sig(s)) => body_signature.clone_from(s),
                (1..=8, _) => return Err(wrong),
                // Unknown fields are ignored, as the specification asks.
                _ => {}
            }
        }
        r.align(8)?;
        let body = r.take(body_len)?;
        if r.at != buf.len() {
            return Err(WireError("bytes after the body"));
        }

        let mut b = Reader {
            buf: body,
            at: 0,
            big_endian,
        };
        for ty in Type::parse(&body_signature)? {
            m.body.push(b.value(&ty, 0)?);
        }
        if b.at != body.len() {
            return Err(WireError("a body longer than its signature"));
        }

        let required = match m.kind {
            Kind::Call => m.path.is_some() && m.member.is_some(),
            Kind::Signal => m.path.is_some() && m.interface.is_some() && m.member.is_some(),
            Kind::Return => m.reply_serial.is_some(),
            Kind::Error => m.reply_serial.is_some() && m.error_name.is_some(),
            Kind::Other(_) => true,
        };
        if !required {
            return Err(WireError("a header field its type requires is missing"));
        }
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Encoded by GLib's GDBusMessage (to_blob), serial 5: the portal's
    // Response signal with (0, {"uris": <["file:///a%20b"]>}) from ":1.7".
    const SIGNAL_LE: &str = "6c0401012e000000050000009100000007017300040000003a312e370000000001016f002e0000002f6f72672f667265656465736b746f702f706f7274616c2f6465736b746f702f726571756573742f315f34322f740000020173001e0000006f72672e667265656465736b746f702e706f7274616c2e526571756573740000080167000675617b73767d00000000000301730008000000526573706f6e73650000000000000000000000002600000004000000757269730002617300000000120000000d00000066696c653a2f2f2f612532306200";
    const SIGNAL_BE: &str = "420401010000002e000000050000009107017300000000043a312e370000000001016f000000002e2f6f72672f667265656465736b746f702f706f7274616c2f6465736b746f702f726571756573742f315f34322f740000020173000000001e6f72672e667265656465736b746f702e706f7274616c2e526571756573740000080167000675617b73767d00000000000301730000000008526573706f6e73650000000000000000000000000000002600000004757269730002617300000000000000120000000d66696c653a2f2f2f612532306200";
    // The same encoder: OpenFile("", "Add", {"handle_token": <"t">,
    // "directory": <true>}), serial 5.
    const CALL_LE: &str = "6c0100015000000005000000a100000001016f001f0000002f6f72672f667265656465736b746f702f706f7274616c2f6465736b746f700002017300220000006f72672e667265656465736b746f702e706f7274616c2e46696c6543686f6f736572000000000000060173001e0000006f72672e667265656465736b746f702e706f7274616c2e4465736b746f70000008016700077373617b73767d0000000003017300080000004f70656e46696c6500000000000000000000000000000000030000004164640038000000000000000c00000068616e646c655f746f6b656e00017300010000007400000000000000090000006469726563746f72790001620000000001000000";

    fn open_file() -> Message {
        let mut m = Message::call(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.FileChooser",
            "OpenFile",
            vec![
                Value::Str(String::new()),
                Value::Str("Add".into()),
                Value::dict([
                    ("handle_token", Value::Str("t".into())),
                    ("directory", Value::Bool(true)),
                ]),
            ],
        );
        m.serial = 5;
        m
    }

    #[test]
    fn reads_glibs_signal_in_both_byte_orders() {
        for blob in [SIGNAL_LE, SIGNAL_BE] {
            let bytes = hex(blob);
            let start: [u8; 16] = bytes[..16].try_into().unwrap();
            assert_eq!(Message::len_from_start(&start), Ok(bytes.len()));
            let m = Message::decode(&bytes).unwrap();
            assert!(m.is_signal(
                "/org/freedesktop/portal/desktop/request/1_42/t",
                "org.freedesktop.portal.Request",
                "Response"
            ));
            assert_eq!((m.serial, m.sender.as_deref()), (5, Some(":1.7")));
            assert_eq!(m.body[0], Value::U32(0));
            let uris = m.body[1].get("uris").unwrap();
            assert_eq!(
                uris,
                &Value::Array(Type::Str, vec![Value::Str("file:///a%20b".into())])
            );
        }
    }

    #[test]
    fn reads_glibs_call_and_writes_its_body_byte_for_byte() {
        let glib = hex(CALL_LE);
        assert_eq!(Message::decode(&glib).unwrap(), open_file());
        // GLib orders the header fields its own way, so only the bodies,
        // which are the end of each message, compare byte for byte.
        let ours = open_file().encode();
        let body = u32::from_le_bytes(glib[4..8].try_into().unwrap()) as usize;
        assert_eq!(ours[ours.len() - body..], glib[glib.len() - body..]);
        assert_eq!(Message::decode(&ours).unwrap(), open_file());
    }

    #[test]
    fn every_type_round_trips() {
        let mut m = Message {
            kind: Kind::Signal,
            serial: u32::MAX,
            path: Some("/a".into()),
            interface: Some("a.b".into()),
            member: Some("C".into()),
            sender: Some(":1.1".into()),
            body: vec![
                Value::Byte(7),
                Value::Bool(false),
                Value::I16(-2),
                Value::U16(3),
                Value::I32(-4),
                Value::U32(5),
                Value::I64(-6),
                Value::U64(7),
                Value::F64(0.5),
                Value::Str("é".into()),
                Value::Path("/o".into()),
                Value::Sig("a{sv}".into()),
                Value::Fd(0),
                Value::Array(Type::U64, vec![]),
                Value::Array(Type::Byte, vec![Value::Byte(1), Value::Byte(2)]),
                Value::Struct(vec![
                    Value::Byte(1),
                    Value::Variant(Box::new(Value::U64(9))),
                ]),
                Value::dict([("k", Value::Variant(Box::new(Value::Str("v".into()))))]),
            ],
            ..Message::default()
        };
        assert_eq!(Message::decode(&m.encode()), Ok(m.clone()));
        m.kind = Kind::Other(9);
        assert_eq!(Message::decode(&m.encode()).unwrap().kind, Kind::Other(9));
    }

    #[test]
    fn signatures() {
        let parse = |s: &str| Type::parse(s).map(|t| signature(&t));
        for good in ["", "a{sv}", "(ua{sv})", "aa(ii)", "a{oa{sa{sv}}}", "yv"] {
            assert_eq!(parse(good).as_deref(), Ok(good));
        }
        for bad in ["a", "{sv}", "(", "()", "a{vs}", "a{sss}", "a{s", ")", "z"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&format!("{}y", "a".repeat(32))).is_ok());
        assert!(parse(&format!("{}y", "a".repeat(33))).is_err());
        assert!(parse(&format!("{}y{}", "(".repeat(33), ")".repeat(33))).is_err());
    }

    #[test]
    fn rejects_malformed_messages() {
        let good = open_file().encode();
        for end in [0, 15, 16, good.len() - 1] {
            assert!(Message::decode(&good[..end]).is_err(), "{end}");
        }
        let mut longer = good.clone();
        longer.push(0);
        assert!(Message::decode(&longer).is_err());

        // A boolean of 2: the last four bytes are `directory`'s value.
        let mut two = good.clone();
        let n = two.len();
        two[n - 4] = 2;
        assert!(Message::decode(&two).is_err());

        // A string that is not UTF-8: the "Add" of the title.
        let mut latin = good.clone();
        let at = latin.windows(3).position(|w| w == b"Add").unwrap();
        latin[at] = 0xe9;
        assert!(Message::decode(&latin).is_err());

        // A method call without its member.
        let mut m = open_file();
        m.member = None;
        assert!(Message::decode(&m.encode()).is_err());

        // Serial 0, and an unknown byte order.
        let mut zero = good.clone();
        zero[8..12].fill(0);
        assert!(Message::decode(&zero).is_err());
        let mut order = good;
        order[0] = b'x';
        assert!(Message::decode(&order).is_err());
    }

    #[test]
    fn variants_nest_only_so_deep() {
        let mut v = Value::U32(1);
        for _ in 0..MAX_DEPTH + 1 {
            v = Value::Variant(Box::new(v));
        }
        let m = Message {
            kind: Kind::Signal,
            serial: 1,
            path: Some("/a".into()),
            interface: Some("a.b".into()),
            member: Some("C".into()),
            body: vec![v],
            ..Message::default()
        };
        assert!(Message::decode(&m.encode()).is_err());
    }

    #[test]
    fn dictionary_lookup_unwraps_variants() {
        let d = Value::dict([("a", Value::Variant(Box::new(Value::U32(2))))]);
        assert_eq!(d.get("a"), Some(&Value::U32(2)));
        assert_eq!(d.get("b"), None);
        assert_eq!(Value::U32(1).get("a"), None);
    }
}
