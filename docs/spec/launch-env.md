# The launch environment

| | |
|---|---|
| Status | Draft for M1c; variable names found in the official client, values unverified |
| Requirements | SPEC LAUNCH-2, FAITH-1, VAC-4 |
| Code | `crates/ossteam-launch/src/env.rs` |

A game started by the official client inherits the client's environment
plus variables that describe the launch. `steam_api` reads some of them,
`SteamAppId` most of all. This page lists what ossteam sets for a native
launch (`launch.md`).

**Verification.** The names below appear in the official Linux client's
library (`ubuntu12_32/steamclient.so`, build 1791918634) as environment
variable names. The values come from what the variables are for and from
public descriptions. A capture of a game's environment under the
official client (§4) is needed before the values are relied on.

## 1. Variables ossteam sets

| Variable | Value |
|---|---|
| `SteamAppId` | the app id, in decimal |
| `SteamGameId` | the game id, in decimal: the app id for an app launched from the library |
| `SteamOverlayGameId` | the same as `SteamGameId` |
| `SteamClientLaunch` | `1` |
| `SteamEnv` | `1` |
| `SteamUser` | the active account's login name |
| `SteamAppUser` | the active account's login name |

They replace any variable of the same name in the daemon's environment.
Everything else in the daemon's environment is passed on unchanged.
*Unverified:* whether the official client removes anything.

## 2. Variables ossteam does not set

These name features that ossteam does not provide. Setting them would
tell the game something untrue (FAITH-1), or would load code into the
game (VAC-4).

| Variables | Why not |
|---|---|
| `ENABLE_VK_LAYER_VALVE_steam_overlay_1`, `LD_PRELOAD` of `gameoverlayrenderer.so`, `STEAM_OVERLAY_*` | ossteam has no overlay |
| `ENABLE_VK_LAYER_VALVE_steam_fossilize_1`, `STEAM_FOSSILIZE_*` | no shader pre-caching |
| `SDL_GAMECONTROLLERCONFIG`, `SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD`, `SDL_GAMECONTROLLER_IGNORE_DEVICES`, `SDL_ENABLE_STEAM_SCREEN_KEYBOARD` | no Steam Input |
| `SteamStreaming*` | no Remote Play |
| `SteamDeck`, `SteamOS`, `STEAM_GAMESCOPE_*`, `STEAM_MANGOAPP_*` | ossteam is not the Steam Deck's session |
| `STEAM_COMPAT_*`, `STEAM_RUNTIME_*`, the runtime's `LD_LIBRARY_PATH` | compatibility tools and the Steam Linux Runtime are M1d (`compat-tools.md`) |
| `SteamRealm` | *unverified* value; to be set once captured |
| `STEAM_GAME_LAUNCH_SHELL`, `STEAM_GAME_PIDS`, `STEAM_GAMES_RUNNING` | *unverified* meaning |

## 3. Differences from the official client in M1c

- On Linux, the official client runs a native game inside the Steam
  Linux Runtime when Valve's compat-tool mapping says so (for example
  "sniper" for Counter-Strike 2), and otherwise with its scout runtime's
  libraries on `LD_LIBRARY_PATH`. ossteam runs native games against the
  host's libraries until M1d builds the runtime chains.
- The official client's environment includes what `steam.sh` sets up for
  the client itself. ossteam's daemon has the environment of the login
  session that started it.

## 4. Verification against the official client

To capture: launch, from the official client, a test app whose
executable writes its environment to a file. Compare the variables in
§1 and §2.

| Item | Status |
|---|---|
| Names in §1 and §2 | found in the official client's library |
| Values in §1 | unverified |
| Variables removed from the inherited environment | unverified |
| The Windows client's variables | unverified |
