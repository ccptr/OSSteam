# ISteamUser

| | |
|---|---|
| Status | Implemented for M1c: logon state, Steam id, the data folder, auth session tickets, Web API tickets, auth sessions and encrypted app tickets |
| Requirements | SPEC §12.3, GIPC-1 to GIPC-3, FAITH-1 to FAITH-3, NET-4 |
| Versions | `SteamUser004` to `SteamUser023` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/user.rs` and `tickets.rs`; the daemon's side in `crates/ossteamd/src/tickets.rs` and `crates/ossteamd/src/core/tickets.rs` |

## 1. Methods

| Method | ossteam |
|---|---|
| `GetHSteamUser` | The object's user handle. |
| `BLoggedOn` | True when the pipe is open, an account is active, and it is logged on to Steam right now (`Session.logged_on`). False while the daemon reconnects (NET-4). |
| `GetSteamID` | The active account's Steam id, also while it is not logged on. `k_steamIDNil` when no account is active. |
| `GetUserDataFolder(buffer, size)` | `<user_dir>/local` (`steamworks-abi.md` §3.4), created when missing. False, with an empty buffer, when no account is active, the folder cannot be created, or the path does not fit with its NUL. *Unverified:* the official client's `<Steam>/userdata/<account id>/<app id>/local` is the model; whether it creates the folder is not known. |
| Tickets and auth sessions | §4.2 |

Everything else fails under ABI-3 for now. In particular:

- `InitiateGameConnection` and `TerminateGameConnection`, the auth of
  games older than the auth session API, are not provided.
- Voice (`StartVoiceRecording`, `GetVoice`, ...) is not provided.
- `GetPlayerSteamLevel`, `GetGameBadgeLevel`, `BIsBehindNAT`, the phone
  and two-factor queries, `GetMarketEligibility`, `GetDurationControl`
  and `RequestStoreAuthURL` need data the daemon does not fetch yet.

## 2. Tickets: the daemon and Steam

A game proves to a server, a peer or a web service that the account
owns the app by handing it an auth session ticket. The receiver asks
Steam about the ticket, and Steam answers from what the account's client
told it: each client keeps Steam informed of the tickets it has handed
out (the auth list). An encrypted app ticket is another proof, made by
Steam itself and readable only by the game's developer.

**Verification.** The messages and their fields come from the community
protobuf definitions (`steammessages_clientserver.proto`,
`steammessages_base.proto`, `encrypted_app_ticket.proto`). The ticket
layout (§2.3) and the use of the auth list (§2.4) come from two
open-source client implementations, SteamKit2 and node-steam-user,
which agree on the layout and differ on four fields of the session
header; those are *unverified*. The callbacks come from the SDK
headers. Nothing on this page has been compared with a capture of the
official client yet.

### 2.1 Game connect tokens

After logon, and later as tokens are used, the CM sends
`ClientGameConnectTokens` (779): `max_tokens_to_keep` and `tokens`, each
a short byte string. The daemon keeps them in arrival order, at most
`max_tokens_to_keep` (default 10), dropping the oldest. Each ticket uses
the oldest token. All tokens are dropped when the connection to Steam is
lost: they belong to the CM session.

### 2.2 App ownership tickets

`ClientGetAppOwnershipTicket` (857) `{app_id}` is answered by
`ClientGetAppOwnershipTicketResponse` (858) `{eresult, app_id, ticket}`.
The ticket is signed by Steam. Its first 40 bytes are, little-endian:

| Offset | Field |
|---|---|
| 0 | `u32` length of the ticket without its signature |
| 4 | `u32` version |
| 8 | `u64` Steam id of the account |
| 16 | `u32` app id |
| 20 | `u32` public IPv4 address Steam saw |
| 24 | `u32` private IPv4 address |
| 28 | `u32` flags |
| 32 | `u32` when it was made, Unix seconds |
| 36 | `u32` when it expires, Unix seconds |

Then come `u16` n and n `u32` package ids, then `u16` m and m DLC
entries, each `u32` app id, `u16` k and k `u32` package ids, then a
`u16`, then the signature. Only §2.6 reads past offset 40.

The daemon keeps one ticket per app in memory for the logged-on
account. It fetches it when a game of an owned app connects while the
account is logged on, when the account logs on while such a game is
connected, and when a ticket is asked for and the one it has expires
within 6 hours (by Steam's clock). A fetch waits 15 s for the reply.
Losing the connection to Steam drops the tickets, so each logon fetches
them again, also when the public address has changed.

### 2.3 The ticket

An auth session ticket is, little-endian:

```
u32 t             length of the token
t bytes           the game connect token (§2.1)
u32 24            length of the session header
u32 1
u32 type          2 for a session ticket, 5 for a Web API ticket
u32 public IPv4   from the logon response's public_ip, its first byte
                  the most significant; 0 when it gave none or an IPv6
                  address
u32 0             the private address, not given
u32 ms            milliseconds since the logon, wrapping
u32 n             how many tickets the daemon made since the logon,
                  this one included
u32 o             length of the ownership ticket
o bytes           the app ownership ticket (§2.2), with its signature
```

The part before the ownership ticket is the *auth ticket*. Its CRC-32
(the IEEE polynomial, as zlib computes it) is the ticket's CRC, by which
the auth list and Steam's answers name it. *Unverified:* the last four
words of the session header. SteamKit2 writes random bytes for the two
addresses, a timestamp and a sequence; node-steam-user writes what
ossteam writes.

### 2.4 The auth list

`ClientAuthList` (5432) tells Steam every ticket the account's games
hold. The daemon sends the whole list whenever it changes, and again
after each logon when it is not empty (*unverified*: what the official
client does after a reconnect). Its fields:

| Field | Value |
|---|---|
| `tokens_left` | how many game connect tokens the daemon holds |
| `last_request_seq` | the `message_sequence` of the previous list |
| `last_request_seq_from_server` | the `message_sequence` of the last `ClientAuthListAck` |
| `tickets` | one `CMsgAuthTicket` per ticket |
| `app_ids` | the distinct apps of `tickets`, and the app of a ticket just removed when no other ticket of that app is left, so Steam learns it has none |
| `message_sequence` | counts from 1 in each CM session |

`CMsgAuthTicket`: `estate` 0 for the account's own ticket and 1 for a
ticket of another user that a game began a session with (§2.6);
`steamid` 0 for an own ticket, the other user's otherwise; `gameid` the
app id; `h_steam_pipe` the daemon's id of the game connection;
`ticket_crc`; `ticket` the auth ticket (§2.3, without the ownership
ticket); `eresult` left at its default. For a Web API ticket,
`server_secret` is `str:` and the identity the game gave, with a NUL.

Steam answers with `ClientAuthListAck` (5575), whose `ticket_crc` lists
the CRCs it holds. An own ticket is *active* when its CRC is in an ack.
One that is not active 10 s after it was added is reported as timed out
(§4.1) and stays in the list. SteamKit2 reports that Steam does not
always answer a repeated list; the CRCs are what count.

A ticket leaves the list when the game cancels it, when it ends the
auth session, and when the game connection closes: the official client
cancels a game's tickets when the game exits (*unverified*).

### 2.5 Steam's answers

`ClientTicketAuthComplete` (5429) tells the outcome of a ticket:
`steam_id`, `game_id`, `estate`, `eauth_session_response`,
`ticket_crc`, `owner_steam_id`. For a ticket of another user, matched
by CRC and Steam id, the daemon tells the game connection that began the
session (§4.1, `ValidateAuthTicketResponse_t`), and drops the ticket
from its list when the response is not `k_EAuthSessionResponseOK` (0),
as node-steam-user does. For an own ticket (another client validated
it, or it was cancelled) the daemon only logs it.

### 2.6 Auth sessions with other users

`BeginAuthSession` hands the daemon another user's ticket. The daemon
checks it, in this order, and answers with an `EBeginAuthSessionResult`:

| Check | Result |
|---|---|
| The account is logged on | otherwise `k_EBeginAuthSessionResultInvalidTicket` (1) (*unverified*) |
| The ticket's lengths fit, the session header is 24 bytes, and the ownership ticket holds at least 40 bytes | otherwise `InvalidTicket` (1) |
| The ownership ticket's Steam id is the one the game gave | otherwise `InvalidTicket` (1) |
| Its app is the connection's | otherwise `k_EBeginAuthSessionResultGameMismatch` (4) |
| It expires after Steam's clock now | otherwise `k_EBeginAuthSessionResultExpiredTicket` (5) |
| The connection has no session with that user yet | otherwise `k_EBeginAuthSessionResultDuplicateRequest` (2) |

On `k_EBeginAuthSessionResultOK` (0) the ticket joins the auth list with
`estate` 1, and Steam's verdict arrives later (§2.5). The signature is
Steam's to check. `EndAuthSession` removes the ticket.

`UserHasLicenseForApp(user, app)` answers from the ownership ticket of
the session with that user: `k_EUserHasLicenseResultHasLicense` (0) when
`app` is the ticket's app or one of its DLC entries,
`k_EUserHasLicenseResultDoesNotHaveLicense` (1) otherwise or when the
list does not parse, `k_EUserHasLicenseResultNoAuth` (2) when there is
no session with that user. *Unverified:* that the official client
answers from the ticket.

### 2.7 Encrypted app tickets

`ClientRequestEncryptedAppTicket` (5526) `{app_id, userdata}` is
answered by `ClientRequestEncryptedAppTicketResponse` (5527) `{app_id,
eresult, encrypted_app_ticket}`. The game receives
`encrypted_app_ticket` encoded as the protobuf `EncryptedAppTicket`,
which is what the SDK's ticket library decrypts (as both open-source
clients do). Steam limits how often an app may ask, and says so in
`eresult` (`k_EResultLimitExceeded`, 25); the daemon passes that on.
The request waits 15 s.

## 3. Game IPC

These requests act on the connection's app only, and need an account
that owns it (`steamworks-abi.md` §3.6).

| Tag | Request | Response |
|---|---|---|
| 6 | `AuthTicket{kind: u32, identity: string, max: u32}`: kind 2 for a session ticket, 5 for a Web API ticket; `max` the game's buffer size, 0 for none | `AuthTicket{result: u32, crc: u32, ticket: list<u8>}` |
| 7 | `CancelAuthTicket{crc: u32}` | `Done{}` |
| 8 | `BeginAuthSession{steam_id: u64, ticket: list<u8>}` | `BeginAuthSession{result: u32}`: an `EBeginAuthSessionResult` (§2.6) |
| 9 | `EndAuthSession{steam_id: u64}` | `Done{}` |
| 10 | `UserLicense{steam_id: u64, app_id: u32}` | `UserLicense{result: u32}`: an `EUserHasLicenseForAppResult` (§2.6) |
| 11 | `EncryptedAppTicket{userdata: list<u8>}` | `EncryptedAppTicket{result: u32, ticket: list<u8>}` |

`AuthTicket.result` is an `EResult`: 1 with the ticket, which is then
in the auth list; 3 when the account is not logged on; 2 when no game
connect token or ownership ticket arrived within 4 s, the ownership
ticket could not be fetched, or the ticket would not fit in `max`.

The daemon sends `Notice` (`steamworks-abi.md` §3.4) to tell a
connection what Steam said later:

| Tag | Notice | When |
|---|---|---|
| 1 | `TicketActive{crc: u32, result: u32}` | an own ticket became active (1), or timed out (`k_EResultTimeout`, 16) (§2.4) |
| 2 | `TicketValidated{steam_id: u64, owner: u64, response: u32}` | Steam's verdict on another user's ticket (§2.5): `steam_id`, `owner_steam_id`, `eauth_session_response` |

## 4. The shim

### 4.1 Callbacks

Laid out with the packing of `steamworks-abi.md` §2.2. Sizes are given
as Linux / Windows.

| Id | Structure | Fields | Size |
|---|---|---|---|
| 143 | `ValidateAuthTicketResponse_t` | `CSteamID m_SteamID`, `EAuthSessionResponse m_eAuthSessionResponse`, `CSteamID m_OwnerSteamID` | 20 / 20 |
| 154 | `EncryptedAppTicketResponse_t` | `EResult m_eResult` | 4 / 4 |
| 163 | `GetAuthSessionTicketResponse_t` | `HAuthTicket m_hAuthTicket`, `EResult m_eResult` | 8 / 8 |
| 168 | `GetTicketForWebApiResponse_t` | `HAuthTicket m_hAuthTicket`, `EResult m_eResult`, `int m_cubTicket`, `uint8 m_rgubTicket[2560]` | 2572 / 2572 |

163 and 168 follow `TicketActive`; 143 follows `TicketValidated`. All
three are queued as callbacks. 154 is the result of the
`RequestEncryptedAppTicket` call.

### 4.2 Methods

| Method | ossteam |
|---|---|
| `GetAuthSessionTicket(buffer, max, size *[, identity])` | Asks the daemon for a session ticket, waiting at most 5 s. On success copies it into `buffer`, writes its length to `*size` and returns a new `HAuthTicket` (counting from 1 in the process); callback 163 follows. Otherwise returns `k_HAuthTicketInvalid` (0) with `*size` 0. The `SteamNetworkingIdentity` of versions `022` and `023` is not used (*unverified*: how the official client uses it). |
| `GetAuthTicketForWebApi(identity)` | Asks for a Web API ticket for `identity` and returns its handle, or 0. The ticket comes only in callback 168, with `m_cubTicket` its length and the rest of the array zero. *Unverified:* SteamKit2 reports that the official client always gives 2560 bytes, the rest being whatever memory held. |
| `CancelAuthTicket(handle)` | Tells the daemon to remove the ticket from the auth list. Nothing for an unknown handle. |
| `BeginAuthSession(ticket, size, user)` | The daemon's result (§2.6); `InvalidTicket` (1) for a null ticket, a size outside 1 to 4096, or no answer. |
| `EndAuthSession(user)` | Tells the daemon to end the session. |
| `UserHasLicenseForApp(user, app)` | The daemon's result (§2.6); `NoAuth` (2) without an answer. |
| `RequestEncryptedAppTicket(data, size)` | An API call whose result is callback 154 with the daemon's `result`. On `k_EResultOK` the shim keeps the ticket, replacing the previous one. `k_uAPICallInvalid` for a negative `size`, a null `data` with a positive `size`, or a closed pipe. |
| `GetEncryptedAppTicket(buffer, max, size *)` | Copies the last ticket kept and writes its length; false, with `*size` 0, when there is none or it does not fit. |

The handles, the Web API tickets waiting for their callback and the
encrypted ticket belong to the pipe and go with it. When the daemon
closes the pipe, nothing more arrives for them.
