---
name: discovery-entries-official-steam
description: An official Steam install owns ~/.steam/sdk64 on the dev machine, so launches fail with exit 8 until the maintainer decides
metadata:
  type: project
---

An official Steam install on the dev machine owns `~/.steam/sdk64`
(→ `~/.local/share/Steam/linux64`), so the daemon makes no ABI discovery
entries (SPEC §12.5) and every launch fails with exit 8.

**How to apply:** M1c and M1d live runs need the maintainer to decide what
to do. Don't work around it.
