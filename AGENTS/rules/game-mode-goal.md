---
name: game-mode-goal
description: Big Picture and Game Mode are goals (SPEC §1.1 goal 4); Game Mode is Linux-only; the compositor it runs under (gamescope, our own on DRM/KMS, or similar) is open (§24 Q5); spec page first; no milestone yet
metadata:
  type: feedback
---

Big Picture and Game Mode are explicit goals (SPEC §1.1 goal 4, the
maintainer, 2026-11-06). Big Picture is the full-screen, controller-driven
interface as an app on the desktop, on all three platforms. Game Mode is
that interface as the whole session a device starts into, as SteamOS
does: ossteam is the shell. It is Linux-only (PLAT-2,
[cross-platform](cross-platform.md)).

Both are built on `ossteam-ui` ([gui-own-toolkit](gui-own-toolkit.md))
and driven by controllers through `ossteam-input`
([steam-input-goal](steam-input-goal.md)). The overlay inside games
still needs its VAC-4 ADR.

Don't settle which compositor Game Mode runs under: gamescope, a
compositor of our own drawing through DRM/KMS, or something similar.
The maintainer left it open (SPEC §24 Q5). Write
`docs/spec/game-mode.md` and get it reviewed before code
([spec-first](spec-first.md)); gamescope is a source for facts only
([research sources](../research-sources.md)). No milestone yet (§21 Later).
