# Connection manager (CM) protocol

| | |
|---|---|
| Status | Draft for M1a; nothing implemented yet |
| Requirements | SPEC §9.1 (NET-1 to NET-4), §19.3 |
| Code (planned) | `crates/ossteam-net` |

The CM is the long-lived connection between a Steam client and Valve's
servers. Logon, licenses, PICS, depot keys, persona state and play-session
reporting all travel over it. This page describes the protocol in our own
words. The message schemas are in the vendored SteamDatabase protobufs
(SPEC LIC-5), so this page names messages but does not restate their
fields.

**Verification.** This draft comes from public community documentation of
the protocol. Before M1a code depends on a statement, that statement must
be confirmed against a capture of the official client on our own test
account (§19.3), and its row in §9 must be updated. Rows marked
*unverified* must not be treated as fact.

## 1. Finding servers (NET-1)

The server list comes from the WebAPI method
`ISteamDirectory/GetCMListForConnect/v1` (HTTPS GET, no authentication).
Its parameters are `cellid`, the client's last known cell, with 0 when
there is none, and optionally `cmtype`. The response holds a list of
servers. Each entry has:

- `endpoint`: `host:port`;
- `type`: `websockets` or `netfilter`, where netfilter means raw TCP;
- a datacenter tag and load figures.

The daemon caches the list in its cache directory together with the time
it was fetched, and tries servers in the order returned. A failed server
backs off exponentially, starting at 1 s and capped at 5 min, with jitter.
After a successful logon the server reports the client's cell id, and the
daemon stores it for the next directory request.

## 2. Transports (NET-2)

Both transports carry the same *Steam messages* (§3). They differ only in
how the messages are framed and protected.

### 2.1 WebSocket (primary)

The client connects to `wss://<endpoint>/cmsocket/` with TLS (SEC-3). Each
binary WebSocket message carries exactly one Steam message. TLS protects
the connection, so the Steam-level channel encryption in §2.2 is not used.

### 2.2 TCP with channel encryption (fallback)

Each packet on the TCP stream is:

```
u32 LE  payload length
u32 LE  magic 0x31305456 ("VT01" in ASCII)
bytes   payload (one Steam message)
```

Before anything else is sent, the server starts a channel-encryption
handshake. The handshake uses the basic, non-protobuf header (§3.2):

1. The server sends `ChannelEncryptRequest`. Its body holds the protocol
   version, the universe and a random challenge (16 bytes).
2. The client generates a random 32-byte session key and encrypts
   *session key ‖ challenge* with RSA-OAEP (SHA-1). The RSA key is Valve's
   published public key for that universe, which ships with the client
   rather than being fetched. The client sends `ChannelEncryptResponse`
   with the protocol version, the length of the encrypted block, the block
   itself, its CRC32, and a zero trailer.
3. The server answers `ChannelEncryptResult` with an EResult. If that
   result is not OK, the connection is dropped.

From then on, every payload in both directions is encrypted:

- **IV.** 16 bytes. The sender picks 3 random bytes `r` and computes
  HMAC-SHA1 over `r ‖ plaintext` with the first 16 bytes of the session key
  as the HMAC key. The IV is the first 13 bytes of that HMAC followed by
  `r`.
- **Body.** The plaintext is encrypted with AES-256-CBC (PKCS#7 padding)
  under the session key and that IV.
- **Wire form.** The IV encrypted with AES-256-ECB under the session key,
  followed by the CBC ciphertext.

The receiver decrypts the IV, decrypts the body, recomputes the HMAC from
the last 3 bytes of the IV and the plaintext, and drops the connection if
it does not match.

*The universe public key is a Valve-published constant. It is recorded
under `docs/spec/` with its source when TCP is implemented. Until then the
TCP fallback is not built (FAITH-2): the daemon connects over WebSocket
only.*

## 3. Steam messages (NET-3)

### 3.1 The EMsg word

Every message starts with a little-endian `u32`. The low 31 bits are the
message type (`EMsg`), whose numbers are defined in the vendored enums. The
high bit (`0x80000000`) marks a message with a protobuf header.

### 3.2 Headers

**Protobuf header**, used by nearly all modern messages:

```
u32 LE  EMsg | 0x80000000
u32 LE  header length N
N bytes CMsgProtoBufHeader
bytes   body (a protobuf message)
```

`CMsgProtoBufHeader` carries the session's Steam id and session id, the
source and target job ids, the EResult on replies, and the
`target_job_name` for service methods (§3.5).

**Extended header**, 36 bytes, used by a few legacy messages:

```
u32 LE  EMsg
u8      header size (36)
u16 LE  header version (2)
u64 LE  target job id
u64 LE  source job id
u8      canary (239)
u64 LE  Steam id
i32 LE  session id
bytes   body (fixed-layout struct)
```

**Basic header**, 20 bytes, used only by the channel-encryption messages:
`u32 EMsg`, `u64` target job id, `u64` source job id.

An unused job id is `u64::MAX`.

### 3.3 Jobs

A request that expects a reply puts a fresh, non-`MAX` value in its source
job id. The reply carries that value in its target job id. The daemon keeps
a table of pending job ids, each with a one-shot channel and a deadline
(CONC-1). A reply whose target job is not pending is logged and dropped.
Job ids are unique within a connection. The daemon uses a monotonically
increasing counter.

### 3.4 `Multi`

`EMsg::Multi` wraps several messages in one. Its body (`CMsgMulti`) has an
unzipped size and a payload. When the size is non-zero the payload is
gzip-compressed and must inflate to exactly that size. The inflated payload
is a sequence of `u32 LE length ‖ message`, and each inner message is
handled as if it had arrived on its own. Multis may nest. The inflated size
is limited to 64 MiB (SEC-4).

### 3.5 Unified service methods

RPC-style services (`Authentication`, `Player`, `ContentServerDirectory`,
...) are called by sending a protobuf-header message:

- `EMsg::ServiceMethodCallFromClientNonAuthed` before logon;
- `EMsg::ServiceMethodCallFromClient` after logon.

`target_job_name` is set to `<Service>.<Method>#<version>`, for example
`Authentication.GetPasswordRSAPublicKey#1`, and the body is the method's
request message. The reply is `EMsg::ServiceMethodResponse`, with the
EResult in the header and the response message as the body. Requests and
responses come from the vendored `steammessages_*.steamclient.proto` files.

## 4. Session lifecycle

1. **Connect** the transport (§2).
2. **Hello**: send `ClientHello` with the client protocol version. This is
   required before unauthenticated service calls. *The protocol version
   constant is unverified.*
3. **Authenticate** if needed (`auth.md`). This uses non-authed service
   calls on the same connection.
4. **Logon**: send `ClientLogon` with the account name, the token from
   `auth.md` §4, the client's OS type and language, the machine
   identifiers the official client sends (§7), and the cell id. The server
   answers `ClientLogOnResponse` with an EResult. On success it also gives
   the session's Steam id (in the header), the heartbeat interval, the cell
   id, and the public IP as the server sees it.
5. **Steady state**: send `ClientHeartBeat` at the interval the server
   gave. Respond to server pushes: license list, persona, PICS changes, and
   so on.
6. **Logoff**: send `ClientLogOff`. The server acknowledges with
   `ClientLoggedOff`, which is also how it announces a logoff it initiates
   (with a reason EResult).

## 5. Reconnection (NET-4)

If the transport drops, or a heartbeat fails, the daemon reconnects with
backoff (§1) and logs on again with the stored refresh token. While
disconnected, IPC status reports `CONNECTING`, and the game API reports
`BLoggedOn() == false`. If the logon fails because the token was revoked,
the daemon stops retrying. It emits a `SessionChanged` event and asks for
a new login (AUTH-3).

## 6. Messages ossteam needs in M1a

| Purpose | Messages |
|---|---|
| Handshake | `ClientHello`, `ChannelEncrypt*` (TCP only) |
| Auth | Non-authed service calls to `Authentication.*` (see `auth.md`) |
| Logon | `ClientLogon`, `ClientLogOnResponse`, `ClientLogOff`, `ClientLoggedOff`, `ClientHeartBeat` |
| Session info | `ClientAccountInfo`, `ClientEmailAddrInfo`, `ClientWalletInfoUpdate` (received; only what IPC exposes is kept) |
| Licenses | `ClientLicenseList` |
| PICS | `ClientPICSProductInfoRequest`/`Response`, `ClientPICSChangesSinceRequest`/`Response`, `ClientPICSAccessTokenRequest`/`Response` |
| Persona | `ClientChangeStatus`, `ClientPersonaState` |

## 7. Matching the official client

The goal in §19.3 means ossteam's wire behaviour must match the official
client's. That covers the fields it sets in `ClientLogon` (OS type,
client package version, machine name, machine id, `should_remember_password`
and the others), the order of messages after logon, and which optional
requests it makes. The values are taken from captures of the official
client on our test account and recorded in this section with the date of
the capture. **No capture has been made yet.** Until one is recorded, M1a
code fills only the fields this page establishes, and the differences are
tracked as open items.

## 8. Limits and robustness (SEC-4, SEC-5)

- Inbound Steam messages are limited to 16 MiB on WebSocket. On TCP the
  length prefix is checked before the payload is allocated.
- Header lengths must fit inside the message, and protobuf decoding failures
  drop the message, not the connection.
- The parsers for the frame, header and Multi layers get fuzz targets
  alongside their implementation (SEC-5).

## 9. Verification status

| Statement | Status |
|---|---|
| Directory method, parameters and response shape (§1) | unverified |
| WebSocket URL path and one message per frame (§2.1) | unverified |
| TCP framing, magic and channel-encryption scheme (§2.2) | unverified |
| Header layouts and the protobuf flag (§3.2) | unverified |
| `Multi` format (§3.4) | unverified |
| Service-method EMsgs and `target_job_name` format (§3.5) | unverified |
| `ClientHello` before non-authed calls, and its protocol version (§4) | unverified |
| `ClientLogon` field values (§7) | not captured |
