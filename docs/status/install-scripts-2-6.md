---
name: install-scripts-2-6
description: install-scripts.md §2.6 accepted for now on 2026-11-03 (manifest checks plus a UAC prompt per elevated step); how kvsignatures work stays half open, for the maintainer to look into
metadata:
  type: project
---

`kvsignatures` on install scripts can't be checked: the algorithm is
unpublished. install-scripts.md §2.6 relies on the manifest checks plus
a UAC prompt per elevated step instead.

On 2026-11-03 the maintainer accepted that for now, and will find out
later how the signatures work. The spec says so.

**How to apply:** the launch-time HKLM writes (M1c) and the prefix
install scripts (M1d) may be built on §2.6 as written. Keep the
signature check a separate step, so it can be added if the algorithm is
learned. Delete this note when the maintainer closes the question.
