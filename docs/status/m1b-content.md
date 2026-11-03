---
name: m1b-content
description: M1b status - content engine, downloader, jobs and install scripts committed; real Windows and live runs left
metadata:
  type: project
---

Committed: engine, downloader, depot selection, daemon jobs and folders,
CLI, fake-CM/CDN daemon tests (`crates/ossteamd/src/testing/m1b.rs`),
and install scripts at verify/uninstall
(`crates/ossteamd/src/content/script/`, schema v5): 7450efa, 62acc30,
5b6db26, 8cdc09f, 8189514. Runbook: `docs/acceptance/m1b.md`. GUI row
dry-run on the bench (abf4bdb..e86b78f; the bench gained Fresh Extra DLC
with its own depot). A resumed job's progress continues (4d9e34a;
journal OSJ2 stores bytes downloaded, OSJ1 is still read).

Left:
- The Windows script code (registry, CreateProcess/ShellExecuteEx,
  `--script-helper` via runas) passes under Wine but has never run on
  real Windows.
- Launch-time script steps (registry writes, firewall, run process)
  belong to M1c ([install-scripts-2-6](install-scripts-2-6.md)).
- No live runs or captures against Steam yet.
