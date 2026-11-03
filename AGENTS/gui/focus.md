---
name: focus
description: How keyboard focus works in the GUI widgets - focus handles, shared ids, reveal and page scrolling
metadata:
  type: reference
---

The keyboard-only row passed on Linux (887ef16) after many focus fixes
(9db2176, 092afb6, a0656d5, 758b0aa, bd1bff5, …).

- Buttons, switches and choices own a focus handle.
- Controls that replace each other (Pause/Resume, Play/Play anyway/Stop)
  share an id so focus survives.
- `widgets/reveal.rs` keeps the focused control in view.
- `widgets/page_scroll.rs` handles PgUp/PgDn/Home/End.
