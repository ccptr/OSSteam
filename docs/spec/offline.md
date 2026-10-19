# Offline mode

| | |
|---|---|
| Status | Draft for M3; awaiting review. Nothing has been built, and §9 lists what must be observed first |
| Requirements | SPEC M3, NET-4, PLAY-1, FAITH-1 to FAITH-3, SEC-2, ABI-3 |
| Code | none yet. Today the daemon keeps an account's licenses in memory only (`crates/ossteamd/src/pics.rs`), so a daemon started without a connection has an empty library |

SPEC M3 asks for "offline mode with truthful offline reporting to
games". The official client can be put in offline mode by hand, and it
falls back to it when it cannot log on. The user then still sees the
library and can play what is installed, and games are told that Steam
is not reachable.

ossteam already copes with a connection that drops while it runs
(NET-4): the license list stays in memory, and games are told they are
logged off. What it cannot do is *start* without Steam, because nothing
it needs for the library survives a restart. This page says what must be
kept for that, what a user can choose, and what games see while ossteam
is offline.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`offline_ticket.proto`, `steammessages_offline.steamclient.proto`,
  `steammessages_player.steamclient.proto`,
  `steammessages_clientserver.proto`);
- the official client's `config/loginusers.vdf`,
  `userdata/<id>/config/localconfig.vdf`,
  `userdata/<id>/config/licensecache`, `appcache/stats/` and its logs
  (`steamui_login.txt`, `timedtrial_log.txt`) on the development
  computer. Only their structure and sizes are used here, never their
  values;
- ossteam's own specs that this page changes: `pics.md`, `launch.md`
  §6, `steamworks/isteamuserstats.md`, `cloud.md`, `friends.md`.

Nobody has used offline mode on purpose on the development computer, so
how the official client enters and leaves it is inferred from its files
and one log sequence, and is *unverified* until §9 is done.

## 1. Three states

| State | Connection | The daemon | Games' `BLoggedOn` |
|---|---|---|---|
| Logged on | up | serves everything | true |
| Disconnected | lost or never made | reconnects with backoff (NET-4) | false |
| Offline mode | none, by choice | does not try to connect | false |

The difference between the last two is only whether the daemon keeps
trying. Both serve the library from what was kept (§3).

### 1.1 What the official client does

`config/loginusers.vdf` has, per account, `RememberPassword`,
`WantsOfflineMode`, `SkipOfflineModeWarning`, `AutoLogin` and a
timestamp. `WantsOfflineMode` is the user's choice (Steam → Go Offline),
kept across restarts. `SkipOfflineModeWarning` turns off the dialog that
the client shows when it starts without a connection.

`steamui_login.txt` shows one start in which the logon failed and the
client went on without asking:

```
[ WaitingForServerResponse ] Received logon failure response
[ WaitingForServerResponse ] Start offline - 0
[ WaitingForServerResponse ] SetLoginState: WaitingForLibraryReady - OK
[ WaitingForLibraryReady ] Received post-logon callback: L=0,P=0,A=0,R=0,C=0,I=0,CM=0
[ WaitingForLibraryReady ] SetLoginState: Success - OK
```

So a failed logon can lead straight into offline mode with the library
shown. *Unverified:* which failures do this (no network, servers down,
a revoked login) and what the `0` means.

## 2. The offline logon ticket

The official client keeps, in `userdata/<id>/config/localconfig.vdf`, an
object `Offline` with four hex strings:

| Key | Size | Most likely |
|---|---|---|
| `Ticket` | 11 bytes | a serialized `COffline_OfflineLogonTicket`: `accountid` and `rtime32_creation_time`. 11 bytes is exactly that message with a 10-digit account id |
| `Signature` | 256 bytes | Steam's signature over `Ticket`, RSA-2048 by its size |
| `EncryptedTicket` | 317 bytes | a serialized `Offline_Ticket`: `encrypted_ticket`, `signature`, and two key derivations (`kdf1`/`salt1`, `kdf2`/`salt2`) |
| `EncryptedSignature` | 256 bytes | a signature over `EncryptedTicket` |

They come from the unified method `Offline.GetOfflineLogonTicket`
(`priority`, `perform_encryption`), whose reply carries
`serialized_ticket`, `signature` and `encrypted_ticket`.
`Offline.GetUnsignedOfflineLogonTicket` returns the plain ticket only.

The signed ticket lets the official client check, without the network,
that Steam let this account log on at a given time. The encrypted ticket,
with its salts, most likely lets it check a password typed in offline,
since the client does not keep the password. *Unverified:* all of it,
including which public key checks the signature and whether the client
limits how old a ticket may be.

**ossteam would not use it.** The daemon decides itself whether an
account may go offline (§4), from state it wrote while the account was
logged on, so it has nothing to prove to itself. Nothing that a game or
the Steamworks API sees depends on the ticket, as far as the SDK
headers show. If §9 finds that a game, SteamStub or CEG reads it through
the client, this decision is revisited (§10 Q3).

## 3. What ossteam keeps

To start without Steam, the daemon needs, per account:

| What | Where today | Change |
|---|---|---|
| The account and its refresh token | the database and the keyring (`auth.md` §4) | none |
| The license list | memory only (`pics.md` §6: "Licenses and access tokens are not cached") | stored (§3.1) |
| App and package info | the PICS cache (`pics.md` §6) | none. It can be deleted at any time, and an app with no cached info is then not in the offline library |
| Installed apps | the database | none |
| Stats and achievements | none (`isteamuserstats.md` §3.1) | stored (§6) |
| Play sessions not yet reported | none | stored (§5) |
| Cloud state | `cloud.md` §3.4 | none |
| Workshop items | `workshop.md` §4 | none |
| Persona name | the database | none |

### 3.1 The license list

The official client keeps a `licensecache` per account, 134 bytes on the
development computer's account, whose content looks encrypted (no
structure, high entropy). *Unverified:* the format and the key.

ossteam would store the last license list it received per account in
the database: the fields `pics.md` §1 reads (`package_id`, `flags`,
`license_type`, `owner_id`), and `time_created`, `minute_limit`,
`minutes_used` and `time_next_process` for §3.2. Package access tokens
are left out: they are only of use online. The list is replaced each
time a new one arrives, and deleted by `Logout`, as the token is. It is
not a secret under SEC-2, but it shows what the account owns, so it gets
the same file permissions as the rest of the database.

This changes `pics.md` §6. Every other rule of `pics.md` stays: the
library is still derived, never stored (PICS-3), now from the stored list
when no list has arrived in this session.

### 3.2 Licenses that run out

Some licenses grant an app only for a while: a timed trial
(`minute_limit`, `minutes_used`), a free weekend, a subscription that
renews. Online, Steam takes them away by sending a new list. Offline,
nothing tells ossteam that the time is up.

The official client's `timedtrial_log.txt` logs "User is offline" when
it starts without a connection, so timed trials are at least looked at.
*Unverified:* whether it refuses them offline.

ossteam would grant nothing offline from a license with a `minute_limit`,
since it cannot count the minutes truthfully. Other licenses grant as
they did at the last logon. *Unverified:* which field marks a free
weekend's end (`time_next_process` is the candidate), and §10 Q4 asks
whether to refuse those too.

## 4. Entering and leaving offline mode

**By choice.** `SetOfflineMode{on}` (§7) puts the active account in
offline mode: the daemon logs off, closes the connection and stops
reconnecting. The choice is kept per account in the database, as
`WantsOfflineMode` is, and holds across restarts. `SetOfflineMode{off}`
connects and logs on with the stored token.

Going offline needs an account that has a stored token (it logged in
with `remember`) and a stored license list. An account logged in with
`--no-remember` has its token in memory only (`auth.md` §4), so it cannot go offline:
`SetOfflineMode{on}` fails with `CONFLICT` and says why. Running games
are not affected; they get `SteamServersDisconnected_t` as for any lost
connection.

**Without a choice.** A daemon that starts and cannot reach Steam is
*disconnected* (§1) and keeps trying (NET-4). It serves the stored
library meanwhile, so the user can play without first having to choose
offline mode. This differs from the official client, which asks
(unless `SkipOfflineModeWarning` is set) or falls back by itself
(§1.1). §10 Q1 asks whether ossteam should ask too.

**A failed logon.** When Steam answers and refuses the stored token
(`ACCESS_DENIED` and the like), the daemon deletes the token
(`auth.md` §4), as today. The account then has no token and can no longer
go offline: the user must log in again. Offline mode is not a way
around a revoked login.

**Leaving.** Once logged on, the license list that Steam sends
replaces the stored one, and the library is derived from it again. Apps
that the new list no longer grants leave the library as they would at
any logon. Then §5 and §6 send what was kept.

## 5. Play sessions

Online, playtime comes only from `ClientGamesPlayed` (`launch.md` §6):
Steam counts the time between the message that names a game and the one
that no longer does. A game that runs and exits while the daemon is not
logged on is therefore never counted today.

`Player.RecordDisconnectedPlaytime` takes a list of `play_sessions`,
each with `appid`, `session_time_start`, `seconds`, `offline` and
`owner`. That is what the official client most likely sends after it
comes back. *Unverified:* that it sends it, and what `offline`
distinguishes. The likely reading is offline mode (true) against a lost
connection (false).

ossteam would record each game run, or the part of a run, during which
the account was not logged on, in the database: app, start, seconds,
whether in offline mode, and the license's owner (the account itself
until Family Sharing). After the next logon it sends them in one
`RecordDisconnectedPlaytime` and deletes them once Steam has answered OK.
A run that is still going at logon is cut in two: the part before logon
goes into this list, and `ClientGamesPlayed` takes over from there.

This is an addition to `launch.md` §6 (PLAY-1).

## 6. Stats and achievements

`isteamuserstats.md` §3.1 has no offline cache in M1:
`RequestCurrentStats` returns false and `StoreStats` fails with
`k_EResultNoConnection`. The official client keeps
`appcache/stats/UserGameStats_<account>_<app>.bin` and
`UserGameStatsSchema_<app>.bin`, and its games unlock achievements
offline.

ossteam would keep, per account and app, the last stats and schema that
Steam sent (in the data directory, since they are the account's data,
not a cache that can be fetched again while offline), and the changes
made since. Offline:

- `RequestCurrentStats` answers from the kept stats with `k_EResultOK`.
  An app with no kept stats fails as today. *Unverified:* the result the
  official client gives.
- `StoreStats` keeps the changes and answers `UserStatsStored_t` with
  `k_EResultOK`. *Unverified:* again, the official client's result.
- Global stats and percentages fail with `k_EResultNoConnection`.

After the next logon the daemon fetches the current stats, applies the
kept changes on top, and stores them. Unlocked achievements stay
unlocked. A stat with `incrementonly` takes the larger value; any other
stat takes the kept value, as it was written last. *Unverified:* how the
official client merges, and whether Steam's `out_of_date` answer
applies here (`isteamuserstats.md` §3.1).

## 7. IPC, CLI and GUI

**IPC** (schema change; exact fields come with the code):

| Method or field | Does |
|---|---|
| `SetOfflineMode{on}` | §4. `NOT_LOGGED_IN` without an active account; `CONFLICT` when the account has no token or license list |
| `GetStatus`'s session | gains `offline_mode` (the choice) and `library_as_of` (when the license list in use arrived) |
| `connection` | `DISCONNECTED` in offline mode, as today when no connection is wanted |

Methods that need Steam (`Install`, `Update`, `Verify` with downloads,
`UnlockBetas`, `Subscribe`, ...) fail with `NOT_LOGGED_IN` while the
account is not logged on, as the job methods already do (`ipc.md`
§5.6). `Launch` already needs only an active account and an app in the
library (`launch.md` §2), not a logged-on one; with the stored list the
library is there after a restart without Steam too.

**CLI:** `ossteam offline on|off`. `ossteam status` shows `offline mode`
and the age of the library.

**GUI:** the account menu gets *Go offline* / *Go online*. The status bar
says *Offline* or *Disconnected*, and the Library says when its data is
from.

## 8. What games see

While the account is not logged on, in either state of §1:

| Interface | Answer |
|---|---|
| `ISteamUser::BLoggedOn` | false. `SteamServersDisconnected_t` when it changes (`steamworks-abi.md` §3.7) |
| `ISteamUser::GetSteamID` | the account's id, as today |
| `ISteamUser` tickets | auth session tickets, Web API tickets and encrypted app tickets need Steam: they fail as the API documents (`k_HAuthTicketInvalid`, `k_EResultNoConnection`). App ownership tickets are dropped with the connection already (`isteamuser.md` §2.2). *Unverified:* the official client may hand out a ticket it fetched earlier |
| `ISteamApps` ownership and DLC | from the stored license list (§3.1) |
| `ISteamUserStats` | §6 |
| `ISteamFriends` | own persona state offline (0). The friends list is kept in memory only (`friends.md`), so it is empty, which is this client's true state |
| `ISteamRemoteStorage` | the local files; sync waits for the next logon (`cloud.md` §4) |
| `ISteamUGC` | installed items from `workshop.md` §4; queries fail with `k_EResultNoConnection` |
| `ISteamUtils::GetServerRealTime` | *unverified*: the official client most likely adds its last known offset to the local clock. ossteam would do the same from the last offset it saw, and the local clock before any |
| Protected apps (SteamStub, CEG) | `drm.md` decides. *Unverified:* whether a CEG title starts offline at all |

Nothing here is made up for the occasion: each answer comes from the
last session's real data, or is the failure the API documents (FAITH-1,
FAITH-3).

## 9. Investigation before code

1. **The official client going offline** on the test account by hand,
   and starting with no network, with `loginusers.vdf`,
   `localconfig.vdf` and the logs compared before and after: when it
   fetches the offline ticket (§2), what the dialog offers, and which
   logon failures fall back to offline (§1.1).
2. **A game offline under the official client**: Spacewar (480) and the
   bench game's `-report` cannot run under it, so this needs a free
   title with stats, run offline: what `RequestCurrentStats` and
   `StoreStats` return, and whether an auth session ticket is handed out.
3. **Coming back online** after that run, with a CM capture if the user
   agrees: whether `RecordDisconnectedPlaytime` is sent and with what,
   and how kept stats are merged (§5, §6).
4. **A timed trial or free weekend** offline, if one is available (§3.2).

Steps 1 to 4 are done with the user watching. Nothing here is sent
from ossteam before the user has approved it.

## 10. Questions for review

1. **Starting without a connection** (§4): serve the stored library and
   keep trying, as proposed, or ask first as the official client does?
2. **The license list on disk** (§3.1): store it in the database, as
   proposed? It is what makes offline play possible, and `pics.md` §6
   says today that licenses are not cached.
3. **The offline logon ticket** (§2): leave it alone, as proposed, until
   something is found that needs it?
4. **Licenses that run out** (§3.2): refuse timed trials offline, as
   proposed, and also free weekends once their marker is known?
5. **Offline stats** (§6): keep them in the data directory and merge
   them at the next logon, as proposed? This changes
   `isteamuserstats.md`, which is M1 code.

## 11. Verification against the official client

| Item | Status |
|---|---|
| `WantsOfflineMode`, `SkipOfflineModeWarning` in `loginusers.vdf` | the official client's files |
| A failed logon falling back to offline | one log sequence; which failures, unverified |
| `Offline` object in `localconfig.vdf`, its four fields and sizes | the official client's files |
| What the offline ticket is used for | unverified |
| `licensecache` exists per account and looks encrypted | the official client's files; format unverified |
| Timed trials checked offline | one log line; behaviour unverified |
| `RecordDisconnectedPlaytime` sent after coming back | message dumps; unverified |
| Stats kept in `appcache/stats/` | the official client's files |
| What games get offline (stats results, tickets, server time) | unverified (§9) |
