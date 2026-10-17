// SPDX-License-Identifier: GPL-2.0-or-later
//! Depot manifests (`content.md` §4): the section format, encrypted names,
//! the checks a manifest must pass, and the stored form.

use std::fmt;

use base64::Engine as _;
use ossteam_proto::steam::{
    ContentManifestMetadata, ContentManifestPayload, ContentManifestSignature,
    content_manifest_payload::{FileMapping, file_mapping::ChunkData},
};
use ossteam_types::{DepotId, ManifestId};
use prost::Message as _;

use crate::key::{self, DepotKey};
use crate::path;

const PAYLOAD: u32 = 0x71F6_17D0;
const METADATA: u32 = 0x1F48_12BE;
const SIGNATURE: u32 = 0x1B81_B817;
const END: u32 = 0x32C4_15AB;

/// The most a manifest may hold once unzipped. The largest seen are tens
/// of MiB; this bounds memory if a server sends something else.
pub const MAX_SIZE: u64 = 512 << 20;

/// File flags (`content.md` §4).
pub mod flags {
    pub const USER_CONFIG: u32 = 0x001;
    pub const VERSIONED_USER_CONFIG: u32 = 0x002;
    pub const READ_ONLY: u32 = 0x008;
    pub const EXECUTABLE: u32 = 0x020;
    pub const DIRECTORY: u32 = 0x040;
    pub const INSTALL_SCRIPT: u32 = 0x100;
    pub const SYMLINK: u32 = 0x200;
}

/// A chunk id: the 20-byte hash the CDN names a chunk by.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ChunkId(pub [u8; 20]);

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: ChunkId,
    /// Steam's Adler-32 of the data (`chunk::adler32`).
    pub crc: u32,
    pub offset: u64,
    /// Size of the data.
    pub original: u32,
    /// Size as served, encrypted and compressed.
    pub compressed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The name as the manifest gives it, `\`-separated; decrypted once
    /// [`Manifest::decrypt_names`] has run.
    pub name: String,
    pub size: u64,
    pub flags: u32,
    /// SHA-1 of the whole file; empty or zero when not given.
    pub sha: Vec<u8>,
    /// Sorted by offset once the manifest is checked.
    pub chunks: Vec<Chunk>,
    pub link_target: Option<String>,
}

impl File {
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIRECTORY != 0
    }

    pub fn is_symlink(&self) -> bool {
        self.flags & flags::SYMLINK != 0
    }

    /// The name's components, checked (`content.md` §4 "Paths").
    pub fn components(&self) -> Result<Vec<&str>, path::PathError> {
        path::components(&self.name, cfg!(windows))
    }

    /// A key that is the same for two spellings of one path: components
    /// joined with `/`.
    pub fn key(&self) -> String {
        self.name.replace('\\', "/")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub depot: DepotId,
    pub id: ManifestId,
    pub created: u32,
    pub names_encrypted: bool,
    /// Total size of the files.
    pub original_size: u64,
    /// Total size of the unique chunks as served.
    pub compressed_size: u64,
    pub unique_chunks: u32,
    pub files: Vec<File>,
    /// Kept so the stored form carries it; not checked (`content.md` §4).
    signature: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("the manifest is cut short")]
    Truncated,
    #[error("the manifest has an unknown section {0:#010x}")]
    Section(u32),
    #[error("this is an older binary manifest, which ossteam does not read")]
    OldFormat,
    #[error("the manifest has no {0} section")]
    Missing(&'static str),
    #[error("the {0} section does not decode: {1}")]
    Decode(&'static str, prost::DecodeError),
    #[error("the manifest is for depot {got_depot} manifest {got_id}, not {want_depot} {want_id}")]
    Wrong {
        got_depot: DepotId,
        got_id: ManifestId,
        want_depot: DepotId,
        want_id: ManifestId,
    },
    #[error("the manifest's zip wrapping: {0}")]
    Zip(#[from] crate::zip::ZipError),
    #[error("a name does not decrypt: {0}")]
    Name(String),
    #[error(transparent)]
    Path(#[from] path::PathError),
    #[error("{0:?}: {1}")]
    Layout(String, &'static str),
    #[error("{0:?} is named twice")]
    Duplicate(String),
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// The manifest's sections, before any interpretation.
fn sections(
    data: &[u8],
) -> Result<
    (
        ContentManifestPayload,
        ContentManifestMetadata,
        ContentManifestSignature,
    ),
    ManifestError,
> {
    let (mut payload, mut metadata, mut signature) = (None, None, None);
    let mut at = 0usize;
    loop {
        let magic = u32_at(data, at).ok_or(ManifestError::Truncated)?;
        at += 4;
        match magic {
            END => break,
            PAYLOAD | METADATA | SIGNATURE => {}
            // Version 4's magic; its layout is not described here.
            0x16349781 => return Err(ManifestError::OldFormat),
            m => return Err(ManifestError::Section(m)),
        }
        let len = u32_at(data, at).ok_or(ManifestError::Truncated)? as usize;
        at += 4;
        let body = at
            .checked_add(len)
            .and_then(|end| data.get(at..end))
            .ok_or(ManifestError::Truncated)?;
        at += len;
        match magic {
            PAYLOAD => {
                payload = Some(
                    ContentManifestPayload::decode(body)
                        .map_err(|e| ManifestError::Decode("payload", e))?,
                )
            }
            METADATA => {
                metadata = Some(
                    ContentManifestMetadata::decode(body)
                        .map_err(|e| ManifestError::Decode("metadata", e))?,
                )
            }
            SIGNATURE => {
                signature = Some(
                    ContentManifestSignature::decode(body)
                        .map_err(|e| ManifestError::Decode("signature", e))?,
                )
            }
            _ => unreachable!("checked above"),
        }
    }
    Ok((
        payload.ok_or(ManifestError::Missing("payload"))?,
        metadata.ok_or(ManifestError::Missing("metadata"))?,
        signature.ok_or(ManifestError::Missing("signature"))?,
    ))
}

impl Manifest {
    /// Reads a manifest as the CDN serves it: a zip with one entry.
    pub fn from_zip(archive: &[u8]) -> Result<Self, ManifestError> {
        let data = crate::zip::extract_single(archive, MAX_SIZE)?;
        Self::parse(&data)
    }

    /// Reads the section format, as unzipped or as stored.
    pub fn parse(data: &[u8]) -> Result<Self, ManifestError> {
        let (payload, metadata, signature) = sections(data)?;
        let files = payload
            .mappings
            .into_iter()
            .map(|m| File {
                name: m.filename.unwrap_or_default(),
                size: m.size.unwrap_or(0),
                flags: m.flags.unwrap_or(0),
                sha: m.sha_content.unwrap_or_default(),
                chunks: m
                    .chunks
                    .into_iter()
                    .map(|c| Chunk {
                        id: ChunkId(
                            c.sha
                                .as_deref()
                                .and_then(|s| s.try_into().ok())
                                .unwrap_or_default(),
                        ),
                        crc: c.crc.unwrap_or(0),
                        offset: c.offset.unwrap_or(0),
                        original: c.cb_original.unwrap_or(0),
                        compressed: c.cb_compressed.unwrap_or(0),
                    })
                    .collect(),
                link_target: m.linktarget.filter(|t| !t.is_empty()),
            })
            .collect();
        Ok(Self {
            depot: DepotId(metadata.depot_id()),
            id: ManifestId(metadata.gid_manifest()),
            created: metadata.creation_time(),
            names_encrypted: metadata.filenames_encrypted(),
            original_size: metadata.cb_disk_original(),
            compressed_size: metadata.cb_disk_compressed(),
            unique_chunks: metadata.unique_chunks(),
            files,
            signature: signature.signature.unwrap_or_default(),
        })
    }

    /// Fails unless this is the manifest that was asked for.
    pub fn expect(&self, depot: DepotId, id: ManifestId) -> Result<(), ManifestError> {
        if self.depot != depot || self.id != id {
            return Err(ManifestError::Wrong {
                got_depot: self.depot,
                got_id: self.id,
                want_depot: depot,
                want_id: id,
            });
        }
        Ok(())
    }

    /// Decrypts names and link targets in place, if they are encrypted.
    pub fn decrypt_names(&mut self, key: &DepotKey) -> Result<(), ManifestError> {
        if !self.names_encrypted {
            return Ok(());
        }
        let decrypt = |text: &str| -> Result<String, ManifestError> {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map_err(|e| ManifestError::Name(format!("{text:?} is not base64: {e}")))?;
            let mut plain = key::decrypt(key, &raw)
                .map_err(|e| ManifestError::Name(format!("{text:?}: {e}")))?;
            while plain.last() == Some(&0) {
                plain.pop();
            }
            String::from_utf8(plain)
                .map_err(|_| ManifestError::Name(format!("{text:?} is not UTF-8 once decrypted")))
        };
        for f in &mut self.files {
            f.name = decrypt(&f.name)?;
            if let Some(t) = &f.link_target {
                f.link_target = Some(decrypt(t)?);
            }
        }
        self.names_encrypted = false;
        Ok(())
    }

    /// Checks every name and every file's chunk layout (`content.md` §4),
    /// and sorts each file's chunks by offset. Names must be decrypted.
    pub fn check(&mut self) -> Result<(), ManifestError> {
        if self.names_encrypted {
            return Err(ManifestError::Name("the names are still encrypted".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &mut self.files {
            f.components()?;
            if !seen.insert(f.key()) {
                return Err(ManifestError::Duplicate(f.name.clone()));
            }
            let bad = |why| ManifestError::Layout(f.name.clone(), why);
            if f.is_dir() && (!f.chunks.is_empty() || f.size != 0) {
                return Err(bad("a directory has content"));
            }
            if f.is_symlink() && f.link_target.is_none() {
                return Err(bad("a symbolic link has no target"));
            }
            f.chunks.sort_by_key(|c| c.offset);
            let mut end = 0u64;
            for c in &f.chunks {
                if c.offset < end {
                    return Err(bad("chunks overlap"));
                }
                if c.original == 0 || c.compressed == 0 {
                    return Err(bad("a chunk is empty"));
                }
                end = c
                    .offset
                    .checked_add(u64::from(c.original))
                    .ok_or(bad("a chunk ends past 2^64"))?;
            }
            if end > f.size {
                return Err(bad("a chunk ends past the end of the file"));
            }
        }
        Ok(())
    }

    /// The stored form (`content.md` §4): the same sections, with names
    /// as they are now (decrypted, once [`Self::decrypt_names`] ran).
    pub fn encode(&self) -> Vec<u8> {
        let payload = ContentManifestPayload {
            mappings: self
                .files
                .iter()
                .map(|f| FileMapping {
                    filename: Some(f.name.clone()),
                    size: Some(f.size),
                    flags: Some(f.flags),
                    sha_filename: None,
                    sha_content: Some(f.sha.clone()),
                    chunks: f
                        .chunks
                        .iter()
                        .map(|c| ChunkData {
                            sha: Some(c.id.0.to_vec()),
                            crc: Some(c.crc),
                            offset: Some(c.offset),
                            cb_original: Some(c.original),
                            cb_compressed: Some(c.compressed),
                        })
                        .collect(),
                    linktarget: f.link_target.clone(),
                })
                .collect(),
        };
        let metadata = ContentManifestMetadata {
            depot_id: Some(self.depot.0),
            gid_manifest: Some(self.id.0),
            creation_time: Some(self.created),
            filenames_encrypted: Some(self.names_encrypted),
            cb_disk_original: Some(self.original_size),
            cb_disk_compressed: Some(self.compressed_size),
            unique_chunks: Some(self.unique_chunks),
            crc_encrypted: None,
            crc_clear: None,
        };
        let signature = ContentManifestSignature {
            signature: Some(self.signature.clone()),
        };
        let mut out = Vec::new();
        for (magic, body) in [
            (PAYLOAD, payload.encode_to_vec()),
            (METADATA, metadata.encode_to_vec()),
            (SIGNATURE, signature.encode_to_vec()),
        ] {
            out.extend(magic.to_le_bytes());
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
        }
        out.extend(END.to_le_bytes());
        out
    }

    /// Builds a manifest from parts, for test fixtures and the fake CDN.
    #[cfg(any(test, feature = "testing"))]
    pub fn new(depot: DepotId, id: ManifestId, files: Vec<File>) -> Self {
        let mut unique = std::collections::HashSet::new();
        let mut compressed_size = 0;
        for c in files.iter().flat_map(|f| &f.chunks) {
            if unique.insert(c.id) {
                compressed_size += u64::from(c.compressed);
            }
        }
        Self {
            depot,
            id,
            created: 1_700_000_000,
            names_encrypted: false,
            original_size: files.iter().map(|f| f.size).sum(),
            compressed_size,
            unique_chunks: unique.len() as u32,
            files,
            signature: vec![0; 128],
        }
    }

    /// Encrypts every name and link target under `key`, as a depot with
    /// encrypted names would be served.
    #[cfg(any(test, feature = "testing"))]
    pub fn encrypt_names(&mut self, key: &DepotKey) {
        let enc = |s: &str| {
            let mut plain = s.as_bytes().to_vec();
            plain.push(0);
            base64::engine::general_purpose::STANDARD.encode(key::encrypt(key, [9; 16], &plain))
        };
        for f in &mut self.files {
            f.name = enc(&f.name);
            f.link_target = f.link_target.as_deref().map(enc);
        }
        self.names_encrypted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::build;

    fn key() -> DepotKey {
        DepotKey::new(&[0x33; 32]).unwrap()
    }

    fn file(name: &str, data: &[u8]) -> File {
        let (chunk, _) = build::served(data, 0, &key(), build::vzstd);
        File {
            name: name.into(),
            size: data.len() as u64,
            flags: 0,
            sha: vec![1; 20],
            chunks: vec![chunk],
            link_target: None,
        }
    }

    fn sample() -> Manifest {
        let mut link = file("bin\\link", b"");
        link.chunks.clear();
        link.flags = flags::SYMLINK;
        link.link_target = Some("game".into());
        let mut dir = file("bin", b"");
        dir.chunks.clear();
        dir.flags = flags::DIRECTORY;
        Manifest::new(
            DepotId(481),
            ManifestId(7_000_000_000_000_000_001),
            vec![dir, file("bin\\game", b"#!/bin/sh\n"), link],
        )
    }

    #[test]
    fn round_trips_through_zip_and_encrypted_names() {
        let mut m = sample();
        m.encrypt_names(&key());
        assert_ne!(m.files[1].name, "bin\\game");
        let zipped = crate::zip::write_single("z", &m.encode());

        let mut got = Manifest::from_zip(&zipped).unwrap();
        got.expect(DepotId(481), ManifestId(7_000_000_000_000_000_001))
            .unwrap();
        assert!(got.check().is_err(), "names are still encrypted");
        got.decrypt_names(&key()).unwrap();
        got.check().unwrap();
        assert_eq!(got, sample());
        assert_eq!(got.files[2].link_target.as_deref(), Some("game"));
    }

    #[test]
    fn the_wrong_manifest_is_refused() {
        let m = Manifest::parse(&sample().encode()).unwrap();
        assert!(m.expect(DepotId(481), ManifestId(1)).is_err());
        assert!(m.expect(DepotId(482), m.id).is_err());
    }

    #[test]
    fn layout_errors_are_caught() {
        let mut m = sample();
        m.files[1].size = 3;
        assert!(matches!(m.check(), Err(ManifestError::Layout(..))));

        let mut m = sample();
        let c = m.files[1].chunks[0].clone();
        m.files[1].chunks.push(c);
        m.files[1].size *= 2;
        assert!(matches!(
            m.check(),
            Err(ManifestError::Layout(_, "chunks overlap"))
        ));

        let mut m = sample();
        m.files[1].name = "..\\evil".into();
        assert!(matches!(m.check(), Err(ManifestError::Path(_))));

        let mut m = sample();
        m.files[0].name = "bin/game".into();
        assert!(matches!(m.check(), Err(ManifestError::Duplicate(_))));
    }

    #[test]
    fn sections_must_all_be_present_and_known() {
        let data = sample().encode();
        assert_eq!(Manifest::parse(&data[..10]), Err(ManifestError::Truncated));
        assert_eq!(
            Manifest::parse(&END.to_le_bytes()),
            Err(ManifestError::Missing("payload"))
        );
        assert_eq!(
            Manifest::parse(&0x1234_5678u32.to_le_bytes()),
            Err(ManifestError::Section(0x1234_5678))
        );
        assert_eq!(
            Manifest::parse(&0x16349781u32.to_le_bytes()),
            Err(ManifestError::OldFormat)
        );
    }

    #[test]
    fn chunk_ids_print_as_hex() {
        let mut id = ChunkId::default();
        id.0[0] = 0xab;
        id.0[19] = 0x01;
        assert_eq!(id.to_string(), format!("ab{}01", "00".repeat(18)));
    }
}
