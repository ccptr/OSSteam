# ISteamGameCoordinator

| | |
|---|---|
| Status | Implemented: messages between a game and its app's Game Coordinator |
| Requirements | SPEC §12.3, GIPC-1 to GIPC-3, FAITH-1 to FAITH-3 |
| Versions | `SteamGameCoordinator001`, the only one (`vtables.txt`), reached through `ISteamClient::GetISteamGenericInterface` |
| Code | `crates/ossteam-steamclient/src/gc.rs`; the daemon's side in `crates/ossteamd/src/core/gc.rs` |

Some games talk to a server of their own behind Steam, the Game
Coordinator (GC): Counter-Strike 2, Team Fortress 2 and Dota 2 use it
for matchmaking, items and the like. The game and its GC exchange
messages that Steam carries without reading them, through the CM: the
client wraps each message from the game in `ClientToGC` and hands each
`ClientFromGC` for the game's app to the game. ossteam does the same:
the shim passes messages to the daemon over the game IPC (§2), the
daemon to the CM (§1), and back.

**Verification.** The interface, its results and its callbacks come
from the SDK headers that Proton's `lsteamclient` carries. The CM
messages come from the community protobuf definitions
(`steammessages_clientserver_2.proto`) and from SteamKit2, which sends
and receives them as a client of its own. What the official client does
between the game and the CM is *unverified* unless a line says
otherwise.

## 1. The daemon and Steam

| Direction | Message | Fields |
|---|---|---|
| to Steam | `ClientToGC` (5452) | header `routing_appid` = the app; body `CMsgGCClient`: `appid` = the app, `msgtype` = the game's message type, `payload` = the game's bytes |
| from Steam | `ClientFromGC` (5453) | body `CMsgGCClient`: `appid`, `msgtype`, `payload` |

The daemon neither reads nor changes `msgtype` and `payload`. It sends
`ClientToGC` as a plain message, not a job, and waits for nothing: what
the GC answers comes as `ClientFromGC` whenever it comes. `steamid`,
`gcname`, `ip` and `flags` are left out of what the daemon sends and
ignored in what it receives (*unverified*: what the official client
fills in).

**What a message holds.** The type's high bit (`0x80000000`) marks a
protobuf message. SteamKit2 builds the `payload` of such a message as
the type with that bit (`uint32`, little-endian), the length of a
header (`int32`), the header (a `CMsgProtoBufHeader`, as the GC
defines it), and the message. Other messages start with a fixed header
of their own. ossteam does not need to know: the payload is the game's.
The game's bytes in `SendMessage`, and what `RetrieveMessage` returns,
are the whole `payload`, with the type in `msgtype` as well. Seen
with Counter-Strike 2 on 2026-11-04: its first message had type
`0x80000fa6` (4006, the GC's client hello, as a protobuf message) and
its bytes began `a6 0f 00 80 00 00 00 00`, the same type and a header
length of 0; the GC answered it (§4).

**Playing.** A GC answers only accounts that Steam counts as playing
its app (*unverified*: observed with open clients, which report the
app in `ClientGamesPlayed` before they talk to a GC). ossteam reports
the games it launched (`launch.md` §6). A process that was not launched
may send messages too, as with the official client, and what the GC
does with them is the GC's.

**Logged off.** While the account is not logged on there is no CM to
send to: `SendMessage` fails with `k_EGCResultNotLoggedOn` in the shim
(§3) and, should the session drop between the shim's check and the
daemon's send, the daemon answers the shim so that it queues
`GCMessageFailed_t` (§3). A message is never kept for later. The GC
does not remember a game across logons either; games start over when
they see `SteamServersConnected_t` (*unverified*).

## 2. Game IPC

| Tag | Request | Response |
|---|---|---|
| 12 | `GcSend{msg_type: u32, payload: list<u8>}` | `GcSent{result: u32}`: 0 sent, 3 the account is not logged on |

| Notice tag | Notice | Fields |
|---|---|---|
| 3 | `GcMessage` | `msg_type: u32`, `payload: list<u8>` |

`GcSend` goes only to the GC of the connection's own app id, and only
for an app the account owns, as the stats requests (`steamworks-abi.md`
§3.6). Otherwise the daemon answers `Error`.

The daemon hands each `ClientFromGC` to the connections whose app is
the message's `appid` and that have sent at least one `GcSend`, in the
order Steam sent them. A connection that never talked to the GC gets
nothing, so a game with several pipes gets each message once, on the
pipe it uses for the GC. A message for an app no connection talks to is
dropped, and the daemon's log says so at debug level.

## 3. The shim

| Method | What it does |
|---|---|
| `SendMessage(type, data, size)` | `k_EGCResultInvalidMessage` (4) when `data` is null and `size` is not 0, or `size` is above 15 MiB (the game IPC's frame limit, with room for the rest); `k_EGCResultNotLoggedOn` (3) when the pipe is not logged on; otherwise it sends `GcSend` without waiting and returns `k_EGCResultOK` (0) |
| `IsMessageAvailable(size)` | true, with `*size` the size of the oldest message, when the pipe has one; false and `*size` = 0 otherwise |
| `RetrieveMessage(type, dest, cap, size)` | `k_EGCResultNoMessage` (1) when the pipe has none. Otherwise `*size` is the oldest message's size and `*type` its type; when `cap` is smaller or `dest` is null, `k_EGCResultBufferTooSmall` (2) and the message stays first; else the message is copied to `dest`, removed, and `k_EGCResultOK` (0) |

Null `size` and `type` pointers are skipped.

**Callbacks.**

| Callback | When | Fields |
|---|---|---|
| `GCMessageAvailable_t` (1701) | a `GcMessage` notice arrived | `uint32 m_nMessageSize` |
| `GCMessageFailed_t` (1702) | the daemon answered `GcSend` with anything but `GcSent{0}` | none (one byte of padding) |

The messages wait in the pipe's queue until the game retrieves them,
and are dropped when the pipe is released. *Unverified:* whether the
official client queues `GCMessageFailed_t` in other cases, and whether
it limits the queue.

## 4. Checks

The daemon logs each message's type and size at debug level
(`OSSTEAM_LOG=info,ossteamd::core::gc=debug`), and the first bytes of
each connection's first message.

| Item | Status |
|---|---|
| The game's bytes are the whole `payload` | done 2026-11-04 (§1) |
| Counter-Strike 2 reaches its GC from ossteam | done 2026-11-04 with the test account, in the Steam Linux Runtime 3.0, from the main menu, no match joined: its hello (4006) was answered with `0x80000fa4` (4004, the GC's welcome, 1621 bytes), and five messages out and seven in followed within two seconds, the largest 28819 bytes |
| `GCMessageFailed_t` in the official client | unverified |
