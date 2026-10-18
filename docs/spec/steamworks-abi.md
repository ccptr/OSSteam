# The Steamworks client ABI

| | |
|---|---|
| Status | Draft for M1c: §1 (discovery entries) written; the interfaces, the game IPC and ShimReports follow with the shim |
| Requirements | SPEC §12 (ABI-1 to ABI-5, GIPC-1 to GIPC-3, DISC-1 to DISC-3) |
| Code | discovery entries in `crates/ossteam-launch/src/discovery/`; the daemon's side in `crates/ossteamd/src/launch.rs` and `crates/ossteamd/src/core/games.rs` |

A game links Valve's `steam_api` library and ships it. `steam_api` finds
a running client, loads that client's library (`steamclient.so`,
`steamclient.dll` or `steamclient64.dll`, `steamclient.dylib`) and talks
to it through versioned interfaces. `ossteam-steamclient` is ossteam's
client library.

## 1. Discovery entries (DISC-1 to DISC-3)

**Verification.** §1.1 comes from the official Linux client's files in
the home directory and from the strings in a shipped `libsteam_api.so`
(`%s/.steam/%s`, the message that `SteamAPI_IsSteamRunning()` found no
running client, and the message that a local `steamclient.so` was tried
first). How `steam_api` checks that the client is running is
*unverified*. The Windows entries in §1.2 are *unverified*.

### 1.1 Linux

The official client keeps these in `~/.steam`:

| Entry | What it is |
|---|---|
| `sdk64` | symlink to the directory that holds the 64-bit `steamclient.so` (`<Steam>/linux64`) |
| `sdk32` | symlink to the directory that holds the 32-bit `steamclient.so` (`<Steam>/linux32`) |
| `steam.pid` | the client's pid in decimal, without a newline |
| `steam`, `root`, `bin`, `bin32`, `bin64` | symlinks into the client's installation |
| `registry.vdf`, `steam.pipe`, `steam.token` | the client's own state; not read by `steam_api` (*unverified*) |

`steam_api` first tries a `steamclient.so` next to itself, then
`~/.steam/sdk64/steamclient.so` (or `sdk32` for a 32-bit game). Before
that it checks for a running client through `~/.steam/steam.pid`
(*unverified*: that it checks the pid names a live process).

ossteam creates only `sdk64`, `sdk32` and `steam.pid`:

- `~/.steam/sdk64` and `~/.steam/sdk32` are symlinks to
  `<data>/steam-sdk/sdk64` and `<data>/steam-sdk/sdk32`. Those
  directories belong to ossteam, and each holds a `steamclient.so`
  symlink to the matching library (§1.3).
- `~/.steam/steam.pid` holds the daemon's pid.

`~/.steam` is created with mode 0755 when it does not exist, as the
official client creates it.

An entry is ossteam's when `sdk64` or `sdk32` is a symlink whose target
is the matching directory under `<data>/steam-sdk`. `steam.pid` is
ossteam's when `sdk64` is. ossteam writes `steam.pid` last and removes
it first.

**Conflict (DISC-3).** If `sdk64`, `sdk32` or `steam.pid` exists and is
not ossteam's, the daemon leaves all three alone. This is the case when
an official client is installed, whether or not it is running, because
its symlinks stay when it exits. A launch then fails with `CONFLICT`,
and the message names the entry and says that an official Steam client
owns it. ossteam never removes, renames or replaces it. Using ossteam to
play on a machine with the official client installed means removing that
client's `~/.steam/sdk32`, `sdk64` and `steam.pid` while it is not
running. The official client recreates them when it starts.

### 1.2 Windows

The official client keeps the key
`HKEY_CURRENT_USER\Software\Valve\Steam\ActiveProcess`. ossteam writes:

| Value | Type | Data |
|---|---|---|
| `pid` | `REG_DWORD` | the daemon's pid |
| `SteamClientDll` | `REG_SZ` | the 32-bit library's absolute path |
| `SteamClientDll64` | `REG_SZ` | the 64-bit library's absolute path |
| `ActiveUser` | `REG_DWORD` | the active account's account id, or 0 when none is logged on |

The entries are ossteam's when `SteamClientDll64` names ossteam's
64-bit library. If the key has a `SteamClientDll` or `SteamClientDll64`
that names another file, the key belongs to an official client, and the
daemon does not touch it (DISC-3). *Unverified:* which values `steam_api`
reads, and whether the official client removes them when it exits.

### 1.3 The client libraries

The daemon finds its client libraries in the directory given by
`OSSTEAM_STEAMCLIENT_DIR`, or else in the directory of its own
executable:

| OS | 64-bit | 32-bit |
|---|---|---|
| Linux | `libossteam_steamclient.so` | `lib32/libossteam_steamclient.so` |
| Windows | `ossteam_steamclient.dll` | `lib32\ossteam_steamclient.dll` |

The 64-bit library is required. Without it, no entries are created, and
a launch fails with `UNSUPPORTED`, with a message that the client
library is not installed. The 32-bit library is optional. Without it,
`sdk32` (Linux) and `SteamClientDll` (Windows) are not created, and a
32-bit game's `steam_api` finds no client, as under an official client
that lacks one.

### 1.4 Lifetime

The daemon creates the entries once the session has an active account,
and removes them when it has none any more (a logout) and when the
daemon exits (DISC-1). An account stays active while the connection is
down and the daemon reconnects, so the entries stay too: the game's
`steam_api` then finds a client that reports being logged off (NET-4),
not a missing one. On start, before anything else, the daemon removes
entries that are ossteam's but hold a pid that is not its own, which a
crash leaves behind (DISC-2).

macOS entries are established in M1e. Until then a launch on macOS fails
with `UNSUPPORTED`.
