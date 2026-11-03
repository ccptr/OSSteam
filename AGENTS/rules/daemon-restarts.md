---
name: daemon-restarts
description: Don't restart the daemon casually; a failed resume once lost the account's token and only the maintainer can sign in again
metadata:
  type: feedback
---

Don't restart the daemon casually.

**Why:** a failed resume once lost the account's token (fixed in
e4019cc). If the account is logged out, only the maintainer can sign it
in again.

**How to apply:** to only look, use `ossteam --no-autostart status`
([daemon-autostart](../testing/daemon-autostart.md)).
