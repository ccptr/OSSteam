---
name: steam-input-goal
description: Steam Input is a goal (SPEC §1.1), built as ossteam-input, a modular LGPL library other programs can embed, reading controllers through SDL 3 and our own driver for the Steam Controllers and the Deck's controls; spec page first; no milestone yet
metadata:
  type: feedback
---

Steam Input is an explicit goal (SPEC §1.1, the maintainer, 2026-11-05).
It is built as `ossteam-input`, a library that other programs can embed
without the daemon, the Steam session or the rest of ossteam, so keep its
API free of ossteam's own types and depend on no other workspace crate
but `ossteam-vdf` (WS-1). It is LGPL: [lgpl-libraries](lgpl-libraries.md).

It reads other makers' controllers through SDL 3, and the Steam
Controllers (2015, 2026) and the Steam Deck's controls (2026-11-06)
through `ossteam-steamcontroller`, our own
driver that other projects can reuse: LGPL, pure Rust, no workspace
dependencies, protocol pages first (the maintainer, 2026-11-06; SPEC
§5.3, `input.md` §6.1, §6.5). The shim never links either (WS-2).

It has no milestone yet (§21 Later). Write and get review of
`docs/spec/input.md` before code ([spec-first](spec-first.md)); until
then `ISteamInput` and `ISteamController` fail honestly (ABI-3).
