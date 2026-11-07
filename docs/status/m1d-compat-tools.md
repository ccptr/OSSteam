---
name: m1d-compat-tools
description: M1d status - compat tool list, mappings, chain launch and install scripts in the prefix committed (scripts run live 2026-11-05); their removal at verify/uninstall on hold for the maintainer (the official client differs, 2026-11-07); the remaining live rows left
metadata:
  type: project
---

Committed: spec `docs/spec/compat-tools.md` (682de1c); tool list,
mappings, manifests and chains in `ossteam-launch/src/compat/`
(02c4a02); settings and install platform, schema 6 (4e7086a); chain
launch with a deferred Launch reply while tool jobs run (e46bba8); GetApp
`compat`, `info` output and `uninstall --remove-compat-data`, schema 7
(67d6d46); steamclient.so copies and the abstract socket
`ossteam-game-<uid>` (a5fbf2f); runbook `docs/acceptance/m1d.md`
(3b87938). Daemon tests: `crates/ossteamd/src/testing/m1d.rs`.

Left:
- Install scripts in the prefix (compat-tools §7): built 2026-11-05
  (f8543d4) and run live with Spacewar the same day
  ([m1d](../acceptance/m1d.md)): its two registry values landed under
  `Wow6432Node`, and a second start planned nothing. Not built: removing
  their values at verify and uninstall, which §7 describes (on Linux
  verify and uninstall still leave the prefix alone). On hold since
  2026-11-07: the official client removes nothing at verify and deletes
  the prefix at uninstall (§7.1); whether to follow it is for the
  maintainer ([install-scripts-in-prefix](install-scripts-in-prefix.md)). Unverified: how
  Proton's `steam.exe` runs a `.cmd` entry and whether `run` returns its
  exit code; Spacewar had no entry due.
- `steamclient.dll` in the prefix (compat-tools §6.3): ossteam's own
  DLLs since 2026-11-05, run live with Spacewar (32-bit); before that
  no Windows game under Proton got past `SteamAPI_Init`. 64-bit games
  and other Proton versions are unverified.
- Live runs: Spacewar starts under Proton Experimental (it stops at
  `ISteamController::Init`, [m1c-play](m1c-play.md)), and Counter-Strike 2
  runs in sniper ([discovery-entries-official-steam](discovery-entries-official-steam.md));
  the rows in `m1d.md` marked ⏳ are left.
- The `use_tool_subprocess_reaper` SIGINT stop path is untested against
  a real tool.
- Seen 2026-11-04: an app with no `oslist` anywhere (Europa Universalis
  III Demo, 22150) gets `proton_experimental` as a title without a Linux
  version (§2), but its only choice is `none`, since §8's choices come
  from the platforms of its launch options or `common/oslist`, and it
  names none. Which platform the official client takes such an app for
  is not in the spec; settle that before changing either rule.
