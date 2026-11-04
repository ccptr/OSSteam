// SPDX-License-Identifier: GPL-2.0-or-later
//! Making a game's HTTP request (`docs/spec/steamworks/isteamhttp.md`
//! §3): one request with its redirects and cookies, on a worker thread.

mod cookies;

use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ossteam_gameipc::{HttpDone, HttpHeader, HttpHeaders, HttpSendRequest};
use ossteam_types::CancelToken;
use ureq::config::AutoHeaderValue;
use ureq::http::{self, HeaderName, HeaderValue, Method, Uri, header};

pub use cookies::{Jar, now as cookies_now};

/// The largest body of a response that is not streamed (§6).
pub const MAX_BODY: u64 = 256 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
/// The most one chunk holds (§5).
const CHUNK: usize = 256 * 1024;
const USER_AGENT: &str = "Valve/Steam HTTP Client 1.0";

/// The agent for games' requests: the TLS policy of SEC-3; statuses,
/// redirects and the User-Agent are left to [`run`].
pub fn agent() -> ureq::Agent {
    ossteam_net::tls::agent_config()
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(AutoHeaderValue::None)
        .build()
        .new_agent()
}

/// A request to make.
pub struct Job {
    /// The connection's app, for the User-Agent.
    pub app: u32,
    pub request: HttpSendRequest,
    /// The cookie container, and whether responses may change it.
    pub jar: Option<Arc<Mutex<Jar>>>,
}

/// What the request has to tell before it ends.
pub enum Event {
    Headers(HttpHeaders),
    Data(Vec<u8>),
}

fn method(m: u32) -> Option<Method> {
    Some(match m {
        1 => Method::GET,
        2 => Method::HEAD,
        3 => Method::POST,
        4 => Method::PUT,
        5 => Method::DELETE,
        6 => Method::OPTIONS,
        7 => Method::PATCH,
        _ => return None,
    })
}

/// The URL, if it is an absolute `http` or `https` URL.
fn web_uri(url: &str) -> Option<Uri> {
    let uri: Uri = url.parse().ok()?;
    let scheme = uri.scheme_str()?;
    (matches!(scheme, "http" | "https") && uri.host().is_some_and(|h| !h.is_empty())).then_some(uri)
}

/// Resolves a `Location` against the URL it came from (RFC 3986 §5.2,
/// without removing dot segments, which the server sees as sent).
fn resolve(base: &Uri, location: &str) -> Option<String> {
    let location = location.trim();
    let scheme = base.scheme_str()?;
    let authority = base.authority()?.as_str();
    let has_scheme = location.split_once(':').is_some_and(|(s, _)| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    });
    Some(if has_scheme {
        location.to_owned()
    } else if location.starts_with("//") {
        format!("{scheme}:{location}")
    } else if location.starts_with('/') {
        format!("{scheme}://{authority}{location}")
    } else if location.starts_with('?') {
        format!("{scheme}://{authority}{}{location}", base.path())
    } else {
        let path = base.path();
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        let dir = if dir.is_empty() { "/" } else { dir };
        format!("{scheme}://{authority}{dir}{location}")
    })
}

fn failed(timed_out: bool) -> HttpDone {
    HttpDone {
        successful: false,
        status: 0,
        timed_out,
    }
}

/// Whether an error is a timeout running out.
fn is_timeout(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(io) => io.kind() == std::io::ErrorKind::TimedOut,
        _ => false,
    }
}

/// Makes the request. `emit` returns false when nobody listens any
/// more, which stops the request, as `stop` does.
pub fn run(
    agent: &ureq::Agent,
    job: Job,
    stop: &CancelToken,
    mut emit: impl FnMut(Event) -> bool,
) -> HttpDone {
    let r = job.request;
    let Some(mut method) = method(r.method) else {
        tracing::debug!(method = r.method, "an HTTP request with an unknown method");
        return failed(false);
    };
    let Some(mut uri) = web_uri(&r.url) else {
        tracing::debug!("an HTTP request for a URL that is not http or https");
        return failed(false);
    };
    let mut headers = http::HeaderMap::new();
    for h in &r.headers {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(h.name.as_bytes()),
            HeaderValue::from_bytes(&h.value),
        ) else {
            return failed(false);
        };
        headers.append(name, value);
    }
    if !headers.contains_key(header::USER_AGENT) {
        let ua = if r.user_agent_info.is_empty() {
            format!("{USER_AGENT} ({})", job.app)
        } else {
            format!("{USER_AGENT} ({};{})", job.app, r.user_agent_info)
        };
        match HeaderValue::from_str(&ua) {
            Ok(v) => {
                headers.insert(header::USER_AGENT, v);
            }
            Err(_) => return failed(false),
        }
    }
    let game_cookie = headers.remove(header::COOKIE);
    let mut body = r.body;
    let activity =
        Some(Duration::from_millis(u64::from(r.activity_timeout_ms))).filter(|d| !d.is_zero());
    let deadline = (r.absolute_timeout_ms > 0)
        .then(|| Instant::now() + Duration::from_millis(u64::from(r.absolute_timeout_ms)));

    for hop in 0..=MAX_REDIRECTS {
        if stop.is_cancelled() {
            return failed(false);
        }
        tracing::debug!(
            hop,
            %method,
            scheme = uri.scheme_str().unwrap_or(""),
            host = uri.host().unwrap_or(""),
            path = uri.path(),
            "a game's HTTP request"
        );
        let mut request_headers = headers.clone();
        let jar_cookies = job.jar.as_ref().and_then(|j| {
            j.lock()
                .unwrap_or_else(|e| e.into_inner())
                .header(&uri, cookies::now())
        });
        let cookie = match (&game_cookie, jar_cookies) {
            (Some(g), Some(j)) => {
                let mut v = g.as_bytes().to_vec();
                v.extend_from_slice(b"; ");
                v.extend_from_slice(j.as_bytes());
                HeaderValue::from_bytes(&v).ok()
            }
            (Some(g), None) => Some(g.clone()),
            (None, Some(j)) => HeaderValue::from_str(&j).ok(),
            (None, None) => None,
        };
        if let Some(c) = cookie {
            request_headers.insert(header::COOKIE, c);
        }
        let global = match deadline {
            Some(d) => match d.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => Some(left),
                _ => return failed(true),
            },
            None => None,
        };
        let mut builder = http::Request::builder()
            .method(method.clone())
            .uri(uri.clone());
        if let Some(h) = builder.headers_mut() {
            *h = request_headers;
        }
        let bodiless = body.is_empty()
            && matches!(
                method,
                Method::GET | Method::HEAD | Method::DELETE | Method::OPTIONS
            );
        let result = if bodiless {
            builder.body(()).map(|req| {
                let req = agent
                    .configure_request(req)
                    .timeout_global(global)
                    .timeout_resolve(activity)
                    .timeout_connect(activity)
                    .timeout_per_read(activity)
                    .timeout_per_write(activity)
                    .build();
                agent.run(req)
            })
        } else {
            builder.body(body.clone()).map(|req| {
                let req = agent
                    .configure_request(req)
                    .timeout_global(global)
                    .timeout_resolve(activity)
                    .timeout_connect(activity)
                    .timeout_per_read(activity)
                    .timeout_per_write(activity)
                    .build();
                agent.run(req)
            })
        };
        let response = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::debug!("a game's HTTP request failed: {e}");
                return failed(is_timeout(&e));
            }
            Err(e) => {
                tracing::debug!("a game's HTTP request is malformed: {e}");
                return failed(false);
            }
        };
        if let Some(jar) = &job.jar {
            let mut jar = jar.lock().unwrap_or_else(|e| e.into_inner());
            if jar.allow_modify {
                for v in response.headers().get_all(header::SET_COOKIE) {
                    if let Ok(v) = std::str::from_utf8(v.as_bytes()) {
                        jar.store(&uri, v, cookies::now());
                    }
                }
            }
        }
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|l| l.to_str().ok())
            .map(str::to_owned);
        if let (true, Some(location)) = (matches!(status, 301 | 302 | 303 | 307 | 308), location)
            && hop < MAX_REDIRECTS
        {
            let Some(next) = resolve(&uri, &location).as_deref().and_then(web_uri) else {
                tracing::debug!(
                    "a game's HTTP request was redirected to a URL that is not http or https"
                );
                return failed(false);
            };
            if status == 303 && method != Method::HEAD
                || matches!(status, 301 | 302) && method == Method::POST
            {
                method = Method::GET;
                body.clear();
                headers.remove(header::CONTENT_TYPE);
                headers.remove(header::CONTENT_LENGTH);
            }
            if next.host() != uri.host() {
                headers.remove(header::AUTHORIZATION);
            }
            uri = next;
            continue;
        }
        if matches!(status, 301 | 302 | 303 | 307 | 308) && hop == MAX_REDIRECTS {
            tracing::debug!("a game's HTTP request was redirected more than {MAX_REDIRECTS} times");
            return failed(false);
        }
        return read(r.handle, r.streaming, response, stop, &mut emit);
    }
    failed(false)
}

/// Sends the response's headers and body.
fn read(
    handle: u32,
    streaming: bool,
    response: http::Response<ureq::Body>,
    stop: &CancelToken,
    emit: &mut impl FnMut(Event) -> bool,
) -> HttpDone {
    let status = u32::from(response.status().as_u16());
    let length = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let headers = response
        .headers()
        .iter()
        .map(|(n, v)| HttpHeader {
            name: n.as_str().to_owned(),
            value: v.as_bytes().to_vec(),
        })
        .collect();
    if !emit(Event::Headers(HttpHeaders {
        handle,
        status,
        headers,
        content_length: length.unwrap_or(0),
        has_length: length.is_some(),
    })) {
        return failed(false);
    }
    let limit = if streaming { u64::MAX } else { MAX_BODY + 1 };
    let mut reader = response
        .into_body()
        .into_with_config()
        .limit(limit)
        .reader();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if stop.is_cancelled() {
            return failed(false);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let e = ureq::Error::from(e);
                tracing::debug!("reading a game's HTTP response failed: {e}");
                return failed(is_timeout(&e));
            }
        };
        total += n as u64;
        if !streaming && total > MAX_BODY {
            tracing::info!("a game's HTTP response is larger than {MAX_BODY} bytes; it fails");
            return failed(false);
        }
        if !emit(Event::Data(buf[..n].to_vec())) {
            return failed(false);
        }
    }
    HttpDone {
        successful: true,
        status,
        timed_out: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_resolve_against_the_url() {
        let base: Uri = "https://a.example/x/y?q=1".parse().unwrap();
        let r = |l| resolve(&base, l).unwrap();
        assert_eq!(r("http://b.example/z"), "http://b.example/z");
        assert_eq!(r("//c.example/z"), "https://c.example/z");
        assert_eq!(r("/z"), "https://a.example/z");
        assert_eq!(r("z?w"), "https://a.example/x/z?w");
        assert_eq!(r("?w"), "https://a.example/x/y?w");
    }

    #[test]
    fn only_web_urls_are_made() {
        assert!(web_uri("https://a.example/").is_some());
        assert!(web_uri("http://127.0.0.1:8/x").is_some());
        assert!(web_uri("file:///etc/passwd").is_none());
        assert!(web_uri("ftp://a.example/").is_none());
        assert!(web_uri("/relative").is_none());
    }
}
