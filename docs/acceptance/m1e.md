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
| Keyboard navigation and focus (§6) | built 2026-10-18; Ctrl-2/3 and the Tab focus ring checked by hand; Page Up, Page Down, Home and End scroll a page, 2026-10-19 |
| Capsules decoded at cell size, at most 256 kept (§4.2) | built 2026-10-18; unit tests; peak RSS over a 10,000-app scroll went from 11.7 GB to 196 MB |
| The App page's hero, logo and header decoded at the size drawn, at most 8 kept (§4.3) | built 2026-10-18; seen on the bench daemon at scale factor 1.25; opening 90 App pages one after another with the keyboard, RSS went from 689 MB and growing by about 6 MB a page to a level 297 MB |
| The scroll bench: `gui_bench_library_of_10k_apps`, `OSSTEAM_GUI_BENCH=library-scroll` (§7) | built 2026-10-18 |
| Integration test of the GUI's connection and store against a real daemon and the fake CM (§7) | built 2026-10-19: `testing::gui::connection_and_store_follow_the_daemon` in the daemon's tests, on gpui's test platform: a wrong password, a login, the library, an App page with a DLC depot, Install and Uninstall, and `daemon stop`, each compared with what a leaf client reads. gpui's `test-support` takes proptest from git; the workspace patches it to the crates.io release, so `deny.toml` is unchanged |
| macOS discovery (§24 Q1): what `steam_api` needs to find a client, in `steamworks-abi.md` §1.3 | written 2026-10-18 from the macOS `libsteam_api.dylib` of SDK 1.65a and one from 2017: the Mach service `com.valvesoftware.steam.ipctool` |
| macOS discovery entries: the Mach service and `<data>/steam-sdk/osx/steamclient.dylib`; `build-steamclient` makes a universal library | built 2026-10-18; type-checked for aarch64-apple-darwin; the unit tests (a client that does what `steam_api` does) have not run on a Mac, and nothing has been loaded by a game |
| `docs/acceptance/reference-hw.md` | drafted 2026-10-19 for review: proposed machines and measurement conditions, no machine chosen yet; the numbers below are from the development machine |

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

  It serves a daemon with a library of 10,000 apps, each with a
  capsule, a hero and a logo already cached, and prints the command that runs the GUI against it
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
| M1a through the GUI: every login path on the Login page (password, each guard kind, QR with a rotated challenge), wrong password and code, *Remember me* off, two accounts and a switch from the account menu, the library with DLC on the App page, a PICS change appearing, persona state set in Settings and shown in the account menu | ⏳ live. Dry run 2026-10-19 on the `gui_bench_states` bench daemon, keyboard only, each step matching `ossteam status`, `account list`, `library` or `info`: a wrong password (*INVALID_PASSWORD (5)*, as `ossteam login` says before exiting 4), a wrong email code (*INVALID_LOGIN_AUTH_CODE (65)*, asked again) then the right one, the authenticator code with *Remember me* off (not listed, no token in the secrets file), QR with a rotated challenge (the code changed from `…/q/1/7` to `…/q/1/8`, then signed in), switches both ways from the account menu (the other account's jobs wait, ipc.md §5.6), the App page of a game whose DLC the account does not own (no DLC listed, as in `library`), an update arriving in a PICS change (the *Update* badge), and Invisible, Offline and Online in Settings, each reaching the CM. Fixed: switching account or logging out from the account menu left no control focused. Seen once, not reproduced in four tries: after a sign-in with the authenticator code, the window took no keys at all until restarted | ⏳ | ⏳ |
| M1b through the GUI: install with a DLC depot, update, pause and resume, a daemon stop mid-download resumed, the queue reordered, verify and repair, uninstall; each job's progress matches `ossteam jobs` | ⏳ live. Dry run 2026-10-19 on the `gui_bench_states` bench daemon, keyboard only: install with a DLC depot, Move to top, pause and resume, verify repairing the DLC's file, uninstall, each matching `ossteam jobs` and `ossteam info`. Fixed: a DLC whose depot was installed was listed as not installed (GUI and the daemon's `ListApps`/`GetApp`); a paused job showed no progress (GUI and `ossteam jobs`); the install chooser and the uninstall confirmation could open out of sight. A resumed job's progress started again from what was left (13.3 of 41.0 MiB before a pause, 0.9 of 27.7 MiB after); fixed, and on the bench it went on from 8.6 to 8.8 of 41.0 MiB | ⏳ | ⏳ |
| M1c through the GUI: Play, launch options and the custom command line, Stop, the last shim report on the App page; with the official client's discovery entries in place, Play fails as `launch` does | ⏳ live. Dry run 2026-10-19 on the `gui_bench_states` bench daemon, keyboard only, each step matching `ossteam events` and `ossteam running`: Play's chooser of two launch options, each run with its own arguments; *Arguments* reaching the game (`-report`, which makes the bench game connect as the shim does and report); the last run's report on the App page (one interface version, `SteamUser021::GetPlayerSteamLevel` twice), again after the GUI restarted; *Running.* and Stop (exit code 130 after 9 s, as `events` says); with links to another client's `linux64` and `linux32` and its `steam.pid` in place, Play's error the same as `launch`'s, which exited 8, and the entries unchanged. Fixed: what follows the last control of a page, such as the last run, could not be scrolled to without a mouse (Page Up, Page Down, Home and End now scroll the App, Downloads, Settings and Login pages); how a game ended was not shown anywhere (now in *Last run*, and a report from an earlier run is said to be so) | ⏳ | ⏳ |
| M1d through the GUI: the compatibility tool, its chain and the choice of tool on the App page, `compat.default_tool` in Settings, the first Play installing tools as jobs, uninstall with *Also delete the Proton prefix* | ⏳ live. Dry run 2026-10-19 on the `gui_bench_states` bench daemon, keyboard only, each step matching `ossteam info`, `config list`, `jobs` and `events`: the Windows title's tool from the default, its chain with each tool's state and the choices; installed for `windows`; `compat.default_tool` refused for an unknown name with `config set`'s words, and emptied (no tool, *runs natively*); Proton chosen on the App page (*your choice*, `compat.tool.22`) and listed in Settings by the app's name; the first Play installing Proton as a job, then running through the runtime and Proton, `compatdata/22` made; Stop; Uninstall keeping `compatdata/22`, and with *Also delete the Proton prefix* deleting it; both settings reset. Fixed: choosing a tool put the chain above the choice and pushed the focused choice out of sight (a control focused by keyboard now stays in view as the page changes); Reset left focus at the page's top (now on the setting's control); an emptied setting showed its default as a placeholder, as if that applied | ⏳ | n/a | n/a |
| The library, the App page's artwork and every job look the same as the official client's for the same account | ⏳ | ⏳ | ⏳ |
| Without a daemon: the GUI says so and starts one; *Retry* reconnects after `ossteam daemon stop` | ✅ 2026-10-18, real install, no account: started a daemon; *Cannot reach ossteamd* with no reconnect of its own; Tab, Enter on *Retry* started one and the Login page's first field had focus again | ⏳ | ⏳ |
| Keyboard only: every page and control reachable with Tab and the §6 shortcuts | ✅ 2026-10-19, `gui_bench_states` bench daemon (fake CM and CDN), window of 1,007 × 537 points. 2026-10-18: the library (grid and list, arrows, Home, End, Page Down, Enter, Ctrl-F, Ctrl-Enter, Ctrl-1/2/3), a not-installed App page with its install chooser and an install error, Settings, and on the real install with no account the Login page and an empty Downloads page. 2026-10-19: the App page installed, running (Stop there and in the status bar) and with an update (Update, *Play anyway*), Verify, the launch-option chooser, a Proton choice and launch through the tool chain, Ctrl-Enter install, Uninstall with *Also delete the Proton prefix*, Escape and Cancel on each chooser; job rows (Pause, Resume, Cancel, the move buttons); the account menu with five accounts, Add account, Log out and a switch; the Login page's password, email code, authenticator code, device confirmation and QR. Fixed: Tab out of the account menu left it open; focus lost when a control disappeared, when a button disabled itself while its call ran, and when Pause and Resume swapped; a chooser closing dropped focus to the page; a Steam Guard prompt left the cursor out of its field; Add account kept the last username; Install and Uninstall left the Tab order while their chooser was open; a Windows game with several launch options and Proton could not be played (launch options qualified for Linux); a tool's job did not refresh the App pages using it; the status bar called a paused job queued; after Launch from the chooser, focus went back to the page's top as Play was replaced by Stop (now Play and Stop keep it either way) | ⏳ | ⏳ |
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
