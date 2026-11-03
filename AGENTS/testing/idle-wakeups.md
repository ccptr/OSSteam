---
name: idle-wakeups
description: Measure idle wakeups per thread from voluntary_ctxt_switches in /proc/<pid>/task/*/status
metadata:
  type: reference
---

Measure idle wakeups per thread from `voluntary_ctxt_switches` in
`/proc/<pid>/task/*/status`. Don't use the bench daemon's RSS
([bench-tests](bench-tests.md)).
