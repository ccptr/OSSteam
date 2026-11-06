---
name: later-steam-input
description: Steam Input - a goal since 2026-11-05 (SPEC §1.1, §21 Later), as the embeddable LGPL library ossteam-input; no spec page, no milestone, no code yet
metadata:
  type: project
---

The maintainer made Steam Input an explicit goal on 2026-11-05 (SPEC
§1.1 goal 3), built as `ossteam-input`, a library other programs can
embed without the rest of ossteam, licensed LGPL-2.1-or-later (LIC-9).
`ossteam-ui`, `ossteam-dbus` and `ossteam-vdf` (so `ossteam-input` can
read configuration files with it) were relicensed to LGPL-2.1-or-later
the same day. Steam Input is listed under §21 Later, with no milestone.

Nothing is written yet. The order (§21 Later): `docs/spec/input.md`,
reviewed by the maintainer; the library; then the daemon's use of it,
`ISteamInput` and `ISteamController` in the shim, and settings in the
CLI and GUI. Workshop and cloud configurations wait for M3.

Until then `ISteamController::Init` fails (ABI-3), which stops Spacewar
under Proton ([m1c-play](m1c-play.md)).
