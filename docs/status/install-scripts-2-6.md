---
name: install-scripts-2-6
description: install-scripts.md §2.6 (unverifiable kvsignatures) is unanswered; it blocks launch-time HKLM writes and prefix install scripts
metadata:
  type: project
---

`kvsignatures` on install scripts can't be checked: the algorithm is
unpublished. install-scripts.md §2.6 proposes relying on the manifest
checks plus a UAC prompt per elevated step. The maintainer hasn't
answered.

It blocks M1c's launch-time HKLM writes and M1d's prefix install
scripts. M1b's removal does not depend on it.
