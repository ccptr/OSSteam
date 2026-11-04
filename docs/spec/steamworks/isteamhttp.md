# ISteamHTTP

| | |
|---|---|
| Status | Implemented: HTTP and HTTPS requests that the daemon makes for a game, streamed responses, and cookie containers |
| Requirements | SPEC §12.3, GIPC-1, SEC-3, SEC-4, FAITH-1 to FAITH-3 |
| Versions | `STEAMHTTP_INTERFACE_VERSION001` to `003` (`vtables.txt`), reached through `ISteamClient::GetISteamHTTP` |
| Code | `crates/ossteam-steamclient/src/http.rs`; the daemon's side in `crates/ossteamd/src/core/http.rs` and `crates/ossteamd/src/http.rs` |

A game asks Steam to make an HTTP request for it and hands it the
response. Counter-Strike 2's networking library fetches its relay
config this way (`isteamnetworkingsocketsserialized.md` §6), and its
client fetches news, leaderboards and the like. The shim keeps each
request's settings and its response; the daemon makes the request, as
the official client does in its own process (the shim may not link an
HTTP or TLS stack, SPEC WS-2).

**Verification.**

- **Slots, arguments, enums and callbacks:** the SDK headers that
  Proton's `lsteamclient` carries, and the struct layouts it generates
  for each platform (facts only, `research-sources.md`).
- **What each method does:** the comments of those headers, unless a
  line says otherwise.
- **The official client's behaviour:** read from its `steamclient.so`
  (structure only). Its adapter classes for the three versions pass
  every slot unchanged to one internal interface; the internal one has
  one more method, `SetHTTPRequestRedirectsEnabled`, which games cannot
  reach. The User-Agent (§3) and the empty URL (§1) come from its code.

What these sources do not show is marked *unverified*.

## 1. Methods

`003` (the slot numbers of `002` are the same; `001` lacks
`SendHTTPRequestAndStreamResponse` and `GetHTTPStreamingResponseBodyData`,
and ends after `SetHTTPRequestRawPostBody`):

| Slot | Method | What it does |
|---|---|---|
| 0 | `HTTPRequestHandle CreateHTTPRequest(EHTTPMethod method, const char *url)` | a new request (§2); 0 when `url` is null or empty, as in the official client, when it is not UTF-8, or when the pipe is closed |
| 1 | `bool SetHTTPRequestContextValue(h, uint64 context)` | the value the callbacks carry; may be set until the request completes |
| 2 | `bool SetHTTPRequestNetworkActivityTimeout(h, uint32 seconds)` | how long the request may wait for the network at any one time; 60 s when not set |
| 3 | `bool SetHTTPRequestHeaderValue(h, const char *name, const char *value)` | sets a request header, replacing one of the same name; false when the name is not an HTTP token or the value holds CR, LF or NUL |
| 4 | `bool SetHTTPRequestGetOrPostParameter(h, const char *name, const char *value)` | adds a parameter: to the body of a POST, otherwise to the query (§2) |
| 5 | `bool SendHTTPRequest(h, SteamAPICall_t *call)` | sends it (§4); the result is `HTTPRequestCompleted_t` |
| 6 | `bool SendHTTPRequestAndStreamResponse(h, SteamAPICall_t *call)` | sends it and streams the response (§5) |
| 7 | `bool DeferHTTPRequest(h)` | moves a waiting request to the back of the daemon's queue (§6) |
| 8 | `bool PrioritizeHTTPRequest(h)` | moves it to the front |
| 9 | `bool GetHTTPResponseHeaderSize(h, const char *name, uint32 *size)` | whether the response has the header, and its value's size with a NUL |
| 10 | `bool GetHTTPResponseHeaderValue(h, const char *name, uint8 *buf, uint32 cb)` | copies the value, NUL-terminated; false when it does not fit |
| 11 | `bool GetHTTPResponseBodySize(h, uint32 *size)` | the body's size, once the request completed |
| 12 | `bool GetHTTPResponseBodyData(h, uint8 *buf, uint32 cb)` | copies the body; false for a streamed request or a smaller buffer |
| 13 | `bool GetHTTPStreamingResponseBodyData(h, uint32 offset, uint8 *buf, uint32 cb)` | copies one received chunk (§5) |
| 14 | `bool ReleaseHTTPRequest(h)` | forgets the request, and cancels it if it is still running (§4) |
| 15 | `bool GetHTTPDownloadProgressPct(h, float *pct)` | 0 to 100: the body received out of its `Content-Length`; 0 while that is not known |
| 16 | `bool SetHTTPRequestRawPostBody(h, const char *contentType, uint8 *body, uint32 cb)` | the whole body and its `Content-Type`; false for GET and HEAD, or when parameters went to the body already |
| 17 | `HTTPCookieContainerHandle CreateCookieContainer(bool allowResponsesToModify)` | a cookie container (§7); 0 when the pipe is closed |
| 18 | `bool ReleaseCookieContainer(c)` | forgets it |
| 19 | `bool SetCookie(c, const char *host, const char *url, const char *cookie)` | adds a cookie, as if `url` on `host` had sent `Set-Cookie: cookie` |
| 20 | `bool SetHTTPRequestCookieContainer(h, c)` | the container the request takes its cookies from |
| 21 | `bool SetHTTPRequestUserAgentInfo(h, const char *info)` | adds to the User-Agent (§3) |
| 22 | `bool SetHTTPRequestRequiresVerifiedCertificate(h, bool require)` | accepted and ignored: certificates are always checked (SEC-3, §4) |
| 23 | `bool SetHTTPRequestAbsoluteTimeoutMS(h, uint32 ms)` | how long the whole request may take; 0 for no limit, the default |
| 24 | `bool GetHTTPRequestWasTimedOut(h, bool *timedOut)` | whether it failed by running out of time |

Every method that takes a request returns false for a handle it does
not know. Slots 1 to 4, 16 and 20 to 23 return false once the request
was sent, and slots 7 and 8 before it was sent or after it completed.
Slots 9 and 10 answer once the response's headers arrived, slots 11,
12 and 24 once it completed. `EHTTPMethod`: 1 GET, 2 HEAD, 3 POST,
4 PUT, 5 DELETE, 6 OPTIONS, 7 PATCH; a request with another method
fails when it is sent.

*Unverified:* whether the official client counts the NUL in slot 9;
whether slot 12 takes a buffer larger than the body (ossteam takes it
and copies the body); where parameters go for methods other than GET and
POST (ossteam: the query); whether a raw body can be set on DELETE or
OPTIONS (ossteam: yes); whether parameters may follow a raw body
(ossteam: not for POST).

## 2. Requests

Handles count up from 1 in each process; so do cookie containers. A
request keeps its pipe: the one of the interface object that created it.

The shim builds what it sends:

- **Parameters** are encoded as `name=value` pairs joined by `&`, each
  byte other than `A-Z a-z 0-9 - . _ ~` written as `%XX`. For a POST they
  are the body, with `Content-Type: application/x-www-form-urlencoded`
  unless the game set one; otherwise they are added to the URL's query.
- **Headers** are the game's, in the order it set them.

*Unverified:* the official client's encoding of spaces (`+` or `%20`).

## 3. What the daemon sends

The daemon makes the request with ossteam's HTTP client: rustls with
the platform verifier (SEC-3). Only `http` and `https` URLs are made,
the first and every redirect; any other request fails.

- **User-Agent:** `Valve/Steam HTTP Client 1.0 (<app>)`, where `<app>`
  is the connection's app id; after `SetHTTPRequestUserAgentInfo(info)`
  with a non-empty `info`, `Valve/Steam HTTP Client 1.0 (<app>;<info>)`.
  This is the official client's format, read from its code; that the
  number is the app id is inferred from the code that fills it in. A
  `User-Agent` the game set with slot 3 wins.
- **Accept and Accept-Encoding:** `*/*` and `gzip`, unless the game set
  them. A response with `Content-Encoding: gzip` is unpacked, and the
  game sees neither that header nor its `Content-Length`. *Unverified:*
  what the official client offers and unpacks.
- **Redirects:** 301, 302, 303, 307 and 308 are followed, at most 10.
  303, and 301 or 302 after a POST, become a GET without a body; the
  others keep the method and body. A redirect to another host drops the
  `Authorization` header. The game sees only the last response.
  *Unverified:* the official client's limit and rules.
- **Cookies** come from the request's container (§7), for every hop.
- **Timeouts:** the activity timeout bounds DNS, connecting, and each
  read and write; the absolute timeout bounds the whole request.

A cached response, as the SDK describes for Steam's offline mode, is
never used: ossteam has no offline mode yet, nor an HTTP cache. A
`Cache-Control` header the game sets goes to the server as it is.
Counter-Strike 2 logs its first relay-config request as an
"only-if-cached" attempt; ossteam makes it like any other, and the game
loads the config from the answer (§9). *Unverified:* what the official
client's cache answers to such a request.

## 4. Sending and completing

`SendHTTPRequest` sends `HttpSend` (§8) and returns the call handle
through `call` (which may be null). It returns false for an unknown
handle, a request already sent, or a closed pipe. The request completes
with `HTTPRequestCompleted_t` (2101), the API call's result:

| Field | Linux and macOS | Windows | Meaning |
|---|---|---|---|
| `HTTPRequestHandle m_hRequest` | 0 | 0 | the request |
| `uint64 m_ulContextValue` | 4 | 8 | its context value |
| `bool m_bRequestSuccessful` | 12 | 16 | a response came, whatever its status |
| `EHTTPStatusCode m_eStatusCode` | 16 | 20 | its status, or 0 |
| `uint32 m_unBodySize` | 20 | 24 | its body's size |
| size | 24 | 32 | |

Callbacks are packed to 4 on Linux and macOS and to 8 on Windows
(`steamworks-abi.md` §2.2). SDKs before 1.32 (`001` and the first
`002`) have no `m_unBodySize`, a structure of 20 and 24 bytes. Their
`steam_api` asks for the shorter size, and the shim hands it the same
result without the last field: this is the only result that may be
read at a size other than its own (`steamworks-abi.md` §2.5).
*Unverified:* how the official client serves those games.

A request is unsuccessful, with status 0, when no response came: the
network failed, a timeout ran out, the certificate did not verify, the
URL or method was not usable, a redirect went too far, a limit of §6
was passed, or the daemon closed. `GetHTTPRequestWasTimedOut` then says
whether a timeout ran out.

A certificate is checked even after
`SetHTTPRequestRequiresVerifiedCertificate(false)`: SEC-3 does not let
it be turned off. A game that talks to a server with a certificate the
platform does not trust gets an unsuccessful request where the official
client would have made it.

`ReleaseHTTPRequest` on a request still running cancels it: the daemon
stops it, and its call never completes, as the official client cancels
the callback of a request it destroys (a message in its code says so).
*Unverified:* what `IsAPICallCompleted` says about that call there.

## 5. Streaming

A request sent with `SendHTTPRequestAndStreamResponse` also gets these
callbacks, queued on its pipe as they come:

| Callback | Fields (Linux and macOS / Windows offsets) | Size |
|---|---|---|
| `HTTPRequestHeadersReceived_t` (2102) | `m_hRequest` 0/0, `m_ulContextValue` 4/8 | 12 / 16 |
| `HTTPRequestDataReceived_t` (2103) | `m_hRequest` 0/0, `m_ulContextValue` 4/8, `uint32 m_cOffset` 12/16, `uint32 m_cBytesReceived` 16/20 | 20 / 24 |

Each `HTTPRequestDataReceived_t` names one chunk: its offset in the body
and its size. `GetHTTPStreamingResponseBodyData` copies a chunk when
`offset` and `cb` are exactly a chunk's, and forgets it then; the
completion's `m_unBodySize` is the whole body's size. Headers can be
read once 2102 came. *Unverified:* whether the official client lets a
chunk be read twice, and how large its chunks are (ossteam: what one
read from the network gave, at most 256 KiB).

## 6. Limits (SEC-4)

| Limit | Value | Past it |
|---|---|---|
| Requests running at once, per connection | 8 | the others wait in the daemon's queue, in the order sent; slots 7 and 8 move a waiting one |
| Requests waiting, per connection | 256 | the request is unsuccessful at once |
| Request body | 15 MiB (the game IPC frame, `steamworks-abi.md` §3.2) | `SendHTTPRequest` returns false |
| URL and headers together | 64 KiB | `SendHTTPRequest` returns false |
| Response headers | 64 KiB | unsuccessful |
| Response body, not streamed | 256 MiB | unsuccessful |
| Chunks received and not read, streamed | 256 MiB | the shim cancels the request, which completes unsuccessful |

The limits are ossteam's; the official client's are *unverified*. A
cancelled request keeps its place among the 8 until the daemon has
stopped it, at most one activity timeout later.

## 7. Cookie containers

A container is a cookie jar that lives in the daemon for the process
that made it, keyed by the process and the container's handle, and goes
when the process's last connection closes. It holds cookies as RFC 6265
§5 stores them: name, value, domain (or the host, for a host-only
cookie), path, expiry, and the Secure flag. Cookies without an expiry
last as long as the container; nothing is written to disk, as the SDK
says.

- `SetCookie(c, host, url, cookie)` parses `cookie` as a `Set-Cookie`
  value received from `url`; `host` is used when `url` is empty or has
  no host. It returns false for an unknown container or a cookie that
  does not parse or is refused.
- A request with a container sends its matching cookies in a `Cookie`
  header on every hop (RFC 6265 §5.4), Secure ones only over HTTPS, and,
  when the container allows it, stores every `Set-Cookie` of every hop.
- A `Domain` attribute that the host does not domain-match, or that has
  no dot inside it, refuses the cookie: ossteam has no public suffix
  list.
- At most 3000 cookies per container and 4096 bytes per cookie; past
  3000, the oldest goes. A process may have 1024 containers; the daemon
  refuses another, which then holds no cookies.

*Unverified:* the official client's handling of each of these; whether
`host` matters when `url` has a host.

## 8. Game IPC

| Tag | Request | Response |
|---|---|---|
| 17 | `HttpSend{handle: u32, method: u32, url: string, headers: list<HttpHeader>, body: list<u8>, user_agent_info: string, activity_timeout_ms: u32, absolute_timeout_ms: u32, streaming: bool, cookies: u32}` | `HttpDone{successful: bool, status: u32, timed_out: bool}` (tag 15) |
| 18 | `HttpCancel{handle: u32}` | `Done{}` |
| 19 | `HttpReorder{handle: u32, first: bool}` | `Done{}` |
| 20 | `HttpCookieContainer{handle: u32, allow_modify: bool}` | `Done{}` |
| 21 | `HttpReleaseCookies{handle: u32}` | `Done{}` |
| 22 | `HttpSetCookie{handle: u32, host: string, url: string, cookie: string}` | `Done{}`, or `Error` when the cookie is refused |

`HttpHeader` is `{name: string, value: list<u8>}`: a value need not be
UTF-8. `cookies` is 0 for no container; `absolute_timeout_ms` is 0 for
none.

The daemon sends, as notices (`NoticeBody` tags 4 and 5), before
`HttpDone`:

- `HttpHeaders{handle: u32, status: u32, headers: list<HttpHeader>,
  content_length: u64, has_length: bool}` when the last response's
  headers arrived;
- `HttpData{handle: u32, data: list<u8>}` for each chunk of its body.

Everything for one request travels on the connection that sent it, in
this order. `HttpCancel` and `HttpReorder` act only on that connection's
requests, and the cookie requests only on the process's containers.

These requests act for no app: any connection may make them, as any
process may make its own. They give a game nothing it could not reach
itself, but the requests go out from the daemon's process.

## 9. Checks

| Item | Status |
|---|---|
| Slot layouts and callback layouts | from the SDK headers and Proton's generated layouts |
| The official adapters pass every slot through unchanged | read from its `steamclient.so` |
| The User-Agent | read from its `steamclient.so` |
| Counter-Strike 2 through `ISteamHTTP` | done 2026-11-04, test account, main menu only: it fetched its relay config (`api.steampowered.com/ISteamApps/GetSDRConfig/v1`) twice, Steam's news and the HLTV event feed. Its console logged "Got SDR network config. Loaded revision … OK", then pings to 26 relays and "avail=OK". No request failed. |
| The official client's limits, encoding, redirects and cookie rules | unverified |
