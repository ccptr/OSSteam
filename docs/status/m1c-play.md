---
name: m1c-play
description: M1c status - launch layer, shim, user stats, remote storage, tickets and install scripts at the start committed (scripts 2026-11-05); live runs and real Windows left
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
- Spacewar (480) on Linux: on 2026-11-03 `launch` first refused it
  ("… is missing"), since its option names `SteamWorksExample.exe` and
  the depot holds `SteamworksExample.exe`; since 3838c1d Windows paths
  are looked up without regard to case, and it runs under Proton
  Experimental.
- Install scripts at the start (`install-scripts.md` §3.1), f8543d4,
  2026-11-05: planned and run by a thread per launch, announced as
  `LaunchStep`, recorded with the install; on Windows through the
  registry API, `netsh` and the elevated helper, in a Proton prefix as
  [m1d-compat-tools](m1d-compat-tools.md) says. Run live on Linux with
  Spacewar ([m1d](../acceptance/m1d.md)). The Windows code has run only
  under Wine (HKCU values): UAC, the helper, HKLM and firewall rules
  wait for real Windows ([hardware-mac-windows](hardware-mac-windows.md)).
  `kvsignatures` stay unchecked ([install-scripts-2-6](install-scripts-2-6.md)).

Left: live runs (a 32-bit title, playtime on the account); install
scripts on real Windows.
