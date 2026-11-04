---
name: discovery-entries-official-steam
description: An official Steam install owns ~/.steam/sdk64 on the dev machine, so launches failed with exit 8; a private ~/.steam per launch (DISC-4 to DISC-6) accepted 2026-11-03; DISC-4 built and run live 2026-11-03 (Portal: First Slice in the Steam Linux Runtime and on the host, Spacewar under Proton; Counter-Strike 2 in sniper, offline: no Game Coordinator); DISC-5 and DISC-6 built 2026-11-04; the take-over's live run with the official client left
metadata:
  type: project
---

An official Steam install on the dev machine owns `~/.steam/sdk64`
(→ `~/.local/share/Steam/linux64`), so the daemon makes no ABI discovery
entries (SPEC §12.5) and every launch fails with exit 8.

On 2026-11-03 the maintainer chose a way out, and asked for warnings to
the user wherever it matters. Drafted in `steamworks-abi.md` §1.1 (with
SPEC DIRS-2, DISC-1 and DISC-3 reworded and DISC-4 to DISC-6 added, and
`launch.md` §2, `ipc.md` §5.1 and §5.4, `gui.md` §2.4 and §4.5,
`compat-tools.md` §9 and `vac.md` §7 question 6 following):

- every launch runs in a user and mount namespace with
  `<data>/steam-sdk` mounted read-only over `~/.steam` (DISC-4);
- the real entries only while free, as before; without namespaces,
  launches use them and fail with `CONFLICT` when an official client
  owns them;
- `discovery.take_over_official`, off by default: take over the
  official entries while that client is not running, hand them back,
  yield when it starts; its two risks (children of running games, and
  the race with `steam.sh`) are stated to the user (DISC-5);
- warnings when namespaces are not available, and while the setting is
  on: daemon log and a line on its standard error, `GetStatus`,
  `ossteam status` and `launch` lines, GUI notice and Settings (DISC-6).

Facts behind it, from the official client's `steam.sh` (facts only):
it counts the client as running only when `steam.pid`'s process has
`~/.steam/steam.pipe` open, and otherwise recreates its links with
`ln -fsn`, ossteam's included. One shipped `libsteam_api.so` builds
`~/.steam` from `$HOME`.

On 2026-11-03 the maintainer accepted these SPEC changes as drafted,
DISC-5 included, and answered the open questions:

- In the game's namespace each official root's `userdata/` is
  ossteam's own `<data>/userdata` (writable) and its `config/` an empty
  read-only directory, so games cannot read the official client's
  accounts (§1.1.2 steps 5 and 6; a clause added to DISC-4).
- Anti-cheat: build DISC-4 now and check it live later (§1.1.8).
- The official client may be started for the take-over checks
  (§1.1.5).

**Built on 2026-11-03 (DISC-4):** `ossteam-launch::isolation` does
steps 1 to 6 of §1.1.2 between `fork` and `exec` (the reaper's, so the
whole launch chain is inside), with system calls only, and reports a
failed step through a pipe. The daemon runs the check at start, writes
the private `steam.pid` whoever owns the real entries, and launches in
the namespace. When an official client owns the real entries it lets
the launch go on, and gives `CONFLICT` only without a namespace. A
launch whose steps fail gets `UNSUPPORTED`, a warning `Notice`, and no
private `~/.steam` until the daemon restarts. The start warning (log
and standard error) is in. The tests run the namespace for real where
the system allows it (`isolation::linux` tests and
`m1c::games_get_a_private_steam`). The test daemon can turn it off
(`Root::refuse_private_steam`).

**Built on 2026-11-04 (DISC-6, but for the take-over's parts):**
`GetStatus.game_isolation` (field 12), the `games` lines of
`ossteam status` and its `--json`, the `launch` warning on standard
error, the GUI's notice on connecting and the Settings page's Discovery
section (the GUI reads `game_isolation` again after a warning `Notice`,
gui.md §3). Checked against the bench with
`OSSTEAM_BENCH_NO_PRIVATE_STEAM` (CLI and GUI). `holds_official_entries`
is always false until DISC-5.

**Built on 2026-11-04 (DISC-5):** `discovery.take_over_official`, the
take-over and hand-back in `ossteam-launch::discovery::takeover`
(record, renames, the §1.1.3 running test, the `/proc/*/maps` scan, the
inotify watcher), and in the daemon (`core/takeover.rs`): take-over
when the conditions hold, re-checked on changes and every 10 s;
hand-back at logout, exit, setting off and at start from the record;
yield with a warning `Notice`; info notices on take-over and hand-back
(added to §1.1.6 so that clients can read `game_isolation` again). The
DISC-6 parts for it too: the start and setting-on warnings, the second
`launch` line, `config set` printing the risks, the ways out naming the
setting, and the GUI's switch with a confirmation. Checked in the bench
(CLI and GUI) and in `m1c::official_entries_are_taken_over_and_handed_back`.

**How to apply:** left to do: run the take-over against the real
official client on the dev PC (§1.1.8). The maintainer allowed starting
the official client for it (2026-11-03), but it rewrites their real
`~/.steam`, so ask before the run. The take-over's record is
`<config>/official-entries.toml` (maintainer, 2026-11-03: in the
configuration directory, not in `<data>/steam-sdk` where every game
would see it in its private `~/.steam`; no mount hides `<config>`).

**Run live on 2026-11-03.** The real daemon, restarted on this build,
launched Portal: First Slice (410) for the test account through the
Steam Linux Runtime 1.0, while the official client owned the real
`~/.steam`. The game loaded ossteam's `sdk32/steamclient.so` from its
private `~/.steam` and connected to the daemon. In its namespace the
official `userdata/` and `config/` were covered, and on the host nothing
changed (steamworks-abi.md §1.1.8). The run needed three fixes first:
encrypted names that span lines (content.md §4), every installed file
executable on Unix (content.md §8, as the official client does), and a
stale 32-bit library on the dev PC (rebuild with
`cargo xtask build-steamclient`). Spacewar (480) under Proton
Experimental connected the same day, once Windows options' paths were
looked up without regard to case (launch.md §2).

Later the same day Portal: First Slice ran with no container, through
`legacysteamruntime` (4690330, scout's libraries on `LD_LIBRARY_PATH`),
in the namespace and connected. With no tool at all it does not start
on the dev PC, which has no 32-bit SDL2. Counter-Strike 2 (730),
reflink-copied from the official client's library and updated by
ossteam (698 MiB downloaded, the rest kept in place), cannot run
outside the Steam Linux Runtime 3.0 (its `cs2.sh` checks
`/etc/os-release`). In sniper it started but waited before loading
`steamclient.so`: another local user had left
`/tmp/source_engine_<n>.lock`, which `fs.protected_regular` keeps it
from opening. Once the maintainer removed it, CS2 loaded ossteam's
`sdk64/steamclient.so` from the private `~/.steam`, connected, and
loaded its menus (§1.1.8).

Open for the maintainer: the Game Coordinator
(`SteamGameCoordinator001`, which CS2, Team Fortress 2 and Dota 2 use to
go online) is in no part of SPEC.md or `docs/spec/`, so where it belongs
is a SPEC decision. CS2 also asked for two interfaces ossteam does not
provide: `SteamNetworkingSocketsSerialized005` and
`SteamAppDisableUpdate001`.

Still open in §1.1.7: games installed under
`~/.steam`, and the Steam Linux Runtime inside the namespace. The checks
are in §1.1.8.
