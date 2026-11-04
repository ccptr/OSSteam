// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamHTTP` (`docs/spec/steamworks/isteamhttp.md`): the shim keeps
//! each request's settings and its response (§1, §2); the daemon makes
//! the request (§3, §8).

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{CStr, c_void};
use std::sync::{Mutex, MutexGuard};

use ossteam_gameipc::{
    HttpCancelRequest, HttpCookieContainerRequest, HttpData, HttpDone, HttpHeader, HttpHeaders,
    HttpReleaseCookiesRequest, HttpReorderRequest, HttpSendRequest, HttpSetCookieRequest,
    RequestBody, ResponseBody,
};

use crate::abi::put;
use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe::{self, Pipe};

/// `EHTTPMethod`.
const GET: u32 = 1;
const HEAD: u32 = 2;
const POST: u32 = 3;

/// The activity timeout when the game sets none (§1), in seconds.
const ACTIVITY_TIMEOUT: u32 = 60;
/// The limits of §6.
const MAX_BODY: usize = 15 * 1024 * 1024;
const MAX_HEAD: usize = 64 * 1024;
const MAX_UNREAD: usize = 256 * 1024 * 1024;

#[derive(PartialEq, Eq)]
enum Phase {
    New,
    Sent,
    Done,
}

/// The last response's headers.
struct Head {
    headers: Vec<(String, Vec<u8>)>,
    length: Option<u64>,
}

struct Request {
    pipe: i32,
    method: u32,
    url: String,
    context: u64,
    activity: u32,
    absolute_ms: u32,
    headers: Vec<(String, Vec<u8>)>,
    params: Vec<(Vec<u8>, Vec<u8>)>,
    raw_body: Option<Vec<u8>>,
    cookies: u32,
    user_agent_info: String,
    phase: Phase,
    streaming: bool,
    call: u64,
    head: Option<Head>,
    /// The body, when not streamed.
    body: Vec<u8>,
    /// Streamed chunks not read yet, by offset.
    chunks: VecDeque<(u32, Vec<u8>)>,
    unread: usize,
    received: u64,
    done: Option<HttpDone>,
}

impl Request {
    fn new(pipe: i32, method: u32, url: String) -> Self {
        Self {
            pipe,
            method,
            url,
            context: 0,
            activity: ACTIVITY_TIMEOUT,
            absolute_ms: 0,
            headers: Vec::new(),
            params: Vec::new(),
            raw_body: None,
            cookies: 0,
            user_agent_info: String::new(),
            phase: Phase::New,
            streaming: false,
            call: 0,
            head: None,
            body: Vec::new(),
            chunks: VecDeque::new(),
            unread: 0,
            received: 0,
            done: None,
        }
    }

    fn set_header(&mut self, name: &str, value: Vec<u8>) {
        match self
            .headers
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(h) => h.1 = value,
            None => self.headers.push((name.to_owned(), value)),
        }
    }

    fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The parameters go to the body (§2).
    fn params_in_body(&self) -> bool {
        self.method == POST
    }

    /// What `HttpSend` carries (§2), or `None` past the limits of §6.
    fn to_send(&self, handle: u32, streaming: bool) -> Option<HttpSendRequest> {
        let mut url = self.url.clone();
        let mut headers = self.headers.clone();
        let mut body = self.raw_body.clone().unwrap_or_default();
        if !self.params.is_empty() {
            let encoded = encode_params(&self.params);
            if self.params_in_body() {
                body = encoded.into_bytes();
                if !self.has_header("Content-Type") {
                    headers.push((
                        "Content-Type".into(),
                        b"application/x-www-form-urlencoded".to_vec(),
                    ));
                }
            } else {
                let (base, fragment) = match url.find('#') {
                    Some(i) => url.split_at(i),
                    None => (url.as_str(), ""),
                };
                let sep = if base.contains('?') { '&' } else { '?' };
                url = format!("{base}{sep}{encoded}{fragment}");
            }
        }
        let head = url.len()
            + headers
                .iter()
                .map(|(n, v)| n.len() + v.len())
                .sum::<usize>();
        if body.len() > MAX_BODY || head > MAX_HEAD {
            return None;
        }
        Some(HttpSendRequest {
            handle,
            method: self.method,
            url,
            headers: headers
                .into_iter()
                .map(|(name, value)| HttpHeader { name, value })
                .collect(),
            body,
            user_agent_info: self.user_agent_info.clone(),
            activity_timeout_ms: self.activity.saturating_mul(1000),
            absolute_timeout_ms: self.absolute_ms,
            streaming,
            cookies: self.cookies,
        })
    }

    fn header(&self, name: &str) -> Option<&[u8]> {
        self.head
            .as_ref()?
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }

    fn body_size(&self) -> u32 {
        if self.streaming {
            self.received.min(u64::from(u32::MAX)) as u32
        } else {
            self.body.len() as u32
        }
    }
}

struct State {
    next_request: u32,
    next_container: u32,
    requests: BTreeMap<u32, Request>,
    /// Cookie containers and the pipe each was made on.
    containers: BTreeMap<u32, i32>,
}

static STATE: Mutex<State> = Mutex::new(State {
    next_request: 1,
    next_container: 1,
    requests: BTreeMap::new(),
    containers: BTreeMap::new(),
});

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The next handle of a counter that starts at 1 and skips 0.
fn next(counter: &mut u32) -> u32 {
    let h = *counter;
    *counter = counter.wrapping_add(1).max(1);
    h
}

/// A game's NUL-terminated bytes.
///
/// # Safety
///
/// `p` is null or points to a NUL-terminated string.
unsafe fn bytes<'a>(p: *mut c_void) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    Some(unsafe { CStr::from_ptr(p.cast()) }.to_bytes())
}

/// # Safety
///
/// As `bytes`.
unsafe fn text<'a>(p: *mut c_void) -> Option<&'a str> {
    // SAFETY: as the caller promises.
    unsafe { bytes(p) }.and_then(|b| std::str::from_utf8(b).ok())
}

/// An HTTP token (RFC 9110 §5.6.2).
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// `name=value&...`, percent-encoded (§2).
fn encode_params(params: &[(Vec<u8>, Vec<u8>)]) -> String {
    fn encode(out: &mut String, b: &[u8]) {
        for &c in b {
            if c.is_ascii_alphanumeric() || b"-._~".contains(&c) {
                out.push(c as char);
            } else {
                out.push_str(&format!("%{c:02X}"));
            }
        }
    }
    let mut out = String::new();
    for (i, (n, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode(&mut out, n);
        out.push('=');
        encode(&mut out, v);
    }
    out
}

/// Runs `f` on request `h` if it was not sent yet. False when there is
/// no such request.
fn with_new(h: u32, f: impl FnOnce(&mut Request) -> bool) -> bool {
    match lock().requests.get_mut(&h) {
        Some(r) if r.phase == Phase::New => f(r),
        _ => false,
    }
}

pub fn create_http_request(o: &Object, method: u32, url: *mut c_void) -> u32 {
    // SAFETY: the SDK declares a NUL-terminated URL.
    let Some(url) = (unsafe { text(url) }).filter(|u| !u.is_empty()) else {
        return 0;
    };
    if pipe::get(o.pipe).is_none_or(|p| p.is_closed()) {
        return 0;
    }
    let mut s = lock();
    let h = next(&mut s.next_request);
    s.requests
        .insert(h, Request::new(o.pipe, method, url.to_owned()));
    h
}

pub fn set_http_request_context_value(_: &Object, h: u32, context: u64) -> bool {
    match lock().requests.get_mut(&h) {
        Some(r) if r.phase != Phase::Done => {
            r.context = context;
            true
        }
        _ => false,
    }
}

pub fn set_http_request_network_activity_timeout(_: &Object, h: u32, seconds: u32) -> bool {
    with_new(h, |r| {
        r.activity = seconds;
        true
    })
}

pub fn set_http_request_header_value(
    _: &Object,
    h: u32,
    name: *mut c_void,
    value: *mut c_void,
) -> bool {
    // SAFETY: the SDK declares NUL-terminated strings.
    let (name, value) = unsafe { (text(name), bytes(value)) };
    let (Some(name), Some(value)) = (name, value) else {
        return false;
    };
    if !is_token(name) || value.iter().any(|&b| b == b'\r' || b == b'\n') {
        return false;
    }
    with_new(h, |r| {
        r.set_header(name, value.to_vec());
        true
    })
}

pub fn set_http_request_get_or_post_parameter(
    _: &Object,
    h: u32,
    name: *mut c_void,
    value: *mut c_void,
) -> bool {
    // SAFETY: the SDK declares NUL-terminated strings.
    let (name, value) = unsafe { (bytes(name), bytes(value)) };
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return false;
    };
    let value = value.unwrap_or_default();
    with_new(h, |r| {
        if r.params_in_body() && r.raw_body.is_some() {
            return false;
        }
        r.params.push((name.to_vec(), value.to_vec()));
        true
    })
}

fn send(h: u32, call: *mut c_void, streaming: bool) -> bool {
    let mut s = lock();
    let Some(r) = s.requests.get_mut(&h).filter(|r| r.phase == Phase::New) else {
        return false;
    };
    let Some(p) = pipe::get(r.pipe).filter(|p| !p.is_closed()) else {
        return false;
    };
    let Some(body) = r.to_send(h, streaming) else {
        log!(Info, "an HTTP request is larger than ossteam sends");
        return false;
    };
    // The lock is held while sending, so that the daemon's answers, read
    // on the pipe's thread, find the request sent.
    let (empty, short) = callbacks::http_request_completed(0, 0, false, 0, 0);
    let handle = p.begin_call_short(
        RequestBody::HttpSend(body),
        callbacks::HTTP_REQUEST_COMPLETED,
        empty.len(),
        Some(short),
        Box::new(move |_, answer| {
            let ResponseBody::HttpDone(d) = answer else {
                log!(Info, "HTTP request {h}: the daemon sent {answer:?}");
                return None;
            };
            Some(finish(h, d.clone()))
        }),
    );
    if handle == 0 {
        return false;
    }
    r.phase = Phase::Sent;
    r.streaming = streaming;
    r.call = handle;
    drop(s);
    // SAFETY: the SDK declares a `SteamAPICall_t *`; `put` skips null.
    unsafe { put(call, handle) };
    true
}

/// Marks request `h` done and makes its `HTTPRequestCompleted_t`.
fn finish(h: u32, d: HttpDone) -> Vec<u8> {
    let mut s = lock();
    let Some(r) = s.requests.get_mut(&h) else {
        return callbacks::http_request_completed(h, 0, false, 0, 0).0;
    };
    let (successful, status) = (d.successful, if d.successful { d.status } else { 0 });
    if !successful {
        r.body.clear();
    }
    r.phase = Phase::Done;
    r.done = Some(d);
    let size = if successful { r.body_size() } else { 0 };
    callbacks::http_request_completed(h, r.context, successful, status, size).0
}

pub fn send_http_request(_: &Object, h: u32, call: *mut c_void) -> bool {
    send(h, call, false)
}

pub fn send_http_request_and_stream_response(_: &Object, h: u32, call: *mut c_void) -> bool {
    send(h, call, true)
}

fn reorder(h: u32, first: bool) -> bool {
    let s = lock();
    let Some(r) = s.requests.get(&h).filter(|r| r.phase == Phase::Sent) else {
        return false;
    };
    let Some(p) = pipe::get(r.pipe) else {
        return false;
    };
    p.send_then(
        RequestBody::HttpReorder(HttpReorderRequest { handle: h, first }),
        Box::new(|_, _| {}),
    )
}

pub fn defer_http_request(_: &Object, h: u32) -> bool {
    reorder(h, false)
}

pub fn prioritize_http_request(_: &Object, h: u32) -> bool {
    reorder(h, true)
}

pub fn get_http_response_header_size(
    _: &Object,
    h: u32,
    name: *mut c_void,
    size: *mut c_void,
) -> bool {
    // SAFETY: the SDK declares a NUL-terminated name.
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let s = lock();
    let Some(v) = s.requests.get(&h).and_then(|r| r.header(name)) else {
        return false;
    };
    // SAFETY: the SDK declares a `uint32 *`; `put` skips null.
    unsafe { put(size, (v.len() + 1) as u32) };
    true
}

pub fn get_http_response_header_value(
    _: &Object,
    h: u32,
    name: *mut c_void,
    buf: *mut c_void,
    cap: u32,
) -> bool {
    // SAFETY: the SDK declares a NUL-terminated name.
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let s = lock();
    let Some(v) = s.requests.get(&h).and_then(|r| r.header(name)) else {
        return false;
    };
    if buf.is_null() || v.len() >= cap as usize {
        return false;
    }
    // SAFETY: the game passes `cap` bytes, which hold the value and a NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(v.as_ptr(), buf.cast::<u8>(), v.len());
        buf.cast::<u8>().add(v.len()).write(0);
    }
    true
}

pub fn get_http_response_body_size(_: &Object, h: u32, size: *mut c_void) -> bool {
    let s = lock();
    let Some(r) = s.requests.get(&h).filter(|r| r.phase == Phase::Done) else {
        return false;
    };
    // SAFETY: the SDK declares a `uint32 *`; `put` skips null.
    unsafe { put(size, r.body_size()) };
    true
}

pub fn get_http_response_body_data(_: &Object, h: u32, buf: *mut c_void, cap: u32) -> bool {
    let s = lock();
    let Some(r) = s
        .requests
        .get(&h)
        .filter(|r| r.phase == Phase::Done && !r.streaming)
    else {
        return false;
    };
    if buf.is_null() || (cap as usize) < r.body.len() {
        return false;
    }
    // SAFETY: the game passes `cap` bytes, which the body fits in.
    unsafe { std::ptr::copy_nonoverlapping(r.body.as_ptr(), buf.cast::<u8>(), r.body.len()) };
    true
}

pub fn get_http_streaming_response_body_data(
    _: &Object,
    h: u32,
    offset: u32,
    buf: *mut c_void,
    cap: u32,
) -> bool {
    let mut s = lock();
    let Some(r) = s.requests.get_mut(&h).filter(|r| r.streaming) else {
        return false;
    };
    let Some(i) = r
        .chunks
        .iter()
        .position(|(o, c)| *o == offset && c.len() == cap as usize)
    else {
        return false;
    };
    if buf.is_null() {
        return false;
    }
    let (_, chunk) = r.chunks.remove(i).expect("found");
    r.unread -= chunk.len();
    // SAFETY: the game passes `cap` bytes, the chunk's size.
    unsafe { std::ptr::copy_nonoverlapping(chunk.as_ptr(), buf.cast::<u8>(), chunk.len()) };
    true
}

pub fn release_http_request(_: &Object, h: u32) -> bool {
    let mut s = lock();
    let Some(r) = s.requests.remove(&h) else {
        return false;
    };
    if r.phase == Phase::Sent
        && let Some(p) = pipe::get(r.pipe)
    {
        cancel(&p, h, r.call);
    }
    true
}

/// Stops a request that was sent: its call never completes (§4).
fn cancel(p: &Pipe, h: u32, call: u64) {
    p.forget_call(call);
    p.send_then(
        RequestBody::HttpCancel(HttpCancelRequest { handle: h }),
        Box::new(|_, _| {}),
    );
}

pub fn get_http_download_progress_pct(_: &Object, h: u32, pct: *mut c_void) -> bool {
    let s = lock();
    let Some(r) = s.requests.get(&h) else {
        return false;
    };
    let value = match r.head.as_ref().and_then(|h| h.length) {
        Some(len) if len > 0 => (r.received as f64 * 100.0 / len as f64).min(100.0) as f32,
        _ => 0.0,
    };
    // SAFETY: the SDK declares a `float *`; `put` skips null.
    unsafe { put(pct, value) };
    true
}

pub fn set_http_request_raw_post_body(
    _: &Object,
    h: u32,
    content_type: *mut c_void,
    body: *mut c_void,
    len: u32,
) -> bool {
    let len = len as usize;
    if body.is_null() && len > 0 {
        return false;
    }
    // SAFETY: the SDK declares a NUL-terminated content type.
    let content_type = unsafe { bytes(content_type) };
    if content_type.is_some_and(|c| c.iter().any(|&b| b == b'\r' || b == b'\n')) {
        return false;
    }
    with_new(h, |r| {
        if r.method == GET || r.method == HEAD || (r.params_in_body() && !r.params.is_empty()) {
            return false;
        }
        let data = if len == 0 {
            Vec::new()
        } else {
            // SAFETY: the SDK declares `len` bytes of body.
            unsafe { std::slice::from_raw_parts(body.cast::<u8>(), len) }.to_vec()
        };
        r.raw_body = Some(data);
        if let Some(c) = content_type.filter(|c| !c.is_empty()) {
            r.set_header("Content-Type", c.to_vec());
        }
        true
    })
}

pub fn create_cookie_container(o: &Object, allow_modify: bool) -> u32 {
    let Some(p) = pipe::get(o.pipe).filter(|p| !p.is_closed()) else {
        return 0;
    };
    let mut s = lock();
    let handle = next(&mut s.next_container);
    let sent = p.send_then(
        RequestBody::HttpCookieContainer(HttpCookieContainerRequest {
            handle,
            allow_modify,
        }),
        Box::new(|_, _| {}),
    );
    if !sent {
        return 0;
    }
    s.containers.insert(handle, o.pipe);
    handle
}

pub fn release_cookie_container(_: &Object, c: u32) -> bool {
    let Some(pipe) = lock().containers.remove(&c) else {
        return false;
    };
    if let Some(p) = pipe::get(pipe) {
        p.send_then(
            RequestBody::HttpReleaseCookies(HttpReleaseCookiesRequest { handle: c }),
            Box::new(|_, _| {}),
        );
    }
    true
}

pub fn set_cookie(
    _: &Object,
    c: u32,
    host: *mut c_void,
    url: *mut c_void,
    cookie: *mut c_void,
) -> bool {
    // SAFETY: the SDK declares NUL-terminated strings.
    let (host, url, cookie) = unsafe { (text(host), text(url), text(cookie)) };
    let Some(cookie) = cookie else {
        return false;
    };
    let Some(pipe) = lock().containers.get(&c).copied() else {
        return false;
    };
    let Some(p) = pipe::get(pipe) else {
        return false;
    };
    // Not under the lock: the answer comes on the pipe's thread.
    let answer = p.request(RequestBody::HttpSetCookie(HttpSetCookieRequest {
        handle: c,
        host: host.unwrap_or_default().to_owned(),
        url: url.unwrap_or_default().to_owned(),
        cookie: cookie.to_owned(),
    }));
    matches!(answer, Some(ResponseBody::Done(_)))
}

pub fn set_http_request_cookie_container(_: &Object, h: u32, c: u32) -> bool {
    let mut s = lock();
    if c != 0 && !s.containers.contains_key(&c) {
        return false;
    }
    match s.requests.get_mut(&h) {
        Some(r) if r.phase == Phase::New => {
            r.cookies = c;
            true
        }
        _ => false,
    }
}

pub fn set_http_request_user_agent_info(_: &Object, h: u32, info: *mut c_void) -> bool {
    // SAFETY: the SDK declares a NUL-terminated string.
    let info = unsafe { text(info) }.unwrap_or_default();
    if info.contains(['\r', '\n']) {
        return false;
    }
    with_new(h, |r| {
        r.user_agent_info = info.to_owned();
        true
    })
}

pub fn set_http_request_requires_verified_certificate(_: &Object, h: u32, require: bool) -> bool {
    with_new(h, |_| {
        if !require {
            log!(
                Info,
                "HTTP request {h} asks not to check certificates; ossteam always checks them"
            );
        }
        true
    })
}

pub fn set_http_request_absolute_timeout_ms(_: &Object, h: u32, ms: u32) -> bool {
    with_new(h, |r| {
        r.absolute_ms = ms;
        true
    })
}

pub fn get_http_request_was_timed_out(_: &Object, h: u32, timed_out: *mut c_void) -> bool {
    let s = lock();
    let Some(r) = s.requests.get(&h) else {
        return false;
    };
    let value = r.done.as_ref().is_some_and(|d| d.timed_out);
    // SAFETY: the SDK declares a `bool *`; `put` skips null.
    unsafe { put(timed_out, u8::from(value)) };
    true
}

/// `HttpHeaders` (§8): the response's headers arrived.
pub fn on_headers(p: &Pipe, n: HttpHeaders) {
    let mut s = lock();
    let Some(r) = s
        .requests
        .get_mut(&n.handle)
        .filter(|r| r.pipe == p.handle && r.phase == Phase::Sent)
    else {
        return;
    };
    r.head = Some(Head {
        headers: n.headers.into_iter().map(|h| (h.name, h.value)).collect(),
        length: n.has_length.then_some(n.content_length),
    });
    if r.streaming {
        p.push(
            callbacks::HTTP_REQUEST_HEADERS_RECEIVED,
            callbacks::http_request_headers_received(n.handle, r.context),
        );
    }
}

/// `HttpData` (§8): a chunk of the body.
pub fn on_data(p: &Pipe, n: HttpData) {
    let mut s = lock();
    let Some(r) = s
        .requests
        .get_mut(&n.handle)
        .filter(|r| r.pipe == p.handle && r.phase == Phase::Sent)
    else {
        return;
    };
    let offset = r.received.min(u64::from(u32::MAX)) as u32;
    let size = n.data.len();
    r.received += size as u64;
    if !r.streaming {
        r.body.extend_from_slice(&n.data);
        return;
    }
    r.unread += size;
    r.chunks.push_back((offset, n.data));
    p.push(
        callbacks::HTTP_REQUEST_DATA_RECEIVED,
        callbacks::http_request_data_received(n.handle, r.context, offset, size as u32),
    );
    if r.unread > MAX_UNREAD {
        log!(
            Info,
            "HTTP request {}: the game left more than {MAX_UNREAD} bytes unread; cancelling it",
            n.handle
        );
        let (handle, call, context) = (n.handle, r.call, r.context);
        r.phase = Phase::Done;
        r.chunks.clear();
        r.unread = 0;
        r.done = Some(HttpDone::default());
        drop(s);
        let (data, _) = callbacks::http_request_completed(handle, context, false, 0, 0);
        p.complete_call(call, data);
        p.send_then(
            RequestBody::HttpCancel(HttpCancelRequest { handle }),
            Box::new(|_, _| {}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_percent_encoded() {
        let p = vec![
            (b"a b".to_vec(), b"x&y=z".to_vec()),
            (b"k".to_vec(), "é~".as_bytes().to_vec()),
        ];
        assert_eq!(encode_params(&p), "a%20b=x%26y%3Dz&k=%C3%A9~");
    }

    #[test]
    fn query_parameters_go_before_the_fragment() {
        let mut r = Request::new(1, GET, "http://h/p?q=1#f".into());
        r.params.push((b"a".to_vec(), b"b".to_vec()));
        assert_eq!(r.to_send(1, false).unwrap().url, "http://h/p?q=1&a=b#f");
        let mut r = Request::new(1, POST, "http://h/p".into());
        r.params.push((b"a".to_vec(), b"b".to_vec()));
        let s = r.to_send(1, false).unwrap();
        assert_eq!(
            (s.url.as_str(), s.body.as_slice()),
            ("http://h/p", &b"a=b"[..])
        );
        assert_eq!(s.headers[0].name, "Content-Type");
    }

    #[test]
    fn tokens() {
        assert!(is_token("X-Custom_1"));
        assert!(!is_token("a b"));
        assert!(!is_token(""));
    }
}
