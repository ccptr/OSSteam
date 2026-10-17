// SPDX-License-Identifier: GPL-2.0-or-later
//! Single-entry zip archives, the wrapping of manifests and of one chunk
//! format (`content.md` §5.3).
//!
//! Only what Steam uses is read: one entry, stored or deflated, no zip64,
//! no encryption. The entry is found through the central directory, so
//! local headers with zero sizes (data descriptors) do not matter.

use std::io::Read;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END_LEN: usize = 22;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ZipError {
    #[error("no end of central directory record")]
    NoEnd,
    #[error("the archive holds {0} entries; exactly one is expected")]
    Entries(u16),
    #[error("a header is cut short or out of place")]
    Truncated,
    #[error("zip64 archives are not supported")]
    Zip64,
    #[error("compression method {0} is not supported")]
    Method(u16),
    #[error("the entry is {0} bytes, over the {1}-byte limit")]
    TooLarge(u64, u64),
    #[error("the entry does not inflate: {0}")]
    Inflate(String),
    #[error("the entry inflated to {got} bytes; its header says {want}")]
    Size { got: u64, want: u64 },
    #[error("the entry's CRC-32 does not match")]
    Crc,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, ZipError> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(ZipError::Truncated)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, ZipError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(ZipError::Truncated)
}

/// The contents of the archive's only entry, which may not exceed `limit`
/// bytes once decompressed.
pub fn extract_single(archive: &[u8], limit: u64) -> Result<Vec<u8>, ZipError> {
    // The end record is the last thing in the file, followed only by a
    // comment of up to 65535 bytes.
    let lowest = archive.len().saturating_sub(END_LEN + 0xFFFF);
    let end = (lowest..=archive.len().saturating_sub(END_LEN))
        .rev()
        .find(|&i| u32_at(archive, i) == Ok(END))
        .ok_or(ZipError::NoEnd)?;
    let entries = u16_at(archive, end + 10)?;
    if entries != 1 {
        return Err(ZipError::Entries(entries));
    }
    let cd = u32_at(archive, end + 16)?;
    if cd == u32::MAX {
        return Err(ZipError::Zip64);
    }
    let cd = cd as usize;
    if u32_at(archive, cd)? != CENTRAL {
        return Err(ZipError::Truncated);
    }
    let method = u16_at(archive, cd + 10)?;
    let crc = u32_at(archive, cd + 16)?;
    let packed = u32_at(archive, cd + 20)?;
    let size = u32_at(archive, cd + 24)?;
    let local = u32_at(archive, cd + 42)?;
    if [packed, size, local].contains(&u32::MAX) {
        return Err(ZipError::Zip64);
    }
    let local = local as usize;
    if u32_at(archive, local)? != LOCAL {
        return Err(ZipError::Truncated);
    }
    let name_len = u16_at(archive, local + 26)? as usize;
    let extra_len = u16_at(archive, local + 28)? as usize;
    let start = local + 30 + name_len + extra_len;
    let data = start
        .checked_add(packed as usize)
        .and_then(|end| archive.get(start..end))
        .ok_or(ZipError::Truncated)?;
    let size = u64::from(size);
    if size > limit {
        return Err(ZipError::TooLarge(size, limit));
    }
    let out = match method {
        0 => data.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(size as usize);
            flate2::read::DeflateDecoder::new(data)
                .take(size + 1)
                .read_to_end(&mut out)
                .map_err(|e| ZipError::Inflate(e.to_string()))?;
            out
        }
        m => return Err(ZipError::Method(m)),
    };
    if out.len() as u64 != size {
        return Err(ZipError::Size {
            got: out.len() as u64,
            want: size,
        });
    }
    if crc32fast::hash(&out) != crc {
        return Err(ZipError::Crc);
    }
    Ok(out)
}

/// A one-entry deflated archive named `name`, laid out as Steam's are,
/// with a data descriptor so the local header carries no sizes.
#[cfg(any(test, feature = "testing"))]
pub fn write_single(name: &str, data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).expect("writing to a Vec");
    let packed = enc.finish().expect("writing to a Vec");
    let crc = crc32fast::hash(data);
    let mut out = Vec::new();
    let put16 = |o: &mut Vec<u8>, v: u16| o.extend(v.to_le_bytes());
    let put32 = |o: &mut Vec<u8>, v: u32| o.extend(v.to_le_bytes());

    put32(&mut out, LOCAL);
    put16(&mut out, 20); // version needed
    put16(&mut out, 0x08); // sizes in a data descriptor
    put16(&mut out, 8);
    put32(&mut out, 0); // time, date
    put32(&mut out, 0); // crc
    put32(&mut out, 0); // sizes
    put32(&mut out, 0);
    put16(&mut out, name.len() as u16);
    put16(&mut out, 0);
    out.extend(name.as_bytes());
    out.extend(&packed);
    put32(&mut out, 0x0807_4b50);
    put32(&mut out, crc);
    put32(&mut out, packed.len() as u32);
    put32(&mut out, data.len() as u32);

    let cd = out.len();
    put32(&mut out, CENTRAL);
    put16(&mut out, 20);
    put16(&mut out, 20);
    put16(&mut out, 0x08);
    put16(&mut out, 8);
    put32(&mut out, 0);
    put32(&mut out, crc);
    put32(&mut out, packed.len() as u32);
    put32(&mut out, data.len() as u32);
    put16(&mut out, name.len() as u16);
    put16(&mut out, 0); // extra
    put16(&mut out, 0); // comment
    put16(&mut out, 0); // disk
    put16(&mut out, 0); // internal attributes
    put32(&mut out, 0); // external attributes
    put32(&mut out, 0); // local header offset
    out.extend(name.as_bytes());
    let cd_len = out.len() - cd;

    put32(&mut out, END);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, 1);
    put16(&mut out, 1);
    put32(&mut out, cd_len as u32);
    put32(&mut out, cd as u32);
    put16(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_deflated_entry_with_a_data_descriptor() {
        let data = b"manifest bytes ".repeat(100);
        let zip = write_single("z", &data);
        assert_eq!(extract_single(&zip, 1 << 20).unwrap(), data);
    }

    #[test]
    fn checks_crc_size_and_limit() {
        let data = b"abcdefgh".repeat(50);
        let mut zip = write_single("z", &data);
        assert_eq!(extract_single(&zip, 10), Err(ZipError::TooLarge(400, 10)));
        // Corrupt the CRC in the central directory.
        let cd = u32_at(&zip, zip.len() - 6).unwrap() as usize;
        zip[cd + 16] ^= 1;
        assert_eq!(extract_single(&zip, 1 << 20), Err(ZipError::Crc));
    }

    #[test]
    fn refuses_garbage_and_truncation() {
        assert_eq!(extract_single(b"", 100), Err(ZipError::NoEnd));
        assert_eq!(extract_single(&[0; 64], 100), Err(ZipError::NoEnd));
        let zip = write_single("z", b"hello");
        let cut = &zip[20..];
        assert!(extract_single(cut, 100).is_err());
    }
}
