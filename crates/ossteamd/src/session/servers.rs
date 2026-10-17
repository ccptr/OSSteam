// SPDX-License-Identifier: GPL-2.0-or-later
//! The CM server list: fetched from the Steam Directory once per daemon
//! run, cached in the cache directory, and used from the cache when the
//! directory cannot be reached (`cm.md` §1, NET-1).

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use ossteam_net::directory;
use ossteam_net::{CmKind, CmServer, ConnectError, Connection, Incoming, ServerPicker};
use ossteam_types::CancelToken;
use serde::{Deserialize, Serialize};

use super::Dialer;

pub const CACHE_FILE: &str = "cm-servers.json";

#[derive(Serialize, Deserialize)]
struct Cache {
    /// Seconds since the Unix epoch.
    fetched_at: u64,
    servers: Vec<CmServer>,
}

#[derive(Debug)]
pub enum NoServer {
    /// Every server is backing off; one is usable after this long.
    Wait(Duration),
    /// There is no list, or it has no server this build can use.
    None(String),
}

pub struct Servers {
    agent: ureq::Agent,
    cache: PathBuf,
    picker: Option<ServerPicker>,
}

impl Servers {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            agent: ossteam_net::tls::http_agent(),
            cache: cache_dir.join(CACHE_FILE),
            picker: None,
        }
    }

    fn load(&self, cell_id: u32) -> Result<Vec<CmServer>, NoServer> {
        match directory::fetch(&self.agent, cell_id) {
            Ok(list) if !list.is_empty() => {
                self.save(&list);
                return Ok(list);
            }
            Ok(_) => tracing::warn!("the Steam Directory returned no servers"),
            Err(e) => tracing::warn!("Steam Directory lookup failed: {e}"),
        }
        match self.read_cache() {
            Some(c) if !c.servers.is_empty() => {
                let age = now_unix().saturating_sub(c.fetched_at);
                tracing::info!(age_secs = age, "using the cached CM server list");
                Ok(c.servers)
            }
            _ => Err(NoServer::None(
                "the Steam Directory is unreachable and no server list is cached".into(),
            )),
        }
    }

    fn read_cache(&self) -> Option<Cache> {
        let text = std::fs::read_to_string(&self.cache).ok()?;
        serde_json::from_str(&text)
            .inspect_err(|e| tracing::warn!(file = %self.cache.display(), "ignoring a malformed server cache: {e}"))
            .ok()
    }

    fn save(&self, servers: &[CmServer]) {
        let cache = Cache {
            fetched_at: now_unix(),
            servers: servers.to_vec(),
        };
        let json = serde_json::to_string(&cache).expect("the cache serializes");
        let tmp = self.cache.with_extension("json.tmp");
        let result = self
            .cache
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, json))
            .and_then(|()| std::fs::rename(&tmp, &self.cache));
        if let Err(e) = result {
            tracing::warn!(file = %self.cache.display(), "cannot cache the server list: {e}");
        }
    }
}

impl Dialer for Servers {
    /// The next server to try. Only WebSocket servers are used: the TCP
    /// transport is not built (`cm.md` §2.2, FAITH-2).
    fn next(&mut self, cell_id: u32) -> Result<CmServer, NoServer> {
        if self.picker.is_none() {
            self.picker = Some(ServerPicker::new(self.load(cell_id)?));
        }
        let picker = self.picker.as_ref().expect("set above");
        match picker.next(CmKind::Websockets, Instant::now()) {
            Ok(s) => Ok(s.clone()),
            Err(Some(wait)) => Err(NoServer::Wait(wait)),
            Err(None) => {
                // Drop the list so the next attempt asks the directory again.
                self.picker = None;
                Err(NoServer::None(
                    "the server list has no WebSocket servers".into(),
                ))
            }
        }
    }

    fn failed(&mut self, endpoint: &str) {
        if let Some(p) = &mut self.picker {
            p.failed(endpoint, Instant::now());
        }
    }

    fn succeeded(&mut self, endpoint: &str) {
        if let Some(p) = &mut self.picker {
            p.succeeded(endpoint);
        }
    }

    fn connect(
        &mut self,
        server: &CmServer,
        timeout: Duration,
        events: Sender<Incoming>,
        cancel: &CancelToken,
    ) -> Result<Connection, ConnectError> {
        Connection::connect(server, timeout, events, cancel)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
