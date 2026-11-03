# Status

Where each milestone stands, what is left, and what waits on the
maintainer: one note per fact in [status/](status/), one line per note
here. Keep it current: it is how a new session (agent or human) picks up
work without re-reading the diffs. See [AGENTS.md](../AGENTS.md) for the
rules and [agent-notes](../AGENTS/rules/agent-notes.md) for the note format.

## Waiting on the maintainer

Almost all remaining work needs one of these.

- [CI fix waits on a push](status/waiting-push-macos-discovery-fix.md) — macOS CI is red on origin/main until 9487c6a runs
- [Test account logged out](status/test-account-logged-out.md) — since 2026-10-18; blocks every live run and logged-in GUI check
- [Discovery entries](status/discovery-entries-official-steam.md) — official Steam owns `~/.steam/sdk64`, so launches fail with exit 8
- [install-scripts.md §2.6](status/install-scripts-2-6.md) — unverifiable `kvsignatures`; blocks HKLM writes and prefix scripts
- [Own GUI toolkit](status/gui-toolkit-adr-0004.md) — ADR 0004 and its SPEC changes proposed 2026-11-02, not reviewed; the spike is nearly done on a branch; replacing gpui waits on the review, cold fonts and the spike's Left list
- [CONC-2 reworded](status/conc-2-reword.md) — SPEC rule and ADR 0001 amended in 471d6d3, not reviewed
- [reference-hw.md §5](status/reference-hw-questions.md) — 5 questions; no §20 row until a reference machine is chosen
- [Spec review gates](status/spec-review-gates.md) — the M2/M3 spec pages awaiting review before code
- [A Mac and real Windows](status/hardware-mac-windows.md) — for macOS acceptance and what Wine can't show

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
- [M1d: Compat tools](status/m1d-compat-tools.md) — committed; prefix scripts and real Proton runs left
- [M1e: GUI and macOS](status/m1e-gui-macos.md) — committed; logged-in checks, macOS runs and §20 rows left
- [M2: Steam DRM](status/m2-drm.md) — drm.md drafted, not reviewed
- [M2: VAC](status/m2-vac.md) — vac.md drafted, not reviewed
- [M3: Cloud](status/m3-cloud.md) — cloud.md drafted; gated on §9 and §8
- [M3: Friends](status/m3-friends.md) — friends.md drafted; needs a second account
- [M3: Betas](status/m3-betas.md) — betas.md drafted; needs a published beta code
- [M3: Workshop](status/m3-workshop.md) — workshop.md drafted; subscribing is visible, ask first
- [M3: Offline](status/m3-offline.md) — offline.md drafted; offline daemon has an empty library today
