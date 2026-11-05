# ISteamHTTP

| | |
|---|---|
| Status | Implemented: HTTP and HTTPS requests that the daemon makes for a game, streamed responses, cookie containers, and the HTTP cache |
| Requirements | SPEC §12.3, GIPC-1, SEC-3, SEC-4, FAITH-1 to FAITH-3 |
| Versions | `STEAMHTTP_INTERFACE_VERSION001` to `003` (`vtables.txt`), reached through `ISteamClient::GetISteamHTTP` |
| Code | `crates/ossteam-steamclient/src/http.rs`; the daemon's side in `crates/ossteamd/src/core/http.rs`, `crates/ossteamd/src/http.rs` and `crates/ossteamd/src/http/cache.rs` |

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
  reach. The User-Agent (§3) and the empty URL (§1) come from its code,
  and so does what is known of its cache (§3.1): the log messages of
  its `CHTTPRequestCache`, which name the decisions it makes, and the
  code that decides what it stores, read in part. §3.1 marks the source
  of each statement.
- **Its cache on disk:** the layout of the files in its
  `appcache/httpcache/` (structure only, `official-client-files.md`).
- **What Counter-Strike 2 asks of the cache:** its
  `libsteamnetworkingsockets.so` (structure only).

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
| 22 | `bool SetHTTPRequestRequiresVerifiedCertificate(h, bool require)` | `false` makes the request without checking the server's certificate (§4) |
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
the platform verifier (SEC-3), unless the game asked not to check
certificates (§4). Only `http` and `https` URLs are made,
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

The game's `Cache-Control` header goes to the server as it is
(*unverified* for the official client). Its directives also steer the
cache (§3.1).

### 3.1 The cache

The daemon keeps an HTTP cache, and every GET and HEAD request goes
through it, on every hop. Counter-Strike 2 relies on a cache: its networking library
first asks for its relay config with
`Cache-Control: only-if-cached, max-stale=2678400`, and when the cache
answers 504 it asks again without it (§9).

**What is known of the official cache.** Each statement is marked by
its source: *read* from the official client's code (`steamclient.so`,
its `CHTTPRequestCache`), *logged* when only a log message in that code
says it, *inferred*, or *unverified*.

- *Unverified, the main uncertainty:* whether the official client's
  cache serves games' requests at all. Its code stores a response only
  when a flag on its request is set, and skips the lookup when another
  is set (*read*); what sets those flags for an `ISteamHTTP` request was
  not traced. That games' requests use it is *inferred*
  from Counter-Strike 2 asking for `only-if-cached` and falling back
  when it gets a 504.
- It stores only GET and HEAD responses, and none for a request with
  `If-Modified-Since`, `If-None-Match`, `If-Unmodified-Since`,
  `If-Match` or `If-Range`, or with some other request fields set that
  were not identified (*read*).
- It stores statuses 200, 300, 301, 308 and 410; 302 and 307 take
  another path, not traced (*read*).
- It stores no response with `Content-Range`, with a `Vary` other than
  `Accept-Encoding`, with `private` or `no-store`, none for a request
  with `no-store`, and none for a request with `Authorization` unless
  the response says `public`. It looks at `max-age`, `s-maxage`,
  `Expires`, `Date`, `ETag` and `Last-Modified` to decide (*read*; how
  it weighs them was read only in part).
- It stores nothing over 4 MiB (*read*: a size compared with 4 MiB;
  that it is the body's size is *inferred*). Its total size:
  *unverified*.
- POST, PUT, DELETE and PATCH mark what is stored for the URL to be
  removed, whatever the status (*read* from its method switch; the log
  message names only PUT, POST and DELETE).
- Its freshness lifetime is `s-maxage`, else `max-age`, else `Expires`
  minus `Date`, else a step from `Last-Modified`: 600 s, 60 s, 5 s or
  none, past one day, one hour and one minute (*read*; that the steps
  measure the time since `Last-Modified` is *inferred*, the subtraction
  was not fully traced). It logs "Computed freshness lifetime for %s is
  %lld".
- At lookup: the request's `max-age`, `min-fresh`, `max-stale` (with or
  without a value) and `only-if-cached`, and the stored response's
  `no-cache` and `must-revalidate`, steer the decision; validation sends
  `If-None-Match` and `If-Modified-Since`; a stale answer allowed by
  `max-stale` gets `Warning: 100 127.0.0.1 stale`, appended after a
  comma to any `Warning` it had; `only-if-cached` with nothing usable
  gets a 504 with a small HTML page (*logged*, the `Warning` and the
  page's shape *read*). That `no-cache` and `must-revalidate` force
  validation even while fresh is *inferred* from where the message sits.
- It keeps its files on disk in `appcache/httpcache/`, and a
  least-recently-used list (*read*).
- The headers it drops, when storing or refreshing (*read*; which of
  the two was not traced): `Connection`, `Keep-Alive`,
  `Proxy-Authenticate`, `Proxy-Authorization`, `Trailers`,
  `Transfer-Encoding`, `Upgrade` and `Proxy-Connection`.
- *Unverified:* whether it stores `Set-Cookie`; whether a stored GET
  answers a HEAD; how it handles several variants of one URL (its
  file names hold a second hash whose input is not known); whether it
  honours `Pragma: no-cache`.

**What ossteam does.** The rules are RFC 9111's for a cache shared by
the client's requests, with the official choices above where they were
read or logged. A request skips the
cache, and its response is not stored, when the game set `Range`,
`If-Match`, `If-None-Match`, `If-Modified-Since`, `If-Unmodified-Since`
or `If-Range` itself.

**Directives.** The request's `Cache-Control` (`Pragma: no-cache` when
it has none) may say `no-cache`, `no-store`, `max-age`, `min-fresh`,
`max-stale` (with or without a value) and `only-if-cached`. The
response's may say `s-maxage`, `max-age`, `no-cache`,
`must-revalidate`, `no-store`, `public` and `private`; `Expires` counts
too. Others are ignored.

**Age and freshness.** A stored response's age is computed as RFC 9111
§4.2.3 says, from its `Date` and `Age` and the times it was asked for and
received. Its freshness lifetime is its `s-maxage`, else its `max-age`,
as in the official client, else `Expires` minus `Date`, else a tenth of
`Date` minus `Last-Modified`, at most a day, else 0. That last,
heuristic, rule is ossteam's, not the official one.

**Lookup.** With a stored response for the URL whose `Vary` headers
match the request (below):

1. It must be validated when it says `no-cache` or `must-revalidate`
   (even while fresh), when the request says `no-cache`, when its age
   passes the request's `max-age`, or when its remaining freshness is
   under the request's `min-fresh`.
2. Otherwise, while fresh, it is the answer, without the network.
3. Stale, it is still the answer when the request says `max-stale`
   without a value, or with a value its staleness does not pass. It then
   carries `Warning: 100 127.0.0.1 stale`, after any `Warning` it had.
4. Otherwise it must be validated.

Validating sends the request with `If-None-Match` (its `ETag`) and
`If-Modified-Since` (its `Last-Modified`). A 304 answer refreshes the
stored response's headers with the 304's (but not `Content-Length`) and
its times, and the stored response is the answer. Any other answer
replaces it, or is stored afresh, by the rules below. A stored response
without `ETag` or `Last-Modified` cannot be validated: the request goes
out as if nothing were stored.

**`only-if-cached`.** Such a request never reaches the network. When
nothing usable is stored, or what is stored must be validated, the cache
answers itself: the request is successful, with status 504,
`Content-Type: text/html` and a short HTML page naming the status.

**An answer from the cache** looks like one from the network: its
status, its stored headers with `Age` set to its age, `HttpHeaders` with
the body's length, then the body in chunks (§5, §8). Its `Set-Cookie`
headers were not stored, so it changes no cookie container.

**Storing.** A GET or HEAD response that came from the network is
stored when:

- its status is not 1xx, 206 or 304;
- neither the request nor the response says `no-store`, and the
  response does not say `private`;
- the request has no `Authorization`, or the response says `public`;
- it has no `Vary`, or `Vary: Accept-Encoding` alone;
- it has explicit freshness (`s-maxage`, `max-age` or `Expires`), says
  `public`, or has a status cacheable by default (200, 203, 204, 300,
  301, 308, 404, 405, 410, 414, 501) and an `ETag` or `Last-Modified`;
- its body, read whole, is at most 4 MiB.

The `private`, `Authorization`, `Vary` and size rules, and `s-maxage`,
are the official client's (*read*).

It is stored without `Set-Cookie` and without the hop-by-hop headers
(`Connection`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`,
`Transfer-Encoding`, `Upgrade` and those `Connection` names). With
`Vary: Accept-Encoding`, the value the daemon sent for that header is
stored with it; a later request whose value differs does not match. One
response is kept per URL and method: the newest replaces the one
before. A HEAD response is stored apart from the GET response of the
same URL, so that it never answers a GET; whether the official client
keeps them apart is *unverified*. A stored
301 or 308 is followed like one from the network.

**Invalidation.** A PUT, POST or DELETE whose answer is 2xx or 3xx
removes what is stored for its URL, GET and HEAD, and for its
`Location` and `Content-Location` on the same host.

**On disk.** The cache lives in the daemon's cache directory (SPEC §17),
in `http/<app>/`, one file per URL named by the SHA-1 of the URL, and it
outlasts the daemon. It holds at most 256 MiB over all apps; past that,
the least recently used responses go. A file that does not read back is
removed.

**Where ossteam differs from what was read.**

- One cache per app, where the official client has one for all of its
  requests: games are untrusted (SEC-4), and with one cache a game could
  read, through `only-if-cached`, what another game fetched. A game
  therefore never sees a response that the client itself or another
  game stored. This is deliberate.
- ossteam stores the statuses RFC 9111 allows (203, 204, 404, 405, 414
  and 501 too, and 302 and 307 with explicit freshness), where the
  official client stores 200, 300, 301, 308 and 410.
- ossteam's heuristic lifetime is a tenth of the time since
  `Last-Modified`, at most a day, not the official steps.
- ossteam drops `TE`, `Trailer`, `Set-Cookie` and the headers that
  `Connection` names, which the official list does not name, and not
  `Proxy-Authenticate` or `Proxy-Authorization`.
- ossteam invalidates only after a 2xx or 3xx, not for PATCH, and also
  for `Location` and `Content-Location` (RFC 9111 §4.4); the official
  client invalidates the URL for PATCH too, whatever the status, and
  whether it removes the other two is *unverified*.
- The 256 MiB total is ossteam's.

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

Certificates are checked with the platform verifier unless the game
called `SetHTTPRequestRequiresVerifiedCertificate(false)`: the daemon
then makes that request, on every hop, without checking the server's
certificate chain or its name, as the SDK says the official client does
(SEC-3 allows this one exception). The official client otherwise drops
a connection whose certificate does not verify (a message in its code
says so). *Unverified:* the official default; ossteam checks unless
asked not to.

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
| 17 | `HttpSend{handle: u32, method: u32, url: string, headers: list<HttpHeader>, body: list<u8>, user_agent_info: string, activity_timeout_ms: u32, absolute_timeout_ms: u32, streaming: bool, cookies: u32, skip_certificate_check: bool}` | `HttpDone{successful: bool, status: u32, timed_out: bool}` (tag 15) |
| 18 | `HttpCancel{handle: u32}` | `Done{}` |
| 19 | `HttpReorder{handle: u32, first: bool}` | `Done{}` |
| 20 | `HttpCookieContainer{handle: u32, allow_modify: bool}` | `Done{}` |
| 21 | `HttpReleaseCookies{handle: u32}` | `Done{}` |
| 22 | `HttpSetCookie{handle: u32, host: string, url: string, cookie: string}` | `Done{}`, or `Error` when the cookie is refused |

`HttpHeader` is `{name: string, value: list<u8>}`: a value need not be
UTF-8. `cookies` is 0 for no container; `absolute_timeout_ms` is 0 for
none; `skip_certificate_check` is true after
`SetHTTPRequestRequiresVerifiedCertificate(false)` (§4).

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
| Counter-Strike 2 through `ISteamHTTP` | done 2026-11-04, test account, main menu only: it fetched its relay config (`api.steampowered.com/ISteamApps/GetSDRConfig/v1`) twice, Steam's news and the HLTV event feed. Its console logged "Got SDR network config. Loaded revision … OK", then pings to 26 relays and "avail=OK". No request failed. That was before the cache (§3.1): ossteam made its `only-if-cached` request over the network then. |
| Counter-Strike 2 and the cache | done 2026-11-04, test account, main menu only. First launch, nothing cached: its `only-if-cached` relay-config request got the cache's 504 at once, it logged "fetch attempt #1 failed. HTTP 504. Trying again", fetched the config from the server (stored, `max-age=228`), and its relay network came up (24 relays, "avail=OK"). Second launch: that request was answered from the cache, it logged "Got SDR network config … OK" with no retry, and measured 26 relays. |
| Requests without certificate checks | done 2026-11-04 against `self-signed.badssl.com`: unsuccessful by default, status 200 with `skip_certificate_check` |
| The official cache | its lookup from its log messages, its storing check from its code, in part (§3.1); whether it serves games' requests is inferred from Counter-Strike 2 relying on it, not verified |
| The official client's limits, encoding, redirects and cookie rules | unverified |
