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
- [A Mac and real Windows](status/hardware-mac-windows.md) — for macOS acceptance and what Wine can't show; waits until closer to release (2026-11-03)

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
- [M1b: Content](status/m1b-content.md) — committed; real Windows and live runs left
- [M1c: Play (native)](status/m1c-play.md) — shim, stats, storage, tickets committed; script steps and live runs left
- [Discovery entries](status/discovery-entries-official-steam.md) — official Steam owns `~/.steam/sdk64`; a private `~/.steam` per launch (DISC-4 to DISC-6) accepted 2026-11-03; DISC-4 built and run live 2026-11-03 (scout, legacy scout, Proton, CS2 in sniper); the Game Coordinator is in no spec; DISC-5 and DISC-6 built 2026-11-04; the take-over not yet run against the real official client
- [M1d: Compat tools](status/m1d-compat-tools.md) — committed; prefix scripts and real Proton runs left
- [M1e: GUI and macOS](status/m1e-gui-macos.md) — committed; logged-in checks, macOS runs and §20 rows left
- [M2: Steam DRM](status/m2-drm.md) — drm.md drafted, not reviewed
- [M2: VAC](status/m2-vac.md) — vac.md drafted, not reviewed
- [M3: Cloud](status/m3-cloud.md) — cloud.md drafted; gated on §9 and §8
- [M3: Friends](status/m3-friends.md) — friends.md drafted; needs a second account
- [M3: Betas](status/m3-betas.md) — betas.md drafted; needs a published beta code
- [M3: Workshop](status/m3-workshop.md) — workshop.md drafted; subscribing is visible, ask first
- [M3: Offline](status/m3-offline.md) — offline.md drafted; offline daemon has an empty library today
