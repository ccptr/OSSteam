// SPDX-License-Identifier: GPL-2.0-or-later
//! Finding CM servers (`docs/spec/cm.md` §1, NET-1).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const DIRECTORY_URL: &str = "https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/";

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(5 * 60);

/// How a server is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CmKind {
    /// WebSocket over TLS (NET-2, primary).
    Websockets,
    /// Raw TCP with Steam channel encryption (NET-2, fallback).
    Netfilter,
}

/// One entry of the directory's server list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CmServer {
    /// `host:port`.
    pub endpoint: String,
    #[serde(rename = "type")]
    pub kind: CmKind,
    #[serde(default)]
    pub dc: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectoryError {
    #[error("directory request failed: {0}")]
    Http(#[from] ureq::Error),
    #[error("directory response is malformed: {0}")]
    Malformed(String),
    #[error("directory reported failure: {0}")]
    Refused(String),
}

#[derive(Deserialize)]
struct Envelope {
    response: Response,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    serverlist: Vec<RawServer>,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct RawServer {
    endpoint: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    dc: String,
}

/// Asks the Steam Directory for servers, in the order it prefers them.
/// Entries of a type this build does not know are skipped.
pub fn fetch(agent: &ureq::Agent, cell_id: u32) -> Result<Vec<CmServer>, DirectoryError> {
    let body = agent
        .get(DIRECTORY_URL)
        .query("cellid", cell_id.to_string())
        .call()?
        .body_mut()
        .read_to_string()?;
    parse(&body)
}

fn parse(body: &str) -> Result<Vec<CmServer>, DirectoryError> {
    let env: Envelope =
        serde_json::from_str(body).map_err(|e| DirectoryError::Malformed(e.to_string()))?;
    if !env.response.success {
        return Err(DirectoryError::Refused(env.response.message));
    }
    Ok(env
        .response
        .serverlist
        .into_iter()
        .filter_map(|s| {
            let kind = match s.kind.as_str() {
                "websockets" => CmKind::Websockets,
                "netfilter" => CmKind::Netfilter,
                other => {
                    tracing::debug!(kind = other, endpoint = %s.endpoint, "skipping CM of unknown type");
                    return None;
                }
            };
            Some(CmServer {
                endpoint: s.endpoint,
                kind,
                dc: s.dc,
            })
        })
        .collect())
}

/// Chooses servers in directory order, skipping ones that failed recently.
///
/// A server's backoff doubles from 1 s up to 5 min with each consecutive
/// failure, with up to 50% random jitter added, and resets on success.
#[derive(Debug)]
pub struct ServerPicker {
    servers: Vec<(CmServer, Backoff)>,
}

#[derive(Debug, Default, Clone)]
struct Backoff {
    failures: u32,
    until: Option<Instant>,
}

impl ServerPicker {
    pub fn new(servers: Vec<CmServer>) -> Self {
        Self {
            servers: servers
                .into_iter()
                .map(|s| (s, Backoff::default()))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// The first usable server of `kind`, or how long until one is usable.
    pub fn next(&self, kind: CmKind, now: Instant) -> Result<&CmServer, Option<Duration>> {
        let mut soonest: Option<Instant> = None;
        for (server, backoff) in self.servers.iter().filter(|(s, _)| s.kind == kind) {
            match backoff.until {
                Some(t) if t > now => soonest = Some(soonest.map_or(t, |s| s.min(t))),
                _ => return Ok(server),
            }
        }
        Err(soonest.map(|t| t - now))
    }

    pub fn failed(&mut self, endpoint: &str, now: Instant) {
        if let Some((_, b)) = self
            .servers
            .iter_mut()
            .find(|(s, _)| s.endpoint == endpoint)
        {
            b.failures = b.failures.saturating_add(1);
            let base = BACKOFF_START
                .saturating_mul(1u32 << b.failures.saturating_sub(1).min(16))
                .min(BACKOFF_CAP);
            b.until = Some(now + base + jitter(base / 2));
        }
    }

    pub fn succeeded(&mut self, endpoint: &str) {
        if let Some((_, b)) = self
            .servers
            .iter_mut()
            .find(|(s, _)| s.endpoint == endpoint)
        {
            *b = Backoff::default();
        }
    }
}

/// A random duration in `0..=max`. `RandomState` is seeded randomly per
/// instance by std, which is plenty for spreading retries.
fn jitter(max: Duration) -> Duration {
    let mut h = RandomState::new().build_hasher();
    h.write_u8(0);
    let r = h.finish();
    let nanos = max.as_nanos() as u64;
    if nanos == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(r % (nanos + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directory_response() {
        let body = r#"{"response":{"serverlist":[
            {"endpoint":"cm1.example:443","legacy_endpoint":"1.2.3.4:27017","type":"websockets","dc":"fra1","realm":"steamglobal","load":10,"wtd_load":12.5},
            {"endpoint":"5.6.7.8:27017","type":"netfilter","dc":"fra1"},
            {"endpoint":"x:1","type":"carrier-pigeon"}
        ],"success":true,"message":""}}"#;
        let list = parse(body).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].endpoint, "cm1.example:443");
        assert_eq!(list[0].kind, CmKind::Websockets);
        assert_eq!(list[1].kind, CmKind::Netfilter);
    }

    #[test]
    fn reports_refusal() {
        let body = r#"{"response":{"success":false,"message":"nope"}}"#;
        assert!(matches!(parse(body), Err(DirectoryError::Refused(m)) if m == "nope"));
    }

    fn ws(endpoint: &str) -> CmServer {
        CmServer {
            endpoint: endpoint.into(),
            kind: CmKind::Websockets,
            dc: String::new(),
        }
    }

    #[test]
    fn picker_backs_off_and_recovers() {
        let now = Instant::now();
        let mut p = ServerPicker::new(vec![ws("a"), ws("b")]);
        assert_eq!(p.next(CmKind::Websockets, now).unwrap().endpoint, "a");
        p.failed("a", now);
        assert_eq!(p.next(CmKind::Websockets, now).unwrap().endpoint, "b");
        p.failed("b", now);
        let wait = p.next(CmKind::Websockets, now).unwrap_err().unwrap();
        assert!(wait >= BACKOFF_START && wait <= BACKOFF_START * 3 / 2);
        let later = now + Duration::from_secs(2);
        assert_eq!(p.next(CmKind::Websockets, later).unwrap().endpoint, "a");
        p.succeeded("a");
        assert_eq!(p.next(CmKind::Websockets, now).unwrap().endpoint, "a");
        assert_eq!(p.next(CmKind::Netfilter, now).unwrap_err(), None);
    }

    #[test]
    fn backoff_is_capped() {
        let now = Instant::now();
        let mut p = ServerPicker::new(vec![ws("a")]);
        for _ in 0..40 {
            p.failed("a", now);
        }
        let wait = p.next(CmKind::Websockets, now).unwrap_err().unwrap();
        assert!(wait <= BACKOFF_CAP * 3 / 2);
        assert!(wait >= BACKOFF_CAP);
    }
}
