---
name: hardware-mac-windows
description: macOS acceptance runs need a Mac (none yet), and what Wine can't show needs real Windows (the dev PC dual-boots it, closer to release); both wait until then
metadata:
  type: project
---

The macOS acceptance runs need a Mac. What Wine can't show (UAC, runas
`--script-helper`, real HKLM, firewall behaviour) needs a real Windows
machine. Both wait on the maintainer.

On 2026-11-03 the maintainer said these runs can wait. No Mac is
available yet. Windows is on the development PC as a dual boot, and the
maintainer will boot into it for testing closer to release.
