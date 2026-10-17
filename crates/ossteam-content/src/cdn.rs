// SPDX-License-Identifier: GPL-2.0-or-later
//! Content servers: which to use, in what order, and fetching manifests
//! and chunks from them over HTTPS (`content.md` §1, §4, §5, §7; ADR 0003).

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ossteam_proto::steam::CContentServerDirectoryServerInfo;
use ossteam_types::{AppId, CancelToken, DepotId, ManifestId};

use crate::manifest::{self, Chunk};
use crate::throttle::Throttle;

/// How long a server that failed is passed over.
pub const PENALTY: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_TIMEOUT: Duration = Duration::from_secs(60);
/// Reads are accounted to the throttle in pieces of this size.
const READ_PIECE: usize = 64 * 1024;

/// One usable server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub kind: String,
    /// What CDN auth tokens are asked for (`content.md` §7).
    pub host: String,
    /// Where requests go.
    pub vhost: String,
}

/// Picks the servers to use for `app` from a server list and orders them
/// (`content.md` §1). Each appears as many times as the list asks.
pub fn select(list: &[CContentServerDirectoryServerInfo], app: AppId) -> Vec<Server> {
    let mut usable: Vec<&CContentServerDirectoryServerInfo> = list
        .iter()
        .filter(|s| matches!(s.r#type(), "CDN" | "SteamCache"))
        .filter(|s| matches!(s.https_support(), "mandatory" | "optional"))
        .filter(|s| !s.steam_china_only())
        .filter(|s| s.allowed_app_ids.is_empty() || s.allowed_app_ids.contains(&app.0))
        .filter(|s| !s.vhost().is_empty() || !s.host().is_empty())
        .collect();
    usable.sort_by(|a, b| a.weighted_load().total_cmp(&b.weighted_load()));
    let mut out = Vec::new();
    for s in usable {
        let server = Server {
            kind: s.r#type().to_owned(),
            host: s.host().to_owned(),
            vhost: if s.vhost().is_empty() {
                s.host()
            } else {
                s.vhost()
            }
            .to_owned(),
        };
        for _ in 0..s.num_entries_in_client_list().max(1) {
            out.push(server.clone());
        }
    }
    out
}

/// The rotation of servers one job uses, shared by its download threads.
#[derive(Debug)]
pub struct Rotation {
    inner: Mutex<RotationState>,
}

#[derive(Debug)]
struct RotationState {
    list: Vec<Server>,
    next: usize,
    /// Until when each failed host is passed over.
    penalised: Vec<(String, Instant)>,
}

impl Rotation {
    /// `None` when `list` is empty.
    pub fn new(list: Vec<Server>) -> Option<Self> {
        (!list.is_empty()).then(|| Self {
            inner: Mutex::new(RotationState {
                list,
                next: 0,
                penalised: Vec::new(),
            }),
        })
    }

    /// The next server to try.
    pub fn pick(&self) -> Server {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        s.penalised.retain(|(_, until)| *until > now);
        let n = s.list.len();
        for i in 0..n {
            let idx = (s.next + i) % n;
            let candidate = &s.list[idx];
            if !s.penalised.iter().any(|(h, _)| *h == candidate.vhost) {
                let picked = candidate.clone();
                s.next = idx + 1;
                return picked;
            }
        }
        // Everything is penalised: the one whose penalty ends first.
        let (host, _) = s
            .penalised
            .iter()
            .min_by_key(|(_, until)| *until)
            .cloned()
            .expect("all servers penalised implies a penalty");
        s.list
            .iter()
            .find(|srv| srv.vhost == host)
            .cloned()
            .expect("penalised hosts are in the list")
    }

    /// `server` failed a request; pass it over for [`PENALTY`].
    pub fn failed(&self, server: &Server) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let until = Instant::now() + PENALTY;
        match s.penalised.iter_mut().find(|(h, _)| *h == server.vhost) {
            Some(entry) => entry.1 = until,
            None => s.penalised.push((server.vhost.clone(), until)),
        }
    }

    /// Whether every server is currently penalised.
    pub fn exhausted(&self) -> bool {
        let s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        s.list.iter().all(|srv| {
            s.penalised
                .iter()
                .any(|(h, until)| *h == srv.vhost && *until > now)
        })
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum FetchError {
    #[error("HTTP {0}")]
    Status(u16),
    #[error("{0}")]
    Transport(String),
    #[error("the response is larger than {0} bytes")]
    TooLarge(u64),
    #[error("cancelled")]
    Cancelled,
}

/// GETs from content servers. The daemon uses [`Https`]; tests substitute
/// a fake CDN (FAITH-4, as ADR 0002 does for the CM).
pub trait Fetch: Send + Sync {
    /// The body of `https://<vhost><path>`, which must not exceed `limit`
    /// bytes.
    fn get(
        &self,
        vhost: &str,
        path: &str,
        limit: u64,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, FetchError>;
}

/// The real thing: HTTPS with the platform verifier (SEC-3), paced by the
/// shared throttle.
pub struct Https {
    agent: ureq::Agent,
    throttle: Arc<Throttle>,
}

impl Https {
    /// `connections` is how many downloads may run at once across jobs;
    /// that many idle connections are kept for reuse.
    pub fn new(throttle: Arc<Throttle>, connections: usize) -> Self {
        let agent = ossteam_net::tls::agent_config()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_body(Some(BODY_TIMEOUT))
            .max_idle_connections(connections.max(4))
            .max_idle_connections_per_host(connections.max(4))
            .build()
            .new_agent();
        Self { agent, throttle }
    }
}

impl Fetch for Https {
    fn get(
        &self,
        vhost: &str,
        path: &str,
        limit: u64,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, FetchError> {
        let url = format!("https://{vhost}{path}");
        let mut resp = match self.agent.get(&url).call() {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(code)) => return Err(FetchError::Status(code)),
            Err(e) => return Err(FetchError::Transport(e.to_string())),
        };
        // One byte over the limit is let through so that the check below,
        // not the reader, reports it.
        let mut reader = resp
            .body_mut()
            .with_config()
            .limit(limit.saturating_add(1))
            .reader();
        let mut out = Vec::new();
        let mut piece = vec![0u8; READ_PIECE];
        loop {
            if !self.throttle.take(READ_PIECE, cancel) {
                return Err(FetchError::Cancelled);
            }
            match reader.read(&mut piece) {
                Ok(0) => return Ok(out),
                Ok(n) => {
                    out.extend_from_slice(&piece[..n]);
                    if out.len() as u64 > limit {
                        return Err(FetchError::TooLarge(limit));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(FetchError::Transport(e.to_string())),
            }
        }
    }
}

/// The path of a manifest (`content.md` §4).
pub fn manifest_path(depot: DepotId, manifest: ManifestId, code: u64) -> String {
    format!("/depot/{}/manifest/{}/5/{code}", depot.0, manifest.0)
}

/// The path of a chunk (`content.md` §5), with a CDN auth token if one is
/// needed (`content.md` §7).
pub fn chunk_path(depot: DepotId, chunk: &Chunk, token: Option<&str>) -> String {
    let mut p = format!("/depot/{}/chunk/{}", depot.0, chunk.id);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        p.push('?');
        p.push_str(t.trim_start_matches('?'));
    }
    p
}

/// A zipped manifest is at most this large; the unzipped limit bounds it.
pub const MANIFEST_LIMIT: u64 = manifest::MAX_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kind: &str, host: &str, load: f32, https: &str) -> CContentServerDirectoryServerInfo {
        CContentServerDirectoryServerInfo {
            r#type: Some(kind.into()),
            host: Some(host.into()),
            vhost: Some(format!("v.{host}")),
            weighted_load: Some(load),
            https_support: Some(https.into()),
            num_entries_in_client_list: Some(1),
            ..Default::default()
        }
    }

    #[test]
    fn selection_filters_and_orders() {
        let mut only_other_app = info("CDN", "d", 1.0, "optional");
        only_other_app.allowed_app_ids = vec![10];
        let mut twice = info("SteamCache", "e", 5.0, "mandatory");
        twice.num_entries_in_client_list = Some(2);
        let list = vec![
            info("CDN", "a", 50.0, "mandatory"),
            info("CDN", "b", 10.0, "none"),
            info("OpenCache", "c", 1.0, "optional"),
            only_other_app,
            twice,
        ];
        let picked: Vec<String> = select(&list, AppId(480))
            .into_iter()
            .map(|s| s.host)
            .collect();
        assert_eq!(picked, ["e", "e", "a"]);
        assert_eq!(select(&list, AppId(10))[0].host, "d");
    }

    #[test]
    fn rotation_skips_failed_servers_until_all_have_failed() {
        let s = |h: &str| Server {
            kind: "CDN".into(),
            host: h.into(),
            vhost: h.into(),
        };
        let r = Rotation::new(vec![s("a"), s("b")]).unwrap();
        assert_eq!(r.pick().host, "a");
        assert_eq!(r.pick().host, "b");
        r.failed(&s("a"));
        assert_eq!(r.pick().host, "b");
        assert_eq!(r.pick().host, "b");
        assert!(!r.exhausted());
        r.failed(&s("b"));
        assert!(r.exhausted());
        // The one whose penalty ends first.
        assert_eq!(r.pick().host, "a");
        assert!(Rotation::new(Vec::new()).is_none());
    }

    #[test]
    fn paths() {
        assert_eq!(
            manifest_path(DepotId(481), ManifestId(99), 1234),
            "/depot/481/manifest/99/5/1234"
        );
        let c = Chunk {
            id: crate::manifest::ChunkId([0xab; 20]),
            crc: 0,
            offset: 0,
            original: 1,
            compressed: 1,
        };
        let id = "ab".repeat(20);
        assert_eq!(
            chunk_path(DepotId(1), &c, None),
            format!("/depot/1/chunk/{id}")
        );
        assert_eq!(
            chunk_path(DepotId(1), &c, Some("?token=x")),
            format!("/depot/1/chunk/{id}?token=x")
        );
    }
}
