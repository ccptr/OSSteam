# M1c acceptance: Play (native)

SPEC §21 M1c. **Exit:** on Linux and Windows, these run correctly:
Spacewar (app 480) for achievements and stats, a native title without
Steamworks, and a 32-bit Steamworks title. Playtime and in-game presence
show up on the account. A SteamStub-protected title is refused with exit
code 7, and launches fail with exit code 8 while an official Steam
install owns the discovery entries.

Each check's result must match the official client's on the same account
(§19.3): the same stats, achievements, playtime and presence, seen from
the official client or the account's community profile. Fill in the
account used (a test account, never a personal one), the titles, the
date and the build.

## Building blocks

| Item | Status |
|---|---|
| `launch.md`, `launch-env.md`, `steamworks-abi.md` | written 2026-10-17 |
| Launch options, checks before a launch, the command line (`launch.md` §1 to §3) | built 2026-10-17; unit tests and daemon tests against the fake CM |
| Supervision and `StopGame`: the reaper on Linux, job objects on Windows (`launch.md` §4, §5) | built 2026-10-17; Linux tested (`ossteamd/tests/reaper.rs`); Windows type-checked only, never run |
| Play sessions (PLAY-1, `launch.md` §6) | built 2026-10-17; daemon test against the fake CM; EMsg and fields unverified |
| SteamStub detection (`launch.md` §7) | built 2026-10-17; unit tests on synthetic PE images |
| Discovery entries and conflicts (`steamworks-abi.md` §1) | built 2026-10-17; Linux tested; Windows registry code type-checked only |
| Game IPC, protocol 4 (`steamworks-abi.md` §3) | built 2026-10-17; Unix sockets tested; Windows named pipes type-checked only |
| `ossteam-steamclient`: exports, pipes, users, vtables from Proton, callbacks and API calls (`steamworks-abi.md` §2) | built 2026-10-17; unit tests; overloads reversed outside Windows 2026-10-18, unverified against a game; C++ ABI test (§19.1 level 4, `steamworks-abi.md` §6.1) 2026-10-19 passes with GCC and Clang, x86_64 and i686, and with clang's MSVC ABI for Windows x86_64 and i686 under Wine, after fixing where two Windows callbacks put their Steam id |
| ShimReports (ABI-5, `steamworks-abi.md` §4) | built 2026-10-17; daemon test |
| `ISteamClient`, `ISteamUser` (logon state), `ISteamApps`, `ISteamUtils`, `ISteamFriends` | built 2026-10-17; unit tests |
| `ISteamUserStats`: stats, achievements, schema, icons, global percentages (`isteamuserstats.md`) | built 2026-10-18; unit tests and a daemon test against the fake CM |
| `ISteamRemoteStorage`: local files only, nothing synced (`isteamremotestorage.md`) | built 2026-10-18; unit tests |
| `ISteamUser` tickets, auth sessions and encrypted app tickets | built 2026-10-18; daemon test against the fake CM; message layout from SteamKit2 and node-steam-user, unverified |
| CLI: `launch`, `running`, `stop` | built 2026-10-17 |
| Install scripts before the first start (`install-scripts.md` §3) | ⏳ waits on the §2.6 decision |
| The 32-bit client library built and installed as `lib32/` (`cargo xtask build-steamclient`) | built 2026-10-18; Linux i686 and Windows i686 (cross, mingw) libraries export the entry points; never loaded by a game |
| §20 shim callback-pump overhead (`ossteam-steamclient/tests/pump.rs`) | built 2026-10-18; on the development machine, not the reference hardware: about 30 ns per empty `Steam_BGetCallback` optimised, on x86_64 and i686 Linux; Windows not run (the test needs a Unix socket) |
| Captures of the official client's play-session and ticket messages | ⏳ needs a capture setup |

## Setup

- Build the daemon, then the client libraries with `cargo xtask
  build-steamclient --release`, which puts the 64-bit library next to
  the daemon and the 32-bit one in `lib32/` (`steamworks-abi.md` §1.4).
  `OSSTEAM_STEAMCLIENT_DIR` can point elsewhere. Without the 64-bit
  library every launch fails with exit code 7; without `lib32/` a
  32-bit game finds no client.
- On a machine where the official client is installed, run the
  conflict check first, then quit the official client and remove its
  `~/.steam/sdk32`, `~/.steam/sdk64` and `~/.steam/steam.pid` (Linux),
  or its `HKCU\Software\Valve\Steam\ActiveProcess` values (Windows). The
  official client recreates them when it starts.
- Run the daemon with `RUST_LOG=debug`; keep its log and each game's log
  file (`ossteam running` shows the path) with the results.
- Compare against the official client after each run, on the same
  account: achievements and stats in its library view, playtime and
  "currently in-game" on the community profile.

## Checklist

| Check | Linux x86_64 | Windows x86_64 |
|---|---|---|
| `info` lists the launch options; `launch --option` picks one; an option that does not qualify fails with exit code 1 | ⏳ in part, 2026-11-04: 18010's macOS option 1 refused ("cannot be launched here") and an unknown option 9 refused, both exit 1; picking a qualifying option not yet run | ⏳ |
| A native title without Steamworks runs; `running` lists it; `launch --wait` reports its exit code | ⏳ | ⏳ |
| `stop` closes the game; a game that ignores the request is killed after the timeout | ⏳ | ⏳ |
| A game that starts a child and exits (a launcher) stays running until the last process is gone | ⏳ | ⏳ |
| Spacewar (480) runs through the shim: `SteamAPI_Init` succeeds, the persona name and Steam id are the account's | ⏳ | ⏳ |
| Spacewar: stats and achievements set in game show up on the official client and the profile | ⏳ | ⏳ |
| Spacewar: a cleared achievement is cleared on the official client | ⏳ | ⏳ |
| Spacewar: achievement icons and global percentages load | ⏳ | ⏳ |
| A 32-bit Steamworks title runs through `sdk32` / `SteamClientDll` | ⏳ | ⏳ |
| A title that authenticates with an auth session ticket or an encrypted app ticket gets past it | ⏳ | ⏳ |
| Playtime: a session of 10 minutes or more adds to the account's playtime | ⏳ | ⏳ |
| In-game presence shows on a friend's client while the game runs, and ends when it exits | ⏳ | ⏳ |
| The `ShimReport` after exit lists the interfaces the game used and the unimplemented calls | ⏳ | ⏳ |
| A SteamStub-protected title is refused with exit code 7 and its files are unchanged | n/a | ⏳ |
| With the official client's discovery entries in place, `launch` fails with exit code 8 and they are unchanged | ⏳ | ⏳ |
| Logout and daemon exit remove ossteam's discovery entries; a crashed daemon's are removed at the next start | ⏳ | ⏳ |
| A game started while the connection is down runs logged off, and its play session is reported after the reconnect | ⏳ | ⏳ |
| `launch` refuses an app with an update pending unless `--allow-stale`, and an app with a job working on it | ⏳ in part, 2026-11-04: Portal: First Slice (410) with a pending update (a Proton choice, `content.auto_update` never) and 18030 with a verify running were both refused with exit 8; `--allow-stale` not yet run | ⏳ |
| §20: shim callback-pump overhead ≤ 5 µs per `RunCallbacks` with nothing pending | ⏳ | ⏳ |

The §20 check runs `cargo test --release -p ossteam-steamclient --test
pump -- --nocapture` on the reference hardware and records the median it
prints. `docs/acceptance/reference-hw.md`, which defines that hardware,
is a draft with no machine chosen yet.

`ConnectToGlobalUser` refuses an app the account does not own
(`steamworks-abi.md` §2.3, *unverified*). If the test account has no
license for 480, that refusal is the first thing the Spacewar run shows;
record what the official client does for the same account.

No SteamStub title is known for Linux (`launch.md` §7), so that check is
Windows only.

The fake-CM tests check the daemon and the shim against our spec pages,
not against Steam. They do not tick any box here: each check still needs
a live run that matches the official client (§19.3).

✅ verified · ⏳ not yet run
