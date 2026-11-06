---
name: project
description: What ossteam is, its parts, and where the spec, subsystem specs, runbooks and ADRs live
metadata:
  type: reference
---

ossteam is a clean Rust rewrite of the Steam client, GPL-2.0-or-later
(its embeddable libraries LGPL-2.1-or-later, SPEC LIC-9). A
per-user daemon, `ossteamd`, owns the Steam session, the library and game
processes. Thin clients drive it: the `ossteam` CLI (clap) and
`ossteam-gui`, on our own toolkit `ossteam-ui` (ADR 0004).

- [SPEC.md](../SPEC.md): the design, its rules (IDs like CONC-2, PLAT-1)
  and the milestones (§21).
- `docs/spec/`: subsystem specs. `docs/acceptance/`: per-milestone
  runbooks and results. `docs/adr/`: decisions.
- `docs/release-checklist.md`: things switched off during development.
