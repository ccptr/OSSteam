---
name: gui-keyboard-driving
description: Driving the GUI by keyboard with wtype - a fresh window has no focus; the tab order; don't export the bench's XDG_RUNTIME_DIR
metadata:
  type: reference
---

Driving the GUI by keyboard (e.g. `wtype -s 50`, every call: each
loses its first key otherwise): a fresh window has no focus.
Tab order from there: Library, Downloads, Settings, account menu,
search, …, grid. `PgUp`/`PgDn`/`Home`/`End` scroll pages. Don't export
the bench's `XDG_RUNTIME_DIR` in the shell that runs `wtype`. How focus
works inside the GUI: [focus](../gui/focus.md).
