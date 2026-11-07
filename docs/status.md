# Status

Where each milestone stands, what is left, and what waits on the
maintainer: one note per fact in [status/](status/), one line per note
here. Keep it current: it is how a new session (agent or human) picks up
work without re-reading the diffs. See [AGENTS.md](../AGENTS.md) for the
rules and [agent-notes](../AGENTS/rules/agent-notes.md) for the note format.

## Waiting on the maintainer

Almost all remaining work needs one of these.

- [install-scripts.md §2.6](status/install-scripts-2-6.md) — accepted for now 2026-11-03; how `kvsignatures` work stays half open, for the maintainer
- [Own GUI toolkit](status/gui-toolkit-adr-0004.md) — ADR 0004 and its SPEC changes accepted 2026-11-03; spike done but for the maintainer's checks; toolkit (step 3) and the port (step 4) done, the previous framework removed, live checks passed 2026-11-03; the portal through our own D-Bus client (ADR amended); `main` since 2026-11-03 (the old `main` is the branch `gpui`), pushed; open: font fallback can map every font (a decision for the maintainer)
- [reference-hw.md §5](status/reference-hw-questions.md) — answered 2026-11-03: budgets judged on the dev PC (interim) until the Steam Deck is set up; download runs bypass the VPN and count against a speed test of the line
- [Spec review gates](status/spec-review-gates.md) — M3 pages reviewed 2026-11-03 with some questions left open (betas Q1/Q4/Q5, offline Q3); DRM and VAC not reviewed; friends needs a second test account
- [Steam Input spec](status/later-steam-input.md) — `docs/spec/input.md` drafted 2026-11-05; SDL 3 and our own Steam Controller driver chosen 2026-11-06, the Deck's controls in it; open: the Windows virtual gamepad, focus, own templates and glyphs, budgets
- [Big Picture and Game Mode](status/later-game-mode.md) — goals since 2026-11-06; open: the compositor Game Mode runs under (SPEC §24 Q5); `docs/spec/game-mode.md` not written yet
- [A Mac and real Windows](status/hardware-mac-windows.md) — for macOS acceptance and what Wine can't show; waits until closer to release (2026-11-03)
- [SPEC changes for CLI-6](status/spec-changes-cli-6.md) — local times in the CLI and the GUI (CLI-6) and the §5.1 line for `ossteam-ipc`; the wording and details need sign-off
- [Retrying a failed automatic update](status/retry-failed-auto-update.md) — a failed automatic update stays failed until resumed by hand (job 17, two days); whether the daemon should retry it on its own
- [Install scripts in a Proton prefix](status/install-scripts-in-prefix.md) — follow the official client, built and run live 2026-11-07; the SPEC.md lines for `Uninstall` (no `remove_compat_data`) need sign-off; tools at install and script order open

## Decisions the maintainer hasn't confirmed

When one is confirmed, write it into the spec and delete the note.

- [vtables from Proton](status/decision-vtables-from-proton.md) — MSVC order, overload runs reversed off Windows
- [ConnectToGlobalUser ownership](status/decision-connect-global-user-ownership.md) — refuses unowned apps; matters for Spacewar 480
- [No play session for unlaunched processes](status/decision-no-play-session-unlaunched.md) — PLAY-1
- [No offline stats cache in M1](status/decision-no-offline-stats-cache.md) — UpdateAvgRateStat unimplemented
- [Compat tools never mapped](status/decision-compat-tools-never-mapped.md) — else SLR would map to Proton
- [`Client::launch` has no timeout](status/decision-launch-no-timeout.md) — tool downloads can be long
- [Update detection](status/decision-update-detection.md) — skips empty depot sets, counts platform changes
- [Native Linux gets scout SLR](status/decision-native-linux-scout.md) — `native` at priority 75

## Milestones

- [M0: Foundations](status/m0-foundations.md) — acceptance items closed; startup flash and DPI fixed
- [M1a: Session and library](status/m1a-session-library.md) — idle wakeups fixed; live runs left
- [M1b: Content](status/m1b-content.md) — committed; most Linux live rows run 2026-11-04; CS2 matches the official tree once optional DLC is left out, and its delta update to a new build matched too (2026-11-06; retries now wait once every server has failed); a shared DLC depot and Windows left; a question on cancelled first installs
- [M1c: Play (native)](status/m1c-play.md) — shim, stats, storage, tickets and install scripts at the start committed (scripts 2026-11-05, run live through Proton); Spacewar gets past `SteamAPI_Init` under Proton since 2026-11-05 and stops at `ISteamController::Init` (for the maintainer); live runs and real Windows left
- [Discovery entries](status/discovery-entries-official-steam.md) — official Steam owns `~/.steam/sdk64`; a private `~/.steam` per launch (DISC-4 to DISC-6) accepted 2026-11-03; DISC-4 built and run live 2026-11-03 (scout, legacy scout, Proton, CS2 in sniper); CS2 reaches its Game Coordinator 2026-11-04, and its relay network comes up with ossteam's certificate (`SteamNetworkingSocketsSerialized005`, `SteamAppDisableUpdate001` and `ISteamHTTP` with its cache added the same day); DISC-5 and DISC-6 built 2026-11-04; the take-over run live against the official client 2026-11-05, and follows the client's close at once since the same day
- [M1d: Compat tools](status/m1d-compat-tools.md) — committed; install scripts in the prefix run live with Spacewar 2026-11-05; ossteam's own `steamclient.dll` in the prefix since the same day; since 2026-11-07 they follow the official client (applied at install and verify, the prefix deleted at uninstall); the remaining live rows left
- [M1e: GUI and macOS](status/m1e-gui-macos.md) — committed; logged-in checks, macOS runs and §20 rows left
- [M2: Steam DRM](status/m2-drm.md) — drm.md drafted, not reviewed
- [M2: VAC](status/m2-vac.md) — vac.md drafted, not reviewed
- [M3: Cloud](status/m3-cloud.md) — cloud.md drafted; gated on §9 and §8
- [M3: Friends](status/m3-friends.md) — friends.md drafted; needs a second account
- [M3: Betas](status/m3-betas.md) — betas.md drafted; needs a published beta code
- [M3: Workshop](status/m3-workshop.md) — workshop.md drafted; subscribing is visible, ask first
- [M3: Offline](status/m3-offline.md) — offline.md drafted; offline daemon has an empty library today
- [Later: Steam Input](status/later-steam-input.md) — a goal since 2026-11-05, as the embeddable LGPL library `ossteam-input`; `input.md` drafted 2026-11-05, awaiting review (nine questions); no milestone or code yet
- [Later: Big Picture and Game Mode](status/later-game-mode.md) — goals since 2026-11-06 (SPEC §1.1 goal 4); no spec page, milestone or code yet
