---
name: synchronous-first
description: Synchronous first (SPEC §2.3); async only behind a sync facade around a dependency, or via an ADR; the GUI too
metadata:
  type: feedback
---

SPEC §2.3. Synchronous first. Async only inside a dependency with no
sync alternative, behind a synchronous facade (CONC-3), or via an ADR.
The GUI is synchronous too (ADR 0004): blocking work, such as daemon
calls and the folder picker, runs on threads whose results come back
to the event loop as messages.
