---
name: no-migrations-before-release
description: Don't write migrations of ossteam's own files, folders or database into ossteam until release; change layouts and formats directly
metadata:
  type: feedback
---

Until the first release, don't write code that converts ossteam's own
files, folders or database from an earlier layout or format. Change the
layout directly, and say in the spec that older data is not converted.

**Why:** the maintainer said so on 2026-11-03, when library folders'
`common/` became `apps/`. Nobody but the maintainer has data to carry
over yet.

**How to apply:** where the maintainer's own data on the dev machine
needs it, convert it by hand (for example `mv common apps` with the
daemon stopped), not in ossteam.
