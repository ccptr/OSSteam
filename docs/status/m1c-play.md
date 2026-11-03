---
name: m1c-play
description: M1c status - launch layer, shim, user stats, remote storage and tickets committed; script steps and live runs left
metadata:
  type: project
---

Committed: launch layer (90664ad), shim slice 1 (6c41bfa), overload slot
order (00dca80), ISteamUserStats (b4b2808), RemoteStorage local files
(d6acdae), tickets (99a9895). Game IPC is protocol 4. Runbook:
`docs/acceptance/m1c.md` (d0b9ff2).

- b4b2808: daemon `stats.rs` (CM 818/5466, Player.GetGameAchievements,
  icons from
  `shared.steamstatic.com/community_assets/images/apps/<app>/<file>`,
  zune-jpeg); shim `userstats.rs` and `images.rs`; spec
  `docs/spec/steamworks/isteamuserstats.md`. Pipes have `send_then`
  closures; a pipe the daemon closed keeps callbacks and failed results
  (§2.5).
- d6acdae: `remote_storage.rs` writes `<user_dir>/remote/`. The quota
  comes from PICS `ufs` via App.cloud_quota / cloud_max_files. Async
  calls complete locally via `Pipe::local_call`. Nothing is synced (M3).
- 99a9895: daemon `tickets.rs` and `core/tickets.rs` (GC tokens 779,
  ownership ticket 857, ClientAuthList 5432 / ack 5575,
  TicketAuthComplete 5429, encrypted 5526); shim `tickets.rs`; Notice
  message (tag 9). Layout from SteamKit2/node-steam-user. The session
  header words are unverified. An unacknowledged ticket gives
  Timeout(16).
- `cargo xtask build-steamclient [--release] [--target t]` builds 64-bit
  plus `lib32/` (44c244f).
- The §20 pump bench (`ossteam-steamclient/tests/pump.rs`) runs at about
  30 ns on the dev box (b73115c). Unix only. The real figure needs the
  reference hardware.
- The C++ ABI test: [abi-test](../../AGENTS/steamworks/abi-test.md).
- Windows job-object, registry and named-pipe code passes under Wine
  only.
- Spacewar (480) on Linux, live 2026-11-03: its one launch option is
  `SteamWorksExample.exe` with no `oslist`, its depot 481 holds Windows
  files only, and the file is `SteamworksExample.exe`. `launch` and the
  GUI's Play both refuse with "… is missing; verify app 480" (exit 5),
  since the daemon matches the name with case and picks Steam Linux
  Runtime 1.0 ("it has a Linux version") rather than Proton. How the
  official client resolves the name's case, and which tool it runs
  Spacewar with on Linux, is not in `launch.md`; spec it before
  changing either.

Left: launch-time install-script steps
([install-scripts-2-6](install-scripts-2-6.md)); live runs (Spacewar 480,
a 32-bit title, playtime on the account).
