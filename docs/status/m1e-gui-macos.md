---
name: m1e-gui-macos
description: M1e status - GUI, macOS discovery and artwork committed; logged-in checks, macOS runs and §20 rows left
metadata:
  type: project
---

Committed: spec `docs/spec/gui.md` and IPC schema 8 (6e8c005); GUI
connection, store, pages and widgets (3ce5c9a); measurable §20 GUI
budgets and runbook `docs/acceptance/m1e.md` (c21e8f8); macOS discovery
(daca257, 9487c6a; see [macos-discovery](../../AGENTS/steamworks/macos-discovery.md)); App page
artwork through the bounded cache (df34a07); gui.md §7 integration test
`testing::gui` in ossteamd (145c5e2: ossteam-gui is now a lib plus a
thin bin and a dev-dependency of ossteamd; the proptest git source is
patched to crates.io). Compare LibraryFolder without `free_bytes`,
which changes under the parallel suite.

- Keyboard-only row ✅ on Linux (887ef16); see [focus](../../AGENTS/gui/focus.md).
- No-daemon/Retry row ✅ on Linux on the real install (bc81f09).
- The M1a–M1d rows were dry-run on the gui_states bench (4a986d3,
  943535e, 22fe26d, abf4bdb..e86b78f) and recorded in m1e.md. Each still
  needs its live run.
- The GUI is to move off gpui ([gui-toolkit-adr-0004](gui-toolkit-adr-0004.md));
  rows passed on gpui are run again after the port.
- Open, seen once: [lost-keyboard-focus](../../AGENTS/gui/lost-keyboard-focus.md).
- Windows cross-clippy of the workspace is clean (706f5d8). The fake
  CM's QR auto-approval is Linux-only.

Left: the logged-in GUI checks
([test-account-logged-out](test-account-logged-out.md)), the macOS
acceptance runs ([hardware-mac-windows](hardware-mac-windows.md)), and
the §20 rows ([reference-hw-questions](reference-hw-questions.md)).
