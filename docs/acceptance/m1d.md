# M1d acceptance: Compatibility tools (Linux)

SPEC §21 M1d. **Exit:** on Linux, a Windows-only Steamworks title
installs its tools through the normal install engine and runs under
Proton inside the Steam Linux Runtime. Its stats and achievements reach
Steam through the shim.

Each check's result must match the official client's on the same account
(§19.3): the same tool for each app, the same stats and achievements,
seen from the official client or the account's community profile. Fill
in the account used (a test account, never a personal one), the titles,
the tool versions (`version` in each tool's directory), the date and the
build.

## Building blocks

| Item | Status |
|---|---|
| `compat-tools.md`; the compat rows of `launch.md`, `launch-env.md`, `content.md`, `ipc.md`, `steamworks-abi.md` | written 2026-10-18 |
| The tool list from app 891390, mappings and their priorities (§1, §2) | built 2026-10-18; unit tests on a copy of the official client's cache entry |
| `toolmanifest.vdf`, the chain, its command line and `STEAM_COMPAT_*` (§3 to §5) | built 2026-10-18; unit tests against the official client's `gameprocess_log.txt` line for CS2 |
| Depots of the tool's platform, recorded with the install (§4) | built 2026-10-18; daemon test against the fake CM and CDN |
| Tools installed or updated as jobs before a launch, which waits for them (§4) | built 2026-10-18; daemon tests with a fake runtime and a fake Proton |
| `compatdata/<app>`, the generated client-install directory (§6) | built 2026-10-18; daemon test |
| `compat.default_tool`, `compat.tool.<app>`; `GetApp` `compat`; `info` output (§8) | built 2026-10-18; daemon and CLI tests |
| `uninstall --remove-compat-data` (§6.1) | built 2026-10-18; daemon test |
| The shim in a container: library copies, the abstract socket (§9) | built 2026-10-18; daemon test of the abstract socket; never run in a real container |
| Stopping through `use_tool_subprocess_reaper` (§10) | built 2026-10-18; not tested against a real tool |
| Install scripts in the prefix (§7) | built 2026-11-05 (f8543d4): Wine's registry files, the 32-bit view, `regedit` imports and Run Process entries as setup commands; daemon test with a fake Proton (`m1d::install_scripts_apply_to_the_prefix_before_the_game_starts`). Not built: their removal at verify and uninstall |

## Setup

- As for M1c: the daemon and both client libraries (`cargo xtask
  build-steamclient --release`), the official client's discovery entries
  removed, the daemon run with `RUST_LOG=debug`, its log and each game's
  log kept with the results.
- Start with no tools installed, so that the first launch installs them.
  `ossteam info 891390` must show the app as known: without the tool list
  no app gets a tool.
- Note the official client's mapping for each title first: the "Force
  the use of a specific Steam Play compatibility tool" setting, and
  `~/.steam/root/logs/compat_log.txt`.
- Keep the data directory under the home directory for the main runs;
  one run with it elsewhere checks `STEAM_COMPAT_MOUNTS`.

## Checklist

| Check | Linux x86_64 |
|---|---|
| `info` on a Windows-only title shows `proton_experimental` from the default, its chain (runtime, then Proton) and the choices; the same on the official client | ⏳ |
| `info` on a native title Valve maps to the runtime (e.g. Counter-Strike 2, 730) shows `steamlinuxruntime_sniper` from Valve's mapping | ⏳ |
| Installing the Windows-only title takes its Windows depots; `info` shows it installed for `windows` | ⏳ |
| The first `launch` installs Proton and the runtime as jobs (`jobs` lists them), then starts the game; `launch` returns once it runs | ⏳ |
| The game runs under Proton in the runtime: its log shows `pressure-vessel` and Proton's start-up; `compatdata/<app>/pfx` is created | ⏳ |
| Through the shim: `SteamAPI_Init` succeeds, the persona name and Steam id are the account's (Proton's `lsteamclient` loads `~/.steam/sdk64/steamclient.so`) | ⏳ |
| Stats and achievements set in the game show up on the official client and the profile | ⏳ |
| Playtime and in-game presence, as for M1c | ⏳ |
| A native title mapped to the runtime runs in it; its stats reach Steam: the container has its own `$XDG_RUNTIME_DIR`, so the shim reached the daemon by the abstract socket | ⏳ |
| `config set compat.tool.<app> <tool>` switches the tool; an unknown name is refused; a choice that changes the platform marks an update | ⏳ |
| `stop` ends a game under Proton, and every process of the prefix exits (`wineserver` included) | ⏳ |
| A tool update published while installed is installed before the next launch | ⏳ |
| `uninstall` keeps `compatdata/<app>`; `uninstall --remove-compat-data` deletes it | ⏳ |
| With the data directory outside the home directory, the shim still loads (`STEAM_COMPAT_MOUNTS`) | ⏳ |
| A Windows-only title with an install script: its registry values land in the prefix | ✅ 2026-11-05, Spacewar (480) under Proton Experimental in the Steam Linux Runtime 4: `launch` printed "running install scripts: Writing 2 registry values", one `proton run regedit /S` exited 0, and the game started 6.7 s after the call; `Exe_Path` (its `Z:` path) and `PatchVersion` = 42 appeared under `Software\Wow6432Node\Valve\Spacewar` in `system.reg` once Wine's server saved it, and were recorded with the install. .NET 4.7.2 (depot 229006) was not run: Proton's prefix records it. A second launch planned nothing and started in 0.57 s. No Run Process entry was due, so `.cmd` files and exit codes through Proton's `steam.exe` are still unverified |
