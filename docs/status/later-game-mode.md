---
name: later-game-mode
description: Big Picture and Game Mode - goals since 2026-11-06 (SPEC §1.1 goal 4, §21 Later); Game Mode Linux-only; the compositor is open (§24 Q5); no spec page, milestone or code
metadata:
  type: project
---

The maintainer made Big Picture and Game Mode explicit goals on
2026-11-06 (SPEC §1.1 goal 4), and Big Picture left the non-goals
(§1.2). Game Mode joins compatibility tools as Linux-only (PLAT-2). The
order (§21 Later): `docs/spec/game-mode.md`, reviewed; Big Picture on
all three platforms, after `ossteam-input`; then the Game Mode session.

Open, for the maintainer: the compositor Game Mode runs under
(gamescope, our own on DRM/KMS, or similar; SPEC §24 Q5). The spec page
is not written yet. `input.md` points there for what the Steam and Quick
Access buttons do, and Q1 there (the Steam Deck's controls) now leans
on Game Mode needing them.

Until Big Picture exists, `ISteamUtils::IsSteamInBigPictureMode` stays
false, which is true (`docs/spec/steamworks/isteamutils.md`).
