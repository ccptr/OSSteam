// SPDX-License-Identifier: GPL-2.0-or-later
//! A content server in memory, for tests of this crate and of the daemon
//! (FAITH-4, as ADR 0002 does for the CM). It serves the URLs of
//! `content.md` §4 and §5 from depots built with [`Depot`], and can be
//! told to fail.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ossteam_types::{CancelToken, DepotId, ManifestId};
use sha1::{Digest, Sha1};

use crate::cdn::{Fetch, FetchError};
use crate::chunk::build;
use crate::download::Tokens;
use crate::key::DepotKey;
use crate::manifest::{Chunk, File, Manifest};

/// The largest chunk [`Depot`] cuts files into; small, so tests cover
/// files of several chunks without large data.
pub const CHUNK: usize = 4096;

#[derive(Default)]
struct State {
    /// Bodies by path, query strings removed.
    objects: HashMap<String, Vec<u8>>,
    /// Requests for these paths fail with this status this many more times.
    failures: HashMap<String, (u16, usize)>,
    /// Hosts that answer 403 unless the query is `token=<value>`.
    token_hosts: HashMap<String, String>,
    /// Bodies served as given instead of the stored one, once each.
    corrupt: HashMap<String, usize>,
    /// Chunk requests after this many wait until [`FakeCdn::release`].
    hold_after: Option<usize>,
    /// How long each chunk request takes.
    pace: Option<std::time::Duration>,
}

#[derive(Default)]
pub struct FakeCdn {
    state: Mutex<State>,
    requests: AtomicUsize,
    chunk_requests: AtomicUsize,
    tokens_issued: AtomicUsize,
}

impl FakeCdn {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Serves `body` at `path`.
    pub fn put(&self, path: String, body: Vec<u8>) {
        self.state().objects.insert(path, body);
    }

    /// The next `times` requests for `path` fail with `status`.
    pub fn fail(&self, path: &str, status: u16, times: usize) {
        self.state()
            .failures
            .insert(path.to_owned(), (status, times));
    }

    /// The next `times` responses for `path` have a byte flipped.
    pub fn corrupt(&self, path: &str, times: usize) {
        self.state().corrupt.insert(path.to_owned(), times);
    }

    /// `host` requires the CDN auth token `token` (`content.md` §7).
    pub fn require_token(&self, host: &str, token: &str) {
        self.state()
            .token_hosts
            .insert(host.to_owned(), token.to_owned());
    }

    /// Chunk requests after the next `n` wait, until [`FakeCdn::release`]
    /// or until they are cancelled: a job caught mid-download.
    pub fn hold_after(&self, n: usize) {
        self.state().hold_after = Some(self.chunk_requests() + n);
    }

    pub fn release(&self) {
        self.state().hold_after = None;
    }

    /// Each chunk request takes `per_chunk`, or until it is cancelled: a
    /// download slow enough to watch.
    pub fn pace(&self, per_chunk: std::time::Duration) {
        self.state().pace = Some(per_chunk);
    }

    /// Requests served so far, failed ones included.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn tokens_issued(&self) -> usize {
        self.tokens_issued.load(Ordering::Relaxed)
    }

    /// Requests for chunks so far, failed ones included.
    pub fn chunk_requests(&self) -> usize {
        self.chunk_requests.load(Ordering::Relaxed)
    }

    /// Serves a depot's manifest under request code `code`, and its chunks.
    pub fn publish(&self, depot: &Depot, code: u64) {
        let mut m = depot.manifest.clone();
        if depot.encrypt_names {
            m.encrypt_names(&depot.key);
        }
        self.put(
            crate::cdn::manifest_path(depot.manifest.depot, depot.manifest.id, code),
            crate::zip::write_single("z", &m.encode()),
        );
        for (chunk, body) in &depot.served {
            self.put(
                crate::cdn::chunk_path(depot.manifest.depot, chunk, None),
                body.clone(),
            );
        }
    }
}

impl Fetch for FakeCdn {
    fn get(
        &self,
        vhost: &str,
        path: &str,
        limit: u64,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, FetchError> {
        if cancel.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        let (bare, query) = path.split_once('?').unwrap_or((path, ""));
        if bare.contains("/chunk/") {
            let n = self.chunk_requests.fetch_add(1, Ordering::Relaxed);
            while self.state().hold_after.is_some_and(|after| n >= after) {
                if cancel.is_cancelled() {
                    return Err(FetchError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let pace = self.state().pace;
            if let Some(pace) = pace {
                let until = std::time::Instant::now() + pace;
                while std::time::Instant::now() < until {
                    if cancel.is_cancelled() {
                        return Err(FetchError::Cancelled);
                    }
                    std::thread::sleep(pace.min(std::time::Duration::from_millis(5)));
                }
            }
        }
        let mut s = self.state();
        if let Some(token) = s.token_hosts.get(vhost)
            && query != format!("token={token}")
        {
            return Err(FetchError::Status(403));
        }
        if let Some((status, left)) = s.failures.get_mut(bare)
            && *left > 0
        {
            *left -= 1;
            return Err(FetchError::Status(*status));
        }
        let mut body = s
            .objects
            .get(bare)
            .cloned()
            .ok_or(FetchError::Status(404))?;
        if let Some(left) = s.corrupt.get_mut(bare)
            && *left > 0
        {
            *left -= 1;
            let mid = body.len() / 2;
            body[mid] ^= 0x40;
        }
        if body.len() as u64 > limit {
            return Err(FetchError::TooLarge(limit));
        }
        Ok(body)
    }
}

impl Tokens for FakeCdn {
    fn token(
        &self,
        _depot: DepotId,
        host: &str,
        _cancel: &CancelToken,
    ) -> Result<(String, u64), String> {
        self.tokens_issued.fetch_add(1, Ordering::Relaxed);
        let s = self.state();
        let token = s.token_hosts.get(host).cloned().unwrap_or_default();
        Ok((format!("token={token}"), u64::MAX))
    }
}

/// A chunk compressor of [`build`].
type Compress = fn(&[u8]) -> Vec<u8>;

/// A depot built from files in memory: its manifest, key and chunks as
/// served.
#[derive(Clone)]
pub struct Depot {
    pub key: DepotKey,
    pub manifest: Manifest,
    pub served: Vec<(Chunk, Vec<u8>)>,
    pub encrypt_names: bool,
}

/// One entry of a depot being built.
pub enum Entry<'a> {
    File {
        name: &'a str,
        data: &'a [u8],
        flags: u32,
    },
    Dir(&'a str),
    Link {
        name: &'a str,
        target: &'a str,
    },
}

impl Depot {
    /// Cuts each file into chunks of up to [`CHUNK`] bytes and compresses
    /// them in turn with each of the three forms.
    pub fn build(depot: DepotId, id: ManifestId, key: DepotKey, entries: &[Entry<'_>]) -> Self {
        let forms: [Compress; 3] = [build::vzstd, build::vzip, build::zip];
        let mut served: Vec<(Chunk, Vec<u8>)> = Vec::new();
        let mut files = Vec::new();
        let mut n = 0usize;
        for e in entries {
            let file = match *e {
                Entry::File { name, data, flags } => {
                    let mut chunks = Vec::new();
                    for (i, piece) in data.chunks(CHUNK).enumerate() {
                        let (c, body) =
                            build::served(piece, (i * CHUNK) as u64, &key, forms[n % 3]);
                        n += 1;
                        if !served.iter().any(|(s, _)| s.id == c.id) {
                            served.push((c.clone(), body));
                        }
                        // The same data at another offset is the same object.
                        let compressed = served
                            .iter()
                            .find(|(s, _)| s.id == c.id)
                            .map(|(s, _)| s.compressed);
                        chunks.push(Chunk {
                            compressed: compressed.unwrap_or(c.compressed),
                            ..c
                        });
                    }
                    let sha = if data.is_empty() {
                        vec![0; 20]
                    } else {
                        Sha1::digest(data).to_vec()
                    };
                    File {
                        name: name.to_owned(),
                        size: data.len() as u64,
                        flags,
                        sha,
                        chunks,
                        link_target: None,
                    }
                }
                Entry::Dir(name) => File {
                    name: name.to_owned(),
                    size: 0,
                    flags: crate::manifest::flags::DIRECTORY,
                    sha: vec![0; 20],
                    chunks: Vec::new(),
                    link_target: None,
                },
                Entry::Link { name, target } => File {
                    name: name.to_owned(),
                    size: 0,
                    flags: crate::manifest::flags::SYMLINK,
                    sha: vec![0; 20],
                    chunks: Vec::new(),
                    link_target: Some(target.to_owned()),
                },
            };
            files.push(file);
        }
        Self {
            key,
            manifest: Manifest::new(depot, id, files),
            served,
            encrypt_names: false,
        }
    }
}
