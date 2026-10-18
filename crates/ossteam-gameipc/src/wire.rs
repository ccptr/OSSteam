// SPDX-License-Identifier: GPL-2.0-or-later
//! The field encoding of `steamworks-abi.md` §3.2: little-endian
//! integers, one-byte booleans, length-prefixed strings and lists.

/// The input ended early or held a value that cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed message: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

/// Reads fields from a message body.
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() < n {
            return Err(DecodeError(format!(
                "{n} bytes needed, {} left",
                self.buf.len()
            )));
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    /// Fails unless every byte was read.
    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(DecodeError(format!("{} bytes left over", self.buf.len())))
        }
    }
}

/// A type with an encoding.
pub trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError>;
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                let b = r.take(size_of::<$t>())?;
                Ok(<$t>::from_le_bytes(b.try_into().expect("taken to size")))
            }
        }
    )*};
}
int!(u8, u16, u32, u64, i32, i64);

impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::get(r)? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(DecodeError(format!("{b} is not a boolean"))),
        }
    }
}

impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let n = u32::get(r)? as usize;
        let b = r.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError("a string is not UTF-8".into()))
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        for item in self {
            item.put(out);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let n = u32::get(r)? as usize;
        // Each item takes at least a byte, so a count beyond what is left
        // is malformed; checking first bounds the allocation.
        if n > r.buf.len() {
            return Err(DecodeError(format!("a list of {n} items cannot fit")));
        }
        (0..n).map(|_| T::get(r)).collect()
    }
}

/// Declares a struct whose fields are encoded in order.
macro_rules! record {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$fmeta:meta])* pub $field:ident: $ty:ty,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Default)]
        pub struct $name { $($(#[$fmeta])* pub $field: $ty,)* }

        impl $crate::wire::Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                $($crate::wire::Wire::put(&self.$field, out);)*
            }
            fn get(r: &mut $crate::wire::Reader<'_>) -> Result<Self, $crate::wire::DecodeError> {
                Ok(Self { $($field: $crate::wire::Wire::get(r)?,)* })
            }
        }
    };
}
pub(crate) use record;
