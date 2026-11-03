---
name: hardware-mac-windows
description: macOS acceptance runs need a Mac, and what Wine can't show needs a real Windows machine
metadata:
  type: project
---

The macOS acceptance runs need a Mac. What Wine can't show (UAC, runas
`--script-helper`, real HKLM, firewall behaviour) needs a real Windows
machine. Both wait on the maintainer.
