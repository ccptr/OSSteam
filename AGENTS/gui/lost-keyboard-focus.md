---
name: lost-keyboard-focus
description: Seen once after erin's authenticator sign-in - the window redrew but took no keys; likely compositor focus
metadata:
  type: reference
---

Seen once and not reproduced in 4 tries: after erin's authenticator
sign-in on the gui_states bench, the window took no keys at all (not even
Ctrl-2) though it still redrew. Ctrl-2 is bound with no context, and
an action with nothing focused still reaches the app's root, so the
window most likely lacked the compositor's keyboard focus. (That run
was before the port to `ossteam-ui`; the same holds there.)
The login path opens no portal; only Settings' path picker does.

**How to apply:** next time, ask the compositor which window has focus
before restarting it.
