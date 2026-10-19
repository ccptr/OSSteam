# M1e acceptance: GUI and macOS

SPEC §21 M1e. **Exit:** the checklists for M1a to M1c pass on macOS, and
the M1a to M1d checklists pass through the GUI on every platform each
applies to. All §20 budgets are met. This completes M1.

Each check's result must match the official client's on the same account
(§19.3), and through the GUI it must also match what the CLI reports for
the same daemon (LEAF-3). Fill in the account used (a test account, never
a personal one), the date and the build.

## Building blocks

| Item | Status |
|---|---|
| `docs/spec/gui.md`; `AppSummary.update_available`, `GetAppResponse.last_shim_report` (IPC schema 8) | written 2026-10-18 |
| The daemon connection, the store and its event handling (`gui.md` §2, §3) | built 2026-10-18; unit tests on recorded events |
| Login, Library, App, Downloads and Settings pages; the account menu and status bar (§4) | built 2026-10-18; Login, Settings and Downloads seen on the real install without an account; Library with 10,000 fake-CM apps 2026-10-18 |
| Keyboard navigation and focus (§6) | built 2026-10-18; Ctrl-2/3 and the Tab focus ring checked by hand |
| Capsules decoded at cell size, at most 256 kept (§4.2) | built 2026-10-18; unit tests; peak RSS over a 10,000-app scroll went from 11.7 GB to 196 MB |
| The scroll bench: `gui_bench_library_of_10k_apps`, `OSSTEAM_GUI_BENCH=library-scroll` (§7) | built 2026-10-18 |
| Integration test of the GUI's connection and store against a real daemon and the fake CM (§7) | ⏳ the fake CM is test code inside the daemon binary, and gpui's `test-support` pulls a git source `deny.toml` rejects |
| macOS discovery (§24 Q1): what `steam_api` needs to find a client, in `steamworks-abi.md` §1.3 | written 2026-10-18 from the macOS `libsteam_api.dylib` of SDK 1.65a and one from 2017: the Mach service `com.valvesoftware.steam.ipctool` |
| macOS discovery entries: the Mach service and `<data>/steam-sdk/osx/steamclient.dylib`; `build-steamclient` makes a universal library | built 2026-10-18; type-checked for aarch64-apple-darwin; the unit tests (a client that does what `steam_api` does) have not run on a Mac, and nothing has been loaded by a game |
| `docs/acceptance/reference-hw.md` | ⏳ not written; the numbers below are from the development machine |

## Setup

- The daemon and the GUI built with `--release`; budgets are measured on
  release builds only.
- For the M1a to M1d checks, the setup of each of those runbooks, with
  the GUI doing what the CLI does there. Keep a terminal with
  `ossteam events` open: the GUI must show each event as the CLI does.
- For the scrolling budget, no account is needed. In one terminal:

  ```text
  cargo test -p ossteamd --release -- --ignored --nocapture gui_bench
  ```

  It serves a daemon with a library of 10,000 apps, each with a capsule
  already cached, and prints the command that runs the GUI against it
  (with `XDG_RUNTIME_DIR`, `TMPDIR` on macOS, pointing at the test
  daemon) and the one that stops it. Keep the GUI window focused and
  unobscured while it scrolls: gpui draws an inactive window at 30 frames
  a second at most, and the bench reports frames drawn so. On Windows the
  daemon's pipe name cannot be redirected, so this setup is Linux and
  macOS only for now.
- For the start-up budget, with the real daemon already running:
  `OSSTEAM_GUI_STARTUP_TIMING=exit ossteam-gui --no-autostart`, five
  times, and record the slowest.

## Checklist

| Check | Linux x86_64 | Windows x86_64 | macOS |
|---|---|---|---|
| M1a through the GUI: every login path on the Login page (password, each guard kind, QR with a rotated challenge), wrong password and code, *Remember me* off, two accounts and a switch from the account menu, the library with DLC on the App page, a PICS change appearing, persona state from the account menu | ⏳ | ⏳ | ⏳ |
| M1b through the GUI: install with a DLC depot, update, pause and resume, a daemon stop mid-download resumed, the queue reordered, verify and repair, uninstall; each job's progress matches `ossteam jobs` | ⏳ | ⏳ | ⏳ |
| M1c through the GUI: Play, launch options and the custom command line, Stop, the last shim report on the App page; with the official client's discovery entries in place, Play fails as `launch` does | ⏳ | ⏳ | ⏳ |
| M1d through the GUI: the compatibility tool, its chain and the choice of tool on the App page, `compat.default_tool` in Settings, the first Play installing tools as jobs, uninstall with *Also delete the Proton prefix* | ⏳ | n/a | n/a |
| The library, the App page's artwork and every job look the same as the official client's for the same account | ⏳ | ⏳ | ⏳ |
| Without a daemon: the GUI says so and starts one; *Retry* reconnects after `ossteam daemon stop` | ⏳ | ⏳ | ⏳ |
| Keyboard only: every page and control reachable with Tab and the §6 shortcuts | ⏳ | ⏳ | ⏳ |
| M1a checklist on macOS (CLI) | n/a | n/a | ⏳ |
| M1b checklist on macOS (CLI) | n/a | n/a | ⏳ |
| M1c checklist on macOS (CLI), with the macOS discovery entries | n/a | n/a | ⏳ |
| macOS discovery: an arm64 and an x86_64 (Rosetta) Steamworks game find the daemon; after `ossteam logout` the game's `steam_api` finds no client, and after a new login it finds the daemon again (the name goes and comes back, §1.3.3); with the official client installed, `launch` fails with `CONFLICT` naming the service | n/a | n/a | ⏳ |
| §20: GUI cold start to first frame ≤ 500 ms (daemon running) | dev machine 2026-10-18: 152 to 154 ms over five runs, scale factor 1; reference ⏳ | ⏳ | ⏳ |
| §20: Library with 10,000 apps scrolls at the display's refresh rate, no frame over 16 ms | dev machine 2026-10-18 at 165 Hz, scale factor 1: grid 1,996 frames, work p50 1.5 ms, max 4.4 ms, 0 to 2 missed; list 9,990 frames, max 1.1 to 2.9 ms, 0 to 1 missed; reference ⏳ | ⏳ | ⏳ |

The development machine's numbers are not results: the budgets are met
only on the reference hardware, with hardware GPU rendering. A missed
frame there is a failure even when no frame's work is over 16 ms: on a
display faster than 60 Hz the budget is the display's own rate. Record
both layouts, the display's refresh rate and the scale factor, and run
once at a scale factor of 2, where the capsules are `image2x` and four
times the pixels.

✅ verified · ⏳ not yet run
