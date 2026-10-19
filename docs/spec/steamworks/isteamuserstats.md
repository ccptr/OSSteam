# ISteamUserStats

| | |
|---|---|
| Status | Implemented for M1c: stats, achievements, the schema, icons and global achievement percentages |
| Requirements | SPEC §12.3, GIPC-1 to GIPC-3, FAITH-1 to FAITH-3 |
| Versions | `STEAMUSERSTATS_INTERFACE_VERSION003` to `013` (`vtables.txt`). `001` and `002` take a `CGameID` in every method and stay stubs. |
| Code | `crates/ossteam-steamclient/src/userstats.rs` and `images.rs`; the daemon's side in `crates/ossteamd/src/stats.rs` and `crates/ossteamd/src/core/shim.rs` |

A game's stats and achievements belong to the account and the app the
process claims (`steamworks-abi.md` §3.3). Steam keeps them; the
official client fetches them when a game asks, holds the game's changes
until `StoreStats`, then sends them. ossteam does the same through the
daemon, which talks to the CM (§1) and answers the shim over the game
IPC (§2). The shim keeps the values and the changes (§3).

**Verification.** The CM messages and their fields come from the
community protobuf definitions (`steammessages_clientserver_userstats.proto`,
`steammessages_player.steamclient.proto`). The schema's layout (§1.2)
comes from the files the official client caches
(`appcache/stats/UserGameStatsSchema_<app>.bin`) and from open-source
tools that read them. The callbacks and their structures come from the
SDK headers as Proton's `lsteamclient` declares them. What the official
client does between the game and the CM is *unverified* unless a line
says otherwise.

## 1. The daemon and Steam

### 1.1 Messages

| Purpose | Request | Reply |
|---|---|---|
| A user's stats, with the schema | `ClientGetUserStats` (818): `game_id` = the app id, `crc_stats` = 0, `schema_local_version` = -1, `steam_id_for_user` = the user | `ClientGetUserStatsResponse` (819): `eresult`, `crc_stats`, `schema`, `stats`, `achievement_blocks` |
| Store changed stats | `ClientStoreUserStats2` (5466): `game_id`, `settor_steam_id` and `settee_steam_id` = the account, `crc_stats` from the last answer, `explicit_reset`, `stats` | `ClientStoreUserStatsResponse` (821): `eresult`, `crc_stats`, `stats_failed_validation`, `stats_out_of_date` |
| Global achievement percentages | `Player.GetGameAchievements#1`: `appid`, `language` = the app's language | `achievements`: `internal_name` and `player_percent_unlocked` |

Each request is a job (`cm.md` §3.3), and the reply targets it. The
daemon waits 15 s for it. A request sent while the account is not
logged on is not sent: the answer is `k_EResultNoConnection` (3).

`schema_local_version` = -1 asks for the schema every time, so the
daemon keeps none (*unverified*: what the official client sends; open
tools send -1 and receive the schema). `game_id` is the app id as a
`CGameID` of type app, which is the app id itself.

A stat's value is 32 bits: an `int32` for an integer stat, the bits of
a `float` for a float or average-rate stat, a bit field for an
achievement block (§1.2). `achievement_blocks` gives, per block, 32
unlock times in Unix seconds, one per bit; 0 for a bit not set.

`Player.GetGameAchievements` gives the percentage as a decimal string.
The daemon parses it as a `float`; an achievement whose percentage does
not parse is left out. *Unverified:* that the official client uses this
method; `Player.GetGlobalAchievementPercentages` gives the same numbers
by an internal key, which would need this method anyway to name them.

### 1.2 The schema

`schema` is binary KeyValues (`keyvalues.md`) with one root object
named after the app id:

| Key | Meaning |
|---|---|
| `version` | the schema's version |
| `stats/<n>` | one stat, `<n>` its id (also in `id`) |
| `stats/<n>/type` or `type_int` | `1` integer, `2` float, `3` average rate, `4` achievements, `5` group achievements; also as the names `INT`, `FLOAT`, `AVGRATE`, `ACHIEVEMENTS`, `GROUPACHIEVEMENTS` |
| `stats/<n>/name` | the API name of a stat of type 1 to 3 |
| `stats/<n>/default` | its default value, 0 when missing |
| `stats/<n>/bits/<b>` | one achievement of a block of type 4 or 5, `<b>` its bit (also in `bit`) |
| `.../bits/<b>/name` | the achievement's API name |
| `.../bits/<b>/display/name`, `display/desc` | its name and description: an object of strings by Steam language, or one string |
| `.../bits/<b>/display/hidden` | `1` when it is hidden until unlocked |
| `.../bits/<b>/display/icon`, `icon_gray` | the file names of its icons, unlocked and locked |
| `.../bits/<b>/progress` | `min_val`, `max_val`, and `value/operand1`: the stat whose value is its progress, when `value/operation` is `statvalue` |

Scalars may be strings or integers. The daemon turns the schema into
the shim's `StatsSchema` (§2): display strings in the app's language
(`App.language`), else `english`, else the first one given; values as
32 bits, as above. Keys the table does not list are ignored, and so are
stats whose type is unknown. *Unverified:* the official client's choice
of language for the display strings; this follows the game's language,
as `GetCurrentGameLanguage` reports it.

### 1.3 Icons

An achievement's icon is a JPEG at
`https://shared.steamstatic.com/community_assets/images/apps/<app>/<file>`,
where `<file>` is the schema's `icon` or `icon_gray`. That is where
Steam's community pages load them from. The daemon downloads it once
into `<cache>/achievement-icons/<app>/<file>`, decodes it, and hands the
shim its pixels as RGBA. A file name that is not one plain path
component is refused. A download that fails is retried on the next
request, not cached.

## 2. Game IPC

These requests act on the connection's app id only (GIPC-1). The daemon
answers them for an app the account owns, and with `Error` otherwise.

| Tag | Request | Response |
|---|---|---|
| 2 | `UserStats{steam_id: u64}`: 0 for the account | `UserStats{result: u32, steam_id: u64, crc: u32, schema: StatsSchema, values: list<StatValue>, unlocks: list<Unlock>}` |
| 3 | `StoreStats{crc: u32, values: list<StatValue>, reset: bool}` | `StoreStats{result: u32, crc: u32, reverted: list<StatValue>, out_of_date: bool}` |
| 4 | `GlobalAchievements{}` | `GlobalAchievements{result: u32, achievements: list<AchievementPercent>}` |
| 5 | `AchievementIcon{file: string}` | `Image{result: u32, width: u32, height: u32, rgba: list<u8>}` |

`result` is an `EResult`: 1 when the answer holds data, otherwise why
not (3 when the account is not logged on, the CM's `eresult` when it
refused, 2 when the request failed otherwise). The other fields are
then empty.

`StatValue`: `id: u32`, `value: u32` (32 bits, §1.1). `Unlock`: `stat:
u32`, `bit: u32`, `time: u32`. `AchievementPercent`: `name: string`,
`percent: u32` (the bits of a `float`).

`StatsSchema`: `version: u32`, `stats: list<StatDef>`.

`StatDef`: `id: u32`, `name: string` (empty for a block), `kind: u32`
(1 to 5, §1.2), `default: u32`, `achievements: list<AchievementDef>`.

`AchievementDef`: `bit: u32`, `name: string`, `display_name: string`,
`description: string`, `hidden: bool`, `icon: string`, `icon_gray:
string`, `progress_stat: string` (empty for none), `progress_min: u32`,
`progress_max: u32` (32 bits, of the progress stat's kind).

## 3. The shim

### 3.1 State

Each pipe holds its account's stats once they have arrived: the schema,
the `crc`, every stat's value with the game's changes applied, the
unlock times, and which stats changed since the last `StoreStats`. A
stat Steam sent no value for has its default. Stats of other users
(`RequestUserStats`) are kept by Steam id until the pipe is released.

**Fetching.** Versions `003` to `012` fetch when the game calls
`RequestCurrentStats`. `013` has no such method: as in SDK 1.61 and
later, where `steam_api` expects the client to have the stats without
asking, the shim fetches when it first hands out a `013` object for a
user, and again when the account logs on to Steam while the pipe has no
stats. When the answer arrives, the shim replaces the values, drops
changes not stored yet, and queues `UserStatsReceived_t`. *Unverified:*
when the official client fetches for `013`.

**Storing.** `StoreStats` sends the changed stats with the `crc`. On
`k_EResultOK` the shim takes the new `crc` and applies `reverted`, then
queues `UserAchievementStored_t` for each achievement the store
unlocked, then `UserStatsStored_t`. When `reverted` is not empty, the
result in `UserStatsStored_t` is `k_EResultInvalidParam` (8), as the
SDK documents. When the store fails, the changes stay and the next
`StoreStats` sends them again. When `out_of_date` is set, the shim
fetches the stats again, as above, which drops the changes; Steam keeps
what it has. *Unverified:* what the official client does when it is
told its stats are out of date.

There is no offline cache in M1: without a connection to Steam,
`RequestCurrentStats` returns false, and `StoreStats` fails with
`k_EResultNoConnection` and keeps the changes. The official client
caches stats on disk and sends them later.

### 3.2 Callbacks

Laid out with the packing of `steamworks-abi.md` §2.2: 4 on Linux and
macOS, 8 on Windows. Sizes are given as Linux / Windows.

| Id | Structure | Fields | Size |
|---|---|---|---|
| 1101 | `UserStatsReceived_t` | `uint64 m_nGameID`, `EResult m_eResult`, `CSteamID m_steamIDUser` | 20 / 24 |
| 1101 | the same, for version `003` | `uint64 m_nGameID`, `EResult m_eResult` | 12 / 16 |
| 1102 | `UserStatsStored_t` | `uint64 m_nGameID`, `EResult m_eResult` | 12 / 16 |
| 1103 | `UserAchievementStored_t` | `uint64 m_nGameID`, `bool m_bGroupAchievement`, `char m_rgchAchievementName[128]`, `uint32 m_nCurProgress`, `uint32 m_nMaxProgress` | 148 / 152 |
| 1109 | `UserAchievementIconFetched_t` | `CGameID m_nGameID`, `char m_rgchAchievementName[128]`, `bool m_bAchieved`, `int m_nIconHandle` | 144 / 144 |
| 1110 | `GlobalAchievementPercentagesReady_t` | `uint64 m_nGameID`, `EResult m_eResult` | 12 / 16 |

`m_steamIDUser` is at offset 12 on both: `CSteamID` is aligned to 1
(`steamworks-abi.md` §2.2), and on Windows the `uint64` pads the end.
`m_steamIDUser` arrived with `RequestUserStats` in version `004`. A
pipe that has handed out only version `003` objects gets the short
structure. *Unverified:* that `steam_api` of that age drops a callback
of another size.

`UserStatsReceived_t` from a fetch of the account's stats is queued as a
callback. From `RequestUserStats` it is the call's result (§2.5 of
`steamworks-abi.md`). `GlobalAchievementPercentagesReady_t` is the
result of `RequestGlobalAchievementPercentages`.

### 3.3 Methods

"The stats" are the account's on this pipe; a method that needs them
fails while they have not arrived. Names are compared exactly.

| Method | ossteam |
|---|---|
| `RequestCurrentStats` | False when the account is not logged on to Steam. Otherwise starts a fetch (§3.1) and returns true. |
| `GetStat(name, int32 *)`, `GetStat(name, float *)` | The value of the stat `name` when its kind fits: integer for `int32`, float or average rate for `float`. |
| `SetStat(name, int32)`, `SetStat(name, float)` | Sets the value of an integer stat (`int32`) or a float stat (`float`), and marks the stat changed. An average-rate stat changes only through `UpdateAvgRateStat`. The shim does not check `min`, `max`, `maxchange` or `incrementonly`: Steam checks them when the stats are stored and reverts what breaks them (§3.1). *Unverified:* whether the official client checks them earlier. |
| `GetAchievement(name, bool *)` | Whether it is unlocked: its bit in its block. |
| `GetAchievementAndUnlockTime(name, bool *, uint32 *)` | The same, and the unlock time, or 0. |
| `SetAchievement(name)` | Sets its bit and marks the block changed. The unlock time is Steam's clock (`GetServerRealTime`), or the computer's when there is none yet. True also when it was unlocked already. |
| `ClearAchievement(name)` | Clears its bit and its unlock time. |
| `StoreStats` | False when the stats have not arrived or the pipe is closed. Otherwise sends the changes (§3.1), even when there are none, and returns true. |
| `IndicateAchievementProgress(name, cur, max)` | False when the achievement is unknown or unlocked. Otherwise queues `UserAchievementStored_t` with the progress at once and returns true. Nothing is stored, as the SDK documents; with no overlay, nothing is shown. |
| `GetNumAchievements` | The achievements of type-4 blocks, in the schema's order. |
| `GetAchievementName(i)` | The API name of achievement `i` in that order, or an empty string. |
| `GetAchievementDisplayAttribute(name, key)` | `name` and `desc`: the display strings (§1.2); `hidden`: `"1"` or `"0"`. An empty string for another key or an unknown achievement. |
| `GetAchievementIcon(name)` | The image handle of its icon: `icon` while unlocked, `icon_gray` while locked. The first call for a file returns 0 and asks the daemon (§1.3); when the image arrives, `UserAchievementIconFetched_t` is queued and later calls return the handle. When the daemon cannot give it, the callback carries handle 0 and the next call asks again. 0 for an unknown achievement, or one without that icon. |
| `GetAchievementProgressLimits(name, int32 *, int32 *)`, `(name, float *, float *)` | `progress_min` and `progress_max` when the achievement's progress stat has the kind the overload names (as `GetStat`). False otherwise. |
| `ResetAllStats(achievements_too)` | Sets every stat of kind 1 to 3 to its default and, with `achievements_too`, every block to 0 with its unlock times, then stores as `StoreStats` does with `explicit_reset` set. False when the stats have not arrived. *Unverified:* what `explicit_reset` changes on Steam's side. |
| `RequestUserStats(user)` | An API call whose result is `UserStatsReceived_t` for `user`. On `k_EResultOK` the user's stats are kept. |
| `GetUserStat(user, name, ...)`, `GetUserAchievement(user, name, bool *)`, `GetUserAchievementAndUnlockTime(...)` | As `GetStat` and `GetAchievement`, on the stats `RequestUserStats` fetched for `user`. False before they arrive. |
| `RequestGlobalAchievementPercentages` | An API call whose result is `GlobalAchievementPercentagesReady_t`. On `k_EResultOK` the percentages are kept, ordered from the most achieved down; equal percentages keep the schema's order. |
| `GetMostAchievedAchievementInfo(name, size, float *, bool *)` | The first achievement in that order: its name (truncated to `size`), its percentage, and whether the account has it. Returns 0, the iterator, or -1 when there are no percentages. |
| `GetNextMostAchievedAchievementInfo(prev, ...)` | The achievement after `prev`, and `prev + 1`, or -1 past the end. |
| `GetAchievementAchievedPercent(name, float *)` | Its percentage, once they have arrived. |

Everything else fails under ABI-3: `UpdateAvgRateStat` (Steam's
average-rate arithmetic is unpublished, so an average-rate stat can be
read but not updated), the leaderboards (`FindOrCreateLeaderboard` and
the rest), `GetNumberOfCurrentPlayers`, `RequestGlobalStats`,
`GetGlobalStat` and `GetGlobalStatHistory` (global stats are not in
M1's scope, SPEC §12.3).

## 4. ISteamUtils images

Image handles count up from 1 and are never reused while the process
runs. An image is the decoded icon: `width` × `height` pixels of RGBA,
8 bits each, rows top down.

| Method | ossteam |
|---|---|
| `GetImageSize(handle, uint32 *, uint32 *)` | The size, or false for an unknown handle. |
| `GetImageRGBA(handle, buffer, size)` | Copies the pixels when `size` is at least `width × height × 4`. False for an unknown handle or a buffer that is too small. |
