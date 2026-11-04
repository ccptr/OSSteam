// SPDX-License-Identifier: GPL-2.0-or-later
//! The HTTP cache of `docs/spec/steamworks/isteamhttp.md` §3.1: the rules
//! of RFC 9111 for one private cache per app, kept on disk.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use sha1::{Digest as _, Sha1};
use ureq::http::{HeaderMap, HeaderName, HeaderValue, header};

use super::cookies::parse_date;

/// The largest body stored (§3.1).
pub const MAX_ENTRY: usize = 16 * 1024 * 1024;
/// What the cache holds over all apps.
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
/// The longest heuristic freshness lifetime, in seconds.
const MAX_HEURISTIC: i64 = 86_400;
/// What a stale answer carries in `Warning`, as in the official client.
const STALE_WARNING: &str = "100 127.0.0.1 stale";
const MAGIC: &[u8; 8] = b"OSHCACH1";
/// The statuses that may be stored without explicit freshness.
const BY_DEFAULT: [u16; 11] = [200, 203, 204, 300, 301, 308, 404, 405, 410, 414, 501];
/// Headers never stored.
const UNSTORED: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "set-cookie",
    "set-cookie2",
];
/// Request headers with which a game makes its own conditional or
/// partial request, which skips the cache.
const GAME_CONDITIONAL: [&str; 6] = [
    "range",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
    "if-range",
];

/// Whether a request with the game's `headers` skips the cache.
pub fn skips(headers: &HeaderMap) -> bool {
    GAME_CONDITIONAL.iter().any(|h| headers.contains_key(*h))
}

/// The directives of `Cache-Control`.
#[derive(Debug, Default)]
pub struct Directives(Vec<(String, Option<String>)>);

impl Directives {
    pub fn of(headers: &HeaderMap) -> Self {
        let mut out = Vec::new();
        for v in headers.get_all(header::CACHE_CONTROL) {
            let Ok(v) = v.to_str() else { continue };
            for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let (k, val) = match part.split_once('=') {
                    Some((k, v)) => (k.trim(), Some(v.trim().trim_matches('"').to_owned())),
                    None => (part, None),
                };
                out.push((k.to_ascii_lowercase(), val));
            }
        }
        Self(out)
    }

    /// A request's: `Pragma: no-cache` counts when it has no
    /// `Cache-Control`.
    pub fn of_request(headers: &HeaderMap) -> Self {
        let mut d = Self::of(headers);
        let pragma = headers.get_all(header::PRAGMA).iter().any(|v| {
            v.to_str().is_ok_and(|v| {
                v.split(',')
                    .any(|p| p.trim().eq_ignore_ascii_case("no-cache"))
            })
        });
        if pragma && !headers.contains_key(header::CACHE_CONTROL) {
            d.0.push(("no-cache".into(), None));
        }
        d
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }

    /// A directive's value in seconds: `None` when it is absent,
    /// `Some(None)` when it has no value or one that is not a number.
    pub fn seconds(&self, name: &str) -> Option<Option<i64>> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| {
            v.as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .map(|n| i64::try_from(n).unwrap_or(i64::MAX / 2).min(i64::MAX / 2))
        })
    }
}

/// What a stored response is good for (§3.1, Lookup).
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Stale, but the request allows it.
    Stale,
    Validate,
}

/// A stored response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub status: u16,
    pub headers: HeaderMap,
    /// The request headers that `Vary` names, as they were sent.
    pub vary: Vec<(String, Option<Vec<u8>>)>,
    /// Unix seconds when the request went out and its answer came.
    pub request_time: i64,
    pub response_time: i64,
    pub body: Vec<u8>,
}

fn text<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The names `Vary` lists, lower case; `None` for `Vary: *`.
fn vary_names(headers: &HeaderMap) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for v in headers.get_all(header::VARY) {
        for n in String::from_utf8_lossy(v.as_bytes()).split(',') {
            let n = n.trim();
            if n == "*" {
                return None;
            }
            if !n.is_empty() {
                names.push(n.to_ascii_lowercase());
            }
        }
    }
    Some(names)
}

/// A request header's values, joined as one.
fn request_value(request: &HeaderMap, name: &str) -> Option<Vec<u8>> {
    let mut values = request.get_all(name).iter().peekable();
    values.peek()?;
    let mut out = Vec::new();
    for (i, v) in values.enumerate() {
        if i > 0 {
            out.extend_from_slice(b", ");
        }
        out.extend_from_slice(v.as_bytes());
    }
    Some(out)
}

/// The headers of `response` that are stored.
fn stored_headers(response: &HeaderMap) -> HeaderMap {
    let named: Vec<String> = response
        .get_all(header::CONNECTION)
        .iter()
        .flat_map(|v| {
            String::from_utf8_lossy(v.as_bytes())
                .split(',')
                .map(|n| n.trim().to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut out = HeaderMap::new();
    for (name, value) in response {
        let n = name.as_str();
        if !UNSTORED.contains(&n) && !named.iter().any(|x| x == n) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

/// Whether a GET response from the network may be stored (§3.1,
/// Storing); the body's size is checked as it is read.
pub fn storable(status: u16, request: &Directives, response: &HeaderMap) -> bool {
    let d = Directives::of(response);
    let validator =
        response.contains_key(header::ETAG) || response.contains_key(header::LAST_MODIFIED);
    status >= 200
        && !matches!(status, 206 | 304)
        && !request.has("no-store")
        && !d.has("no-store")
        && vary_names(response).is_some()
        && (d.seconds("max-age").is_some()
            || response.contains_key(header::EXPIRES)
            || d.has("public")
            || d.has("private")
            || (BY_DEFAULT.contains(&status) && validator))
}

impl Entry {
    /// A response to store, as `request` was sent.
    pub fn new(
        url: &str,
        status: u16,
        response: &HeaderMap,
        request: &HeaderMap,
        request_time: i64,
        response_time: i64,
    ) -> Self {
        let vary = vary_names(response)
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let v = request_value(request, &n);
                (n, v)
            })
            .collect();
        Self {
            url: url.to_owned(),
            status,
            headers: stored_headers(response),
            vary,
            request_time,
            response_time,
            body: Vec::new(),
        }
    }

    /// Whether `request` sends what `Vary` named, as the stored one did.
    pub fn matches(&self, request: &HeaderMap) -> bool {
        self.vary
            .iter()
            .all(|(n, v)| request_value(request, n) == *v)
    }

    fn date(&self) -> i64 {
        text(&self.headers, &header::DATE)
            .and_then(parse_date)
            .unwrap_or(self.response_time)
    }

    /// RFC 9111 §4.2.3.
    pub fn age(&self, now: i64) -> i64 {
        let age_value = text(&self.headers, &header::AGE)
            .and_then(|a| a.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);
        let apparent = (self.response_time - self.date()).max(0);
        let delay = (self.response_time - self.request_time).max(0);
        let initial = apparent.max(age_value.saturating_add(delay));
        initial.saturating_add((now - self.response_time).max(0))
    }

    /// RFC 9111 §4.2.1, and the heuristic of §3.1.
    pub fn lifetime(&self) -> i64 {
        let d = Directives::of(&self.headers);
        if let Some(max_age) = d.seconds("max-age") {
            return max_age.unwrap_or(0);
        }
        if let Some(expires) = self.headers.get(header::EXPIRES) {
            return expires
                .to_str()
                .ok()
                .and_then(parse_date)
                .map_or(0, |e| (e - self.date()).max(0));
        }
        if BY_DEFAULT.contains(&self.status)
            && let Some(modified) = text(&self.headers, &header::LAST_MODIFIED).and_then(parse_date)
        {
            return ((self.date() - modified) / 10).clamp(0, MAX_HEURISTIC);
        }
        0
    }

    pub fn decide(&self, request: &Directives, now: i64) -> Freshness {
        let d = Directives::of(&self.headers);
        let age = self.age(now);
        let lifetime = self.lifetime();
        if d.has("no-cache") || d.has("must-revalidate") || request.has("no-cache") {
            return Freshness::Validate;
        }
        if let Some(Some(max_age)) = request.seconds("max-age")
            && age > max_age
        {
            return Freshness::Validate;
        }
        if let Some(Some(min_fresh)) = request.seconds("min-fresh")
            && lifetime - age < min_fresh
        {
            return Freshness::Validate;
        }
        if age < lifetime {
            return Freshness::Fresh;
        }
        match request.seconds("max-stale") {
            Some(None) => Freshness::Stale,
            Some(Some(n)) if age - lifetime <= n => Freshness::Stale,
            _ => Freshness::Validate,
        }
    }

    /// The headers that validate it: `If-None-Match` and
    /// `If-Modified-Since`. Empty when it cannot be validated.
    pub fn validators(&self) -> Vec<(HeaderName, HeaderValue)> {
        let mut out = Vec::new();
        if let Some(etag) = self.headers.get(header::ETAG) {
            out.push((header::IF_NONE_MATCH, etag.clone()));
        }
        if let Some(modified) = self.headers.get(header::LAST_MODIFIED) {
            out.push((header::IF_MODIFIED_SINCE, modified.clone()));
        }
        out
    }

    /// Takes the headers of a 304 that validated it.
    pub fn refresh(&mut self, response: &HeaderMap, request_time: i64, response_time: i64) {
        let fresh = stored_headers(response);
        for name in fresh.keys() {
            if *name == header::CONTENT_LENGTH {
                continue;
            }
            self.headers.remove(name);
            for v in fresh.get_all(name) {
                self.headers.append(name.clone(), v.clone());
            }
        }
        self.request_time = request_time;
        self.response_time = response_time;
    }

    /// The headers it is answered with: `Age`, and the stale `Warning`.
    pub fn answer_headers(&self, now: i64, stale: bool) -> HeaderMap {
        let mut headers = self.headers.clone();
        headers.insert(header::AGE, HeaderValue::from(self.age(now).max(0)));
        if stale {
            let warning = match text(&headers, &header::WARNING) {
                Some(w) => format!("{w},{STALE_WARNING}"),
                None => STALE_WARNING.to_owned(),
            };
            if let Ok(v) = HeaderValue::from_str(&warning) {
                headers.insert(header::WARNING, v);
            }
        }
        headers
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 1024);
        let bytes = |out: &mut Vec<u8>, b: &[u8]| {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        };
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.request_time.to_le_bytes());
        out.extend_from_slice(&self.response_time.to_le_bytes());
        out.extend_from_slice(&self.status.to_le_bytes());
        bytes(&mut out, self.url.as_bytes());
        out.extend_from_slice(&(self.headers.len() as u32).to_le_bytes());
        for (n, v) in &self.headers {
            bytes(&mut out, n.as_str().as_bytes());
            bytes(&mut out, v.as_bytes());
        }
        out.extend_from_slice(&(self.vary.len() as u32).to_le_bytes());
        for (n, v) in &self.vary {
            bytes(&mut out, n.as_bytes());
            match v {
                Some(v) => {
                    out.push(1);
                    bytes(&mut out, v);
                }
                None => out.push(0),
            }
        }
        out.extend_from_slice(&(self.body.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.body);
        out
    }

    fn decode(data: &[u8]) -> Option<Self> {
        let mut r = Reader(data);
        if r.take(MAGIC.len())? != MAGIC {
            return None;
        }
        let request_time = i64::from_le_bytes(r.take(8)?.try_into().ok()?);
        let response_time = i64::from_le_bytes(r.take(8)?.try_into().ok()?);
        let status = u16::from_le_bytes(r.take(2)?.try_into().ok()?);
        let url = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let mut headers = HeaderMap::new();
        for _ in 0..r.u32()? {
            let name = HeaderName::from_bytes(r.bytes()?).ok()?;
            let value = HeaderValue::from_bytes(r.bytes()?).ok()?;
            headers.append(name, value);
        }
        let mut vary = Vec::new();
        for _ in 0..r.u32()? {
            let name = String::from_utf8(r.bytes()?.to_vec()).ok()?;
            let value = match r.take(1)?[0] {
                0 => None,
                1 => Some(r.bytes()?.to_vec()),
                _ => return None,
            };
            vary.push((name, value));
        }
        let len = usize::try_from(u64::from_le_bytes(r.take(8)?.try_into().ok()?)).ok()?;
        let body = r.take(len)?.to_vec();
        r.0.is_empty().then_some(Self {
            url,
            status,
            headers,
            vary,
            request_time,
            response_time,
            body,
        })
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// When a file was last used: seconds, then the order within the run.
type Used = (i64, u64);

#[derive(Default)]
struct State {
    scanned: bool,
    files: HashMap<PathBuf, (u64, Used)>,
    total: u64,
    seq: u64,
}

/// The cache: `<cache>/http/<app>/<SHA-1 of the URL>`.
pub struct Cache {
    root: PathBuf,
    limit: u64,
    state: Mutex<State>,
}

fn mtime(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

impl Cache {
    pub fn new(root: PathBuf) -> Self {
        Self::with_limit(root, MAX_TOTAL)
    }

    fn with_limit(root: PathBuf, limit: u64) -> Self {
        Self {
            root,
            limit,
            state: Mutex::new(State::default()),
        }
    }

    fn path(&self, app: u32, url: &str) -> PathBuf {
        let hash = Sha1::digest(url.as_bytes());
        let name: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        self.root.join(app.to_string()).join(name)
    }

    /// The state, with the files on disk counted the first time.
    fn state(&self) -> MutexGuard<'_, State> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.scanned {
            s.scanned = true;
            for app in fs::read_dir(&self.root).into_iter().flatten().flatten() {
                for file in fs::read_dir(app.path()).into_iter().flatten().flatten() {
                    let path = file.path();
                    if path.extension().is_some() {
                        // A write that did not finish.
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if let Ok(meta) = file.metadata()
                        && meta.is_file()
                    {
                        s.total += meta.len();
                        s.files.insert(path, (meta.len(), (mtime(&meta), 0)));
                    }
                }
            }
        }
        s
    }

    fn forget(s: &mut State, path: &Path) {
        if let Some((size, _)) = s.files.remove(path) {
            s.total -= size;
        }
        let _ = fs::remove_file(path);
    }

    /// What is stored for `url`.
    pub fn get(&self, app: u32, url: &str, now: i64) -> Option<Entry> {
        let path = self.path(app, url);
        let mut s = self.state();
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::debug!("cannot read a cached HTTP response: {e}");
                }
                Self::forget(&mut s, &path);
                return None;
            }
        };
        let Some(entry) = Entry::decode(&data).filter(|e| e.url == url) else {
            tracing::debug!("a cached HTTP response does not read back; it is removed");
            Self::forget(&mut s, &path);
            return None;
        };
        s.seq += 1;
        let seq = s.seq;
        if let Some((_, used)) = s.files.get_mut(&path) {
            *used = (now, seq);
        }
        if let Ok(f) = fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(entry)
    }

    /// Stores `entry` for its URL, replacing what was there.
    pub fn put(&self, app: u32, entry: &Entry, now: i64) {
        let path = self.path(app, &entry.url);
        let data = entry.encode();
        let mut s = self.state();
        Self::forget(&mut s, &path);
        let tmp = path.with_extension("tmp");
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(&data)?;
                f.sync_data()
            })
            .and_then(|()| fs::rename(&tmp, &path));
        if let Err(e) = written {
            tracing::info!("cannot store an HTTP response in the cache: {e}");
            let _ = fs::remove_file(&tmp);
            return;
        }
        s.seq += 1;
        let seq = s.seq;
        s.total += data.len() as u64;
        s.files
            .insert(path.clone(), (data.len() as u64, (now, seq)));
        while s.total > self.limit {
            let Some(oldest) = s
                .files
                .iter()
                .filter(|(p, _)| **p != path)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(p, _)| p.clone())
            else {
                break;
            };
            Self::forget(&mut s, &oldest);
        }
    }

    /// Removes what is stored for `url`.
    pub fn remove(&self, app: u32, url: &str) {
        let path = self.path(app, url);
        let mut s = self.state();
        Self::forget(&mut s, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(list: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (n, v) in list {
            h.append(
                HeaderName::from_bytes(n.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn entry(list: &[(&str, &str)], at: i64) -> Entry {
        Entry::new(
            "http://a.example/x",
            200,
            &headers(list),
            &HeaderMap::new(),
            at,
            at,
        )
    }

    fn req(cc: &str) -> Directives {
        Directives::of_request(&headers(&[("cache-control", cc)]))
    }

    #[test]
    fn freshness_follows_the_directives() {
        let e = entry(&[("cache-control", "max-age=100")], 1000);
        let none = Directives::default();
        assert_eq!(e.lifetime(), 100);
        assert_eq!(e.decide(&none, 1050), Freshness::Fresh);
        assert_eq!(e.decide(&none, 1100), Freshness::Validate);
        assert_eq!(e.decide(&req("max-age=10"), 1050), Freshness::Validate);
        assert_eq!(e.decide(&req("min-fresh=60"), 1050), Freshness::Validate);
        assert_eq!(e.decide(&req("no-cache"), 1001), Freshness::Validate);
        let pragma = Directives::of_request(&headers(&[("pragma", "no-cache")]));
        assert_eq!(e.decide(&pragma, 1001), Freshness::Validate);
        assert_eq!(e.decide(&req("max-stale"), 99_999), Freshness::Stale);
        assert_eq!(e.decide(&req("max-stale=50"), 1150), Freshness::Stale);
        assert_eq!(e.decide(&req("max-stale=50"), 1151), Freshness::Validate);
        assert_eq!(
            e.decide(&req("only-if-cached, max-stale=2678400"), 5000),
            Freshness::Stale
        );
        // must-revalidate and no-cache validate even while fresh, as the
        // official client's log message suggests (§3.1).
        let e = entry(&[("cache-control", "max-age=100, must-revalidate")], 1000);
        assert_eq!(e.decide(&none, 1001), Freshness::Validate);
        assert_eq!(e.decide(&req("max-stale"), 1200), Freshness::Validate);
    }

    #[test]
    fn age_and_lifetime_come_from_the_headers() {
        let date = "Wed, 21 Oct 2015 07:28:00 GMT";
        let t = 1_445_412_480;
        let e = entry(
            &[
                ("date", date),
                ("age", "30"),
                ("expires", "Wed, 21 Oct 2015 07:29:40 GMT"),
            ],
            t,
        );
        assert_eq!(e.lifetime(), 100);
        assert_eq!(e.age(t + 10), 40);
        // The heuristic: a tenth of the time since it changed, at most a day.
        let e = entry(
            &[
                ("date", date),
                ("last-modified", "Wed, 21 Oct 2015 06:28:00 GMT"),
            ],
            t,
        );
        assert_eq!(e.lifetime(), 360);
        let e = entry(
            &[
                ("date", date),
                ("last-modified", "Wed, 21 Oct 2005 07:28:00 GMT"),
            ],
            t,
        );
        assert_eq!(e.lifetime(), MAX_HEURISTIC);
        // A bad Expires is already stale.
        assert_eq!(entry(&[("expires", "0")], t).lifetime(), 0);
    }

    #[test]
    fn only_some_responses_are_stored() {
        let none = Directives::default();
        let s = |status, list: &[(&str, &str)]| storable(status, &none, &headers(list));
        assert!(s(200, &[("cache-control", "max-age=60")]));
        assert!(s(500, &[("cache-control", "max-age=60")]));
        assert!(s(200, &[("etag", "\"a\"")]));
        assert!(s(
            404,
            &[("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT")]
        ));
        assert!(!s(500, &[("etag", "\"a\"")]));
        assert!(!s(200, &[]));
        assert!(!s(302, &[("location", "/x")]));
        assert!(!s(206, &[("cache-control", "max-age=60")]));
        assert!(!s(200, &[("cache-control", "max-age=60, no-store")]));
        assert!(!s(200, &[("cache-control", "max-age=60"), ("vary", "*")]));
        assert!(!storable(
            200,
            &req("no-store"),
            &headers(&[("cache-control", "max-age=60")])
        ));
    }

    #[test]
    fn vary_and_stored_headers() {
        let response = headers(&[
            ("vary", "X-A, x-b"),
            ("set-cookie", "a=1"),
            ("connection", "x-hop"),
            ("x-hop", "1"),
            ("x-kept", "2"),
        ]);
        let request = headers(&[("x-a", "1")]);
        let e = Entry::new("u", 200, &response, &request, 0, 0);
        assert!(e.headers.get("set-cookie").is_none());
        assert!(e.headers.get("x-hop").is_none());
        assert_eq!(e.headers.get("x-kept").unwrap(), "2");
        assert!(e.matches(&headers(&[("x-a", "1")])));
        assert!(!e.matches(&headers(&[("x-a", "2")])));
        assert!(!e.matches(&headers(&[("x-a", "1"), ("x-b", "1")])));
    }

    #[test]
    fn a_304_refreshes_and_stale_answers_warn() {
        let mut e = entry(
            &[
                ("cache-control", "max-age=0"),
                ("etag", "\"a\""),
                ("content-length", "3"),
            ],
            10,
        );
        assert_eq!(
            e.validators(),
            vec![(header::IF_NONE_MATCH, HeaderValue::from_static("\"a\""))]
        );
        e.refresh(
            &headers(&[("cache-control", "max-age=60"), ("content-length", "0")]),
            20,
            21,
        );
        assert_eq!(e.headers.get("cache-control").unwrap(), "max-age=60");
        assert_eq!(e.headers.get("content-length").unwrap(), "3");
        assert_eq!(e.decide(&Directives::default(), 30), Freshness::Fresh);
        let h = e.answer_headers(30, true);
        assert_eq!(h.get("age").unwrap(), "10");
        assert_eq!(h.get("warning").unwrap(), STALE_WARNING);
    }

    #[test]
    fn entries_persist_and_the_oldest_go() {
        let root = std::env::temp_dir().join(format!("ossteamd-httpcache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cache = Cache::with_limit(root.clone(), 1000);
        let mut e = entry(&[("cache-control", "max-age=60"), ("vary", "x-a")], 5);
        e.body = vec![7; 300];
        e.vary = vec![("x-a".into(), Some(b"1".to_vec())), ("x-b".into(), None)];
        cache.put(1, &e, 5);
        assert_eq!(cache.get(1, &e.url, 6), Some(e.clone()));
        assert_eq!(cache.get(2, &e.url, 6), None, "apps do not share");
        // A new Cache reads what is on disk.
        let cache = Cache::with_limit(root.clone(), 1000);
        assert_eq!(cache.get(1, &e.url, 7).map(|e| e.body.len()), Some(300));
        let other = |url: &str, at| {
            let mut x = e.clone();
            x.url = url.into();
            cache.put(1, &x, at);
        };
        other("http://a.example/y", 8);
        other("http://a.example/z", 9);
        // Over 1000 bytes, the least recently used went.
        assert_eq!(cache.get(1, &e.url, 10), None);
        assert!(cache.get(1, "http://a.example/z", 10).is_some());
        cache.remove(1, "http://a.example/z");
        assert!(cache.get(1, "http://a.example/z", 10).is_none());
        // A damaged file is removed.
        let path = cache.path(1, "http://a.example/y");
        fs::write(&path, b"junk").unwrap();
        assert!(cache.get(1, "http://a.example/y", 11).is_none());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
