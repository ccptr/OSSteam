---
name: lgpl-libraries
description: ossteam-vdf, ossteam-ui, ossteam-dbus, ossteam-input and ossteam-steamcontroller are LGPL-2.1-or-later (SPEC LIC-9); their files take the LGPL header; they never depend on a GPL workspace crate
metadata:
  type: feedback
---

`ossteam-vdf`, `ossteam-ui`, `ossteam-dbus` and, once written,
`ossteam-input` and `ossteam-steamcontroller` are LGPL-2.1-or-later so
other programs can embed them (SPEC LIC-9, the maintainer, 2026-11-05;
`ossteam-vdf` the same day; `ossteam-steamcontroller` 2026-11-06). Everything else stays GPL-2.0-or-later.

- New files in these crates start with
  `// SPDX-License-Identifier: LGPL-2.1-or-later`.
- They depend on no GPL workspace crate. Relicensing one so they can is
  a SPEC change for the maintainer; don't do it on your own.
- Don't copy code into them from the GPL crates.
- `cargo xtask check-spdx` checks headers, crate licenses and the
  dependency rule; add `ossteam-input` and `ossteam-steamcontroller` to
  its `LGPL_CRATES` when they are created.
