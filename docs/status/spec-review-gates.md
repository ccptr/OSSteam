---
name: spec-review-gates
description: M3 spec pages reviewed 2026-11-03 with some questions left open; DRM and VAC (M2) not reviewed
metadata:
  type: project
---

No DRM, VAC, cloud, friends, betas, workshop or offline code before the
maintainer has reviewed that spec page and answered its questions
([review-gates](../../AGENTS/rules/review-gates.md)).

On 2026-11-03 the maintainer answered the M3 pages' questions, each
answer written under its question:

- cloud.md §9: [m3-cloud](m3-cloud.md); Q1 (`~/.SteamCloud`) left until a
  game uses that root.
- friends.md §11: [m3-friends](m3-friends.md); all answered.
- betas.md §9: [m3-betas](m3-betas.md); Q1, Q4 and Q5 open.
- workshop.md §11: [m3-workshop](m3-workshop.md); all answered.
- offline.md §10: [m3-offline](m3-offline.md); Q3 open.

Not reviewed yet: drm.md §5 ([m2-drm](m2-drm.md)) and vac.md §7
([m2-vac](m2-vac.md)).

M3's order is now cloud, friends and chat, offline, Workshop, betas
(SPEC §21).

**How to apply:** code for an answered page may land. Code that depends
on an open question waits for its answer. Most M3 pages also need live
observations first; each note says which.
