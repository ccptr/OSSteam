---
name: focus
description: How keyboard focus works in the GUI - named focus handles, shared ids, the page focus, reveal and page scrolling
metadata:
  type: reference
---

The keyboard-only row first passed on Linux on gpui (887ef16); the
`ossteam-ui` port keeps the same behaviour with the toolkit's own
machinery.

- Every control's focus handle is named after its id
  (`FocusHandle::named`), so a control that replaces another with the
  same id keeps the focus: Pause/Resume, Play/Play anyway/Stop.
- A confirm row's Cancel takes the focus handle of the button that
  opened it (Log out…, Stop ossteamd), so a second Enter cancels rather
  than confirms.
- A control that goes away leaves focus on its page (`Gui::focus_lost`);
  Tab from a focused element that is not a tab stop goes on to the next
  stop after it, Shift-Tab to the one before.
- Choosers return focus to what opened them, with the ring if the
  keyboard opened them (`pages/app.rs`, `close_chooser`).
- The toolkit keeps the keyboard-focused control in view, and an
  element marked `reveal_when_shown` (errors, choosers) is scrolled
  into view when it appears near focus.
- `widgets::scrolling_page` handles PgUp/PgDn/Home/End through the
  `ScrollingPage` key context. An element has one key context, so a
  page with its own (the App page's Escape) wraps the scrolling page.
