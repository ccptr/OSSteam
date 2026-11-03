---
name: test-account-logged-out
description: The test account has been logged out since 2026-10-18; every live run and logged-in GUI check waits on the maintainer
metadata:
  type: project
---

The test account is logged out since a failed resume lost its token on
2026-10-18. The cause is fixed in e4019cc, but the fix is verified only
against the fake CM. Still logged out on 2026-11-02. Account details are
in the local notes.

**Why it matters:** every live run and every logged-in GUI check waits on
it, and only the maintainer can sign the account in again.

**How to apply:** check with `ossteam --no-autostart status` before
planning live work, and don't restart the daemon casually
([daemon-restarts](../../AGENTS/rules/daemon-restarts.md)).
