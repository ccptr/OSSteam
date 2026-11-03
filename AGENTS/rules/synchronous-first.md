---
name: synchronous-first
description: Synchronous first (SPEC §2.3); async only behind a sync facade around a dependency, or via an ADR; the gpui GUI until ADR 0004's port
metadata:
  type: feedback
---

SPEC §2.3. Synchronous first. Async only inside a dependency with no
sync alternative, behind a synchronous facade (CONC-3), or via an ADR.
The GUI is synchronous too once ADR 0004's port lands; until then its
gpui code keeps `cx.spawn`.
