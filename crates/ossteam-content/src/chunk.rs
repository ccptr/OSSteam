// SPDX-License-Identifier: GPL-2.0-or-later
//! Chunks as the CDN serves them: decryption, the three compressed forms
//! and the checks a chunk must pass before it is written (`content.md` §5,
//! CONT-3).

use sha1::{Digest, Sha1};

use crate::key::{self, DecryptError, DepotKey};
use crate::manifest::Chunk;
use crate::zip::{self, ZipError};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("the chunk is {got} bytes as downloaded; the manifest says {want}")]
    PackedSize { got: usize, want: u32 },
    #[error("the chunk does not decrypt: {0}")]
    Decrypt(#[from] DecryptError),
    #[error("unknown compression; the chunk starts with {0:02x?}")]
    Format(Vec<u8>),
    #[error("the {0} frame is malformed")]
    Frame(&'static str),
    #[error("the chunk does not decompress: {0}")]
    Decompress(String),
    #[error("the chunk's zip wrapping: {0}")]
    Zip(#[from] ZipError),
    #[error("the chunk is {got} bytes once decompressed; the manifest says {want}")]
    Size { got: usize, want: u32 },
    #[error("the chunk's checksum does not match")]
    Checksum,
    #[error("the chunk's SHA-1 is not its id")]
    Sha,
}

/// Steam's Adler-32: the usual algorithm, but with both sums starting at
/// 0 instead of `a = 1` (`content.md` §5).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    // 5552 is the most bytes that can be summed before `b` may overflow.
    let (mut a, mut b) = (0u32, 0u32);
    for block in data.chunks(5552) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// Checks decompressed data against its manifest entry (`content.md` §5,
/// checks 1 to 3).
pub fn check(chunk: &Chunk, data: &[u8]) -> Result<(), ChunkError> {
    check_quick(chunk, data)?;
    if Sha1::digest(data).as_slice() != chunk.id.0 {
        return Err(ChunkError::Sha);
    }
    Ok(())
}

/// Checks 1 and 2 only: the length and the Adler-32 sum. Used for data
/// already on disk, which is re-read in bulk (`content.md` §9 to §11).
pub fn check_quick(chunk: &Chunk, data: &[u8]) -> Result<(), ChunkError> {
    if data.len() != chunk.original as usize {
        return Err(ChunkError::Size {
            got: data.len(),
            want: chunk.original,
        });
    }
    if adler32(data) != chunk.crc {
        return Err(ChunkError::Checksum);
    }
    Ok(())
}

/// Turns a chunk as downloaded into its file data, checked.
pub fn process(chunk: &Chunk, downloaded: &[u8], key: &DepotKey) -> Result<Vec<u8>, ChunkError> {
    if downloaded.len() != chunk.compressed as usize {
        return Err(ChunkError::PackedSize {
            got: downloaded.len(),
            want: chunk.compressed,
        });
    }
    let plain = key::decrypt(key, downloaded)?;
    let data = decompress(&plain, chunk.original)?;
    check(chunk, &data)?;
    Ok(data)
}

/// Decompresses whichever form `plain` is in. `size` is the expected size,
/// which bounds every allocation.
pub fn decompress(plain: &[u8], size: u32) -> Result<Vec<u8>, ChunkError> {
    if plain.starts_with(b"VSZa") {
        vzstd(plain, size)
    } else if plain.starts_with(b"VZa") {
        vzip(plain, size)
    } else if plain.starts_with(b"PK\x03\x04") {
        Ok(zip::extract_single(plain, u64::from(size))?)
    } else {
        Err(ChunkError::Format(plain[..plain.len().min(4)].to_vec()))
    }
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn declared_size(footer_size: u32, want: u32) -> Result<(), ChunkError> {
    if footer_size != want {
        return Err(ChunkError::Size {
            got: footer_size as usize,
            want,
        });
    }
    Ok(())
}

/// `content.md` §5.1.
fn vzip(plain: &[u8], size: u32) -> Result<Vec<u8>, ChunkError> {
    const HEADER: usize = 7;
    const PROPS: usize = 5;
    const FOOTER: usize = 10;
    if plain.len() < HEADER + PROPS + FOOTER || !plain.ends_with(b"zv") {
        return Err(ChunkError::Frame("VZip"));
    }
    let footer = &plain[plain.len() - FOOTER..];
    declared_size(le32(&footer[4..]), size)?;
    let stream = &plain[HEADER..plain.len() - FOOTER];
    let mut out = Vec::with_capacity(size as usize);
    let options = lzma_rs::decompress::Options {
        unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(u64::from(size))),
        // The output is at most `size`, so no dictionary needs to be larger.
        memlimit: Some((size as usize).max(4096)),
        allow_incomplete: false,
    };
    lzma_rs::lzma_decompress_with_options(&mut &stream[..], &mut out, &options)
        .map_err(|e| ChunkError::Decompress(e.to_string()))?;
    Ok(out)
}

/// `content.md` §5.2.
fn vzstd(plain: &[u8], size: u32) -> Result<Vec<u8>, ChunkError> {
    const HEADER: usize = 8;
    const FOOTER: usize = 15;
    if plain.len() < HEADER + FOOTER || !plain.ends_with(b"zsv") {
        return Err(ChunkError::Frame("VZstd"));
    }
    let footer = &plain[plain.len() - FOOTER..];
    declared_size(le32(&footer[4..]), size)?;
    let frame = &plain[HEADER..plain.len() - FOOTER];
    zstd::bulk::decompress(frame, size as usize).map_err(|e| ChunkError::Decompress(e.to_string()))
}

/// Builders for test fixtures and the fake CDN: each wraps `data` in one
/// of the forms above, then encrypts it as the CDN would serve it.
#[cfg(any(test, feature = "testing"))]
pub mod build {
    use super::*;

    pub fn vzstd(data: &[u8]) -> Vec<u8> {
        let crc = crc32fast::hash(data);
        let frame = zstd::bulk::compress(data, 3).expect("compressing in memory");
        let mut out = b"VSZa".to_vec();
        out.extend(crc.to_le_bytes());
        out.extend(frame);
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend([0; 4]);
        out.extend(b"zsv");
        out
    }

    pub fn vzip(data: &[u8]) -> Vec<u8> {
        let crc = crc32fast::hash(data);
        // lzma-rs writes the 13-byte `.lzma` header: 5 bytes of properties,
        // then an 8-byte size that VZip does not carry.
        let mut lzma = Vec::new();
        lzma_rs::lzma_compress(&mut &data[..], &mut lzma).expect("compressing in memory");
        let mut out = b"VZa".to_vec();
        out.extend(crc.to_le_bytes());
        out.extend(&lzma[..5]);
        out.extend(&lzma[13..]);
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(b"zv");
        out
    }

    pub fn zip(data: &[u8]) -> Vec<u8> {
        crate::zip::write_single("z", data)
    }

    /// The chunk's manifest entry and its bytes as served, compressed with
    /// `form` and encrypted under `key`.
    pub fn served(
        data: &[u8],
        offset: u64,
        key: &DepotKey,
        form: fn(&[u8]) -> Vec<u8>,
    ) -> (Chunk, Vec<u8>) {
        let packed = crate::key::encrypt(key, [0x5a; 16], &form(data));
        let chunk = Chunk {
            id: crate::manifest::ChunkId(Sha1::digest(data).into()),
            crc: adler32(data),
            offset,
            original: data.len() as u32,
            compressed: packed.len() as u32,
        };
        (chunk, packed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> DepotKey {
        DepotKey::new(&[0x11; 32]).unwrap()
    }

    fn sample() -> Vec<u8> {
        (0..70_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect()
    }

    #[test]
    fn adler_starts_at_zero() {
        assert_eq!(adler32(b""), 0);
        // The standard Adler-32 of "a" is 0x00620062; seeded with 0, both
        // sums are one `a` short.
        assert_eq!(adler32(b"a"), 0x0061_0061);
        // Long input exercises the deferred modulo.
        let long = vec![0xffu8; 100_000];
        let (mut a, mut b) = (0u64, 0u64);
        for &x in &long {
            a = (a + u64::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&long), ((b << 16) | a) as u32);
    }

    #[test]
    fn every_form_round_trips() {
        let data = sample();
        for form in [build::vzstd, build::vzip, build::zip] {
            let (chunk, served) = build::served(&data, 0, &key(), form);
            assert_eq!(process(&chunk, &served, &key()).unwrap(), data);
        }
    }

    #[test]
    fn checks_catch_corruption_and_the_wrong_key() {
        let data = sample();
        let (chunk, served) = build::served(&data, 0, &key(), build::vzstd);

        let mut bad = chunk.clone();
        bad.crc ^= 1;
        assert_eq!(process(&bad, &served, &key()), Err(ChunkError::Checksum));

        let mut bad = chunk.clone();
        bad.id.0[0] ^= 1;
        assert_eq!(process(&bad, &served, &key()), Err(ChunkError::Sha));

        let mut bad = chunk.clone();
        bad.original += 1;
        assert!(matches!(
            process(&bad, &served, &key()),
            Err(ChunkError::Size { .. })
        ));

        assert!(matches!(
            process(&chunk, &served[1..], &key()),
            Err(ChunkError::PackedSize { .. })
        ));

        let other = DepotKey::new(&[0x22; 32]).unwrap();
        assert!(process(&chunk, &served, &other).is_err());
    }

    #[test]
    fn unknown_and_short_frames_are_refused() {
        assert_eq!(
            decompress(b"XYZW1234", 4),
            Err(ChunkError::Format(b"XYZW".to_vec()))
        );
        assert_eq!(decompress(b"VZa", 4), Err(ChunkError::Frame("VZip")));
        assert_eq!(decompress(b"VSZa", 4), Err(ChunkError::Frame("VZstd")));
        assert_eq!(decompress(b"", 4), Err(ChunkError::Format(Vec::new())));
    }
}
