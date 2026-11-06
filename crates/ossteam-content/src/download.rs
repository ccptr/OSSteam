// SPDX-License-Identifier: GPL-2.0-or-later
//! Fetching manifests and chunks with retries across the server rotation,
//! and CDN auth tokens (`content.md` §4, §5, §7).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ossteam_types::{CancelToken, DepotId, ManifestId};

use crate::cdn::{self, Fetch, FetchError, Rotation, Server};
use crate::chunk;
use crate::key::DepotKey;
use crate::manifest::{Chunk, Manifest};

/// Attempts per chunk or manifest before the job fails (`content.md` §7).
pub const ATTEMPTS: usize = 5;

/// Asks Steam for a CDN auth token (`content.md` §7). The daemon answers
/// over CM; tests answer from the fake CDN.
pub trait Tokens: Send + Sync {
    /// The token for `depot` on `host`, and the Unix time it expires.
    fn token(
        &self,
        depot: DepotId,
        host: &str,
        cancel: &CancelToken,
    ) -> Result<(String, u64), String>;
}

/// Sleeps for `d`, checking `cancel` at least once a second (CONC-2).
fn wait(d: Duration, cancel: &CancelToken) {
    const SLICE: Duration = Duration::from_millis(250);
    let end = Instant::now() + d;
    while !cancel.is_cancelled() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(SLICE));
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DownloadError {
    #[error("{what} failed {ATTEMPTS} times; the last error: {last}")]
    Exhausted { what: String, last: String },
    #[error("cancelled")]
    Cancelled,
}

/// What one job downloads through: the transport, the job's server
/// rotation, and the tokens it has been given.
pub struct Downloader {
    fetch: Arc<dyn Fetch>,
    rotation: Rotation,
    tokens: Option<Arc<dyn Tokens>>,
    /// Tokens by depot and host, with their expiry.
    granted: Mutex<HashMap<(DepotId, String), (String, u64)>>,
    /// Held while asking for a token, so threads that all get a 403 ask
    /// once.
    asking: Mutex<()>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Downloader {
    pub fn new(fetch: Arc<dyn Fetch>, rotation: Rotation, tokens: Option<Arc<dyn Tokens>>) -> Self {
        Self {
            fetch,
            rotation,
            tokens,
            granted: Mutex::new(HashMap::new()),
            asking: Mutex::new(()),
        }
    }

    /// A downloader that fails every request, for jobs that only delete.
    pub fn offline() -> Self {
        struct Offline;
        impl Fetch for Offline {
            fn get(
                &self,
                _: &str,
                _: &str,
                _: u64,
                _: &CancelToken,
            ) -> Result<Vec<u8>, FetchError> {
                Err(FetchError::Transport("this job downloads nothing".into()))
            }
        }
        let none = Server {
            kind: String::new(),
            host: String::new(),
            vhost: String::new(),
        };
        Self::new(
            Arc::new(Offline),
            Rotation::new(vec![none]).expect("one server"),
            None,
        )
    }

    fn granted(&self, depot: DepotId, server: &Server) -> Option<String> {
        let g = self.granted.lock().unwrap_or_else(|e| e.into_inner());
        g.get(&(depot, server.host.clone()))
            .filter(|(_, until)| *until > now())
            .map(|(t, _)| t.clone())
    }

    /// One GET with the depot's token for that server, asking for a token
    /// first if the server answers 403 and none is held.
    fn get(
        &self,
        depot: DepotId,
        server: &Server,
        path: &dyn Fn(Option<&str>) -> String,
        limit: u64,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, FetchError> {
        let held = self.granted(depot, server);
        match self
            .fetch
            .get(&server.vhost, &path(held.as_deref()), limit, cancel)
        {
            Err(FetchError::Status(403)) if held.is_none() => {
                let Some(tokens) = &self.tokens else {
                    return Err(FetchError::Status(403));
                };
                let _asking = self.asking.lock().unwrap_or_else(|e| e.into_inner());
                let token = match self.granted(depot, server) {
                    Some(t) => t,
                    None => {
                        let (token, until) =
                            tokens.token(depot, &server.host, cancel).map_err(|e| {
                                FetchError::Transport(format!(
                                    "HTTP 403, and no CDN auth token: {e}"
                                ))
                            })?;
                        self.granted
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert((depot, server.host.clone()), (token.clone(), until));
                        token
                    }
                };
                self.fetch
                    .get(&server.vhost, &path(Some(&token)), limit, cancel)
            }
            other => other,
        }
    }

    /// Tries servers in turn until `accept` takes a body.
    fn attempt<T, E: std::fmt::Display>(
        &self,
        what: &dyn Fn() -> String,
        depot: DepotId,
        path: &dyn Fn(Option<&str>) -> String,
        limit: u64,
        cancel: &CancelToken,
        accept: impl Fn(Vec<u8>) -> Result<T, E>,
    ) -> Result<T, DownloadError> {
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0
                && let Some(rest) = self.rotation.rest()
            {
                // Every server failed lately: wait for one, not at once.
                tracing::debug!(?rest, "every content server failed; waiting");
                wait(rest, cancel);
            }
            if cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let server = self.rotation.pick();
            let outcome = match self.get(depot, &server, path, limit, cancel) {
                Ok(body) => accept(body).map_err(|e| e.to_string()),
                Err(FetchError::Cancelled) => return Err(DownloadError::Cancelled),
                Err(e) => Err(e.to_string()),
            };
            match outcome {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::debug!(server = %server.vhost, error = %e, "content request failed");
                    self.rotation.failed(&server);
                    last = format!("{}: {e}", server.vhost);
                }
            }
        }
        Err(DownloadError::Exhausted { what: what(), last })
    }

    /// Whether every server of the rotation is penalised, so a job that
    /// failed should fetch the server list again (`content.md` §1).
    pub fn exhausted(&self) -> bool {
        self.rotation.exhausted()
    }

    /// A chunk's file data, decrypted and checked (`content.md` §5).
    pub fn chunk(
        &self,
        depot: DepotId,
        c: &Chunk,
        key: &DepotKey,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, DownloadError> {
        self.attempt(
            &|| format!("chunk {} of depot {depot}", c.id),
            depot,
            &|token| cdn::chunk_path(depot, c, token),
            u64::from(c.compressed),
            cancel,
            |body| chunk::process(c, &body, key),
        )
    }

    /// A manifest as served, parsed and checked to be the one asked for
    /// (`content.md` §4). Names are still as served.
    pub fn manifest(
        &self,
        depot: DepotId,
        id: ManifestId,
        code: u64,
        cancel: &CancelToken,
    ) -> Result<Manifest, DownloadError> {
        self.attempt(
            &|| format!("manifest {id} of depot {depot}"),
            depot,
            &|token| {
                let mut p = cdn::manifest_path(depot, id, code);
                if let Some(t) = token.filter(|t| !t.is_empty()) {
                    p.push('?');
                    p.push_str(t.trim_start_matches('?'));
                }
                p
            },
            cdn::MANIFEST_LIMIT,
            cancel,
            |body| {
                let m = Manifest::from_zip(&body)?;
                m.expect(depot, id)?;
                Ok::<_, crate::manifest::ManifestError>(m)
            },
        )
    }
}
