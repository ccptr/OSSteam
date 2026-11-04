---
name: discovery-entries-official-steam
description: An official Steam install owns ~/.steam/sdk64 on the dev machine, so launches failed with exit 8; a private ~/.steam per launch (DISC-4 to DISC-6) accepted 2026-11-03; DISC-4 built and run live 2026-11-03 (Portal: First Slice in the Steam Linux Runtime); DISC-5 and most of DISC-6 to build
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

**How to apply:** still to build:

- DISC-6: `GetStatus.game_isolation`, the `ossteam status` and `launch`
  lines, the GUI notice and Settings.
- DISC-5: the take-over. The `CONFLICT` message leaves out
  `discovery.take_over_official` as a way out until the setting exists.
  The take-over's record is `<config>/official-entries.toml`
  (maintainer, 2026-11-03: in the configuration directory, not in
  `<data>/steam-sdk` where every game would see it in its private
  `~/.steam`; no mount hides `<config>`).

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

Still open in §1.1.7: games installed under
`~/.steam`, and the Steam Linux Runtime inside the namespace. The checks
are in §1.1.8.
