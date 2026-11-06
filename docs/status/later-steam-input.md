---
name: later-steam-input
description: Steam Input - a goal since 2026-11-05 (SPEC §1.1, §21 Later), as the embeddable LGPL library ossteam-input; docs/spec/input.md drafted 2026-11-05, awaiting review; no milestone, no code
metadata:
  type: project
---

The maintainer made Steam Input an explicit goal on 2026-11-05 (SPEC
§1.1 goal 3), built as `ossteam-input`, a library other programs can
embed without the rest of ossteam, licensed LGPL-2.1-or-later (LIC-9).
`ossteam-ui`, `ossteam-dbus` and `ossteam-vdf` (so `ossteam-input` can
read configuration files with it) were relicensed to LGPL-2.1-or-later
the same day. Steam Input is listed under §21 Later, with no milestone.

`docs/spec/input.md` was drafted on 2026-11-05 and awaits the
maintainer's review: its §13 has nine questions (SDL 3 for device access,
the virtual gamepad on Windows and macOS, focus under Wayland, templates
and glyph art of our own, reading the user's server-side choice before
M3, reports to Valve, `Init` with no controllers, budgets, SPEC ids).
Its §11 lists what must be observed in the official client first; the
development computer has a 2026 Steam Controller with its receiver.

The order (§21 Later): the page, reviewed; the library; then the
daemon's use of it, `ISteamInput` and `ISteamController` in the shim,
and settings in the CLI and GUI. Workshop and cloud configurations wait
for M3. No code before the review ([steam-input-goal](../../AGENTS/rules/steam-input-goal.md)).

Until then `ISteamController::Init` fails (ABI-3), which stops Spacewar
under Proton ([m1c-play](m1c-play.md)).
