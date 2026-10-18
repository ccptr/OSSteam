// SPDX-License-Identifier: GPL-2.0-or-later
//! SteamStub detection (`launch.md` §7, SPEC §13.2): a PE image with a
//! `.bind` section. Only headers are read; nothing is changed (DRM-2).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// More sections than any real image has; a larger count is not a PE
/// image this check needs to understand.
const MAX_SECTIONS: u16 = 96;

/// Whether the file at `path` is a PE image with a `.bind` section. A
/// file that is not a PE image is not protected.
pub fn is_protected(path: &Path) -> io::Result<bool> {
    let mut f = File::open(path)?;
    let mut dos = [0u8; 64];
    if !read_full(&mut f, &mut dos)? || &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let pe_offset = u32::from_le_bytes(dos[60..64].try_into().unwrap());
    f.seek(SeekFrom::Start(u64::from(pe_offset)))?;
    // The signature and the COFF header.
    let mut coff = [0u8; 24];
    if !read_full(&mut f, &mut coff)? || &coff[..4] != b"PE\0\0" {
        return Ok(false);
    }
    let sections = u16::from_le_bytes([coff[6], coff[7]]);
    let optional_size = u16::from_le_bytes([coff[20], coff[21]]);
    if sections > MAX_SECTIONS {
        return Ok(false);
    }
    f.seek(SeekFrom::Current(i64::from(optional_size)))?;
    let mut header = [0u8; 40];
    for _ in 0..sections {
        if !read_full(&mut f, &mut header)? {
            return Ok(false);
        }
        if &header[..8] == b".bind\0\0\0" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Fills `buf`, or returns false when the file ends first.
fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<bool> {
    match f.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PE image with the given section names.
    fn pe(sections: &[&[u8; 8]]) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..2].copy_from_slice(b"MZ");
        v[60..64].copy_from_slice(&128u32.to_le_bytes());
        v.resize(128, 0);
        v.extend_from_slice(b"PE\0\0");
        let mut coff = [0u8; 20];
        coff[2..4].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        coff[16..18].copy_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(&coff);
        v.extend_from_slice(&[0u8; 16]);
        for s in sections {
            let mut h = [0u8; 40];
            h[..8].copy_from_slice(*s);
            v.extend_from_slice(&h);
        }
        v
    }

    fn check(bytes: &[u8]) -> bool {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ossteam-steamstub-{}-{n}", std::process::id()));
        std::fs::write(&dir, bytes).unwrap();
        let r = is_protected(&dir).unwrap();
        std::fs::remove_file(&dir).unwrap();
        r
    }

    #[test]
    fn finds_the_bind_section() {
        assert!(check(&pe(&[b".text\0\0\0", b".bind\0\0\0"])));
        assert!(!check(&pe(&[
            b".text\0\0\0",
            b".data\0\0\0",
            b".rsrc\0\0\0"
        ])));
        assert!(!check(&pe(&[b".binder\0"])));
    }

    #[test]
    fn other_files_are_not_protected() {
        assert!(!check(b"#!/bin/sh\nexec ./game\n"));
        assert!(!check(b"\x7fELF\x02\x01\x01"));
        assert!(!check(b"MZ"));
        let mut truncated = pe(&[b".text\0\0\0", b".bind\0\0\0"]);
        truncated.truncate(truncated.len() - 50);
        assert!(!check(&truncated));
    }
}
