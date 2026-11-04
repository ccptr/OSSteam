---
name: m1b-content
description: M1b status - content engine, downloader, jobs and install scripts committed; most Linux live rows run 2026-11-04; CS2 matches the official tree once optional DLC is left out; a shared DLC depot, delta and Windows left; cancelled first installs leave files
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
- Live on Linux, 2026-11-04 (m1b.md): info, a SIGKILL mid-download,
  verify, uninstall, a second folder, job controls, the bandwidth limit,
  two jobs at once and a beta branch. Counter-Strike 2's tree matched
  the official client's install of the same build, once optional DLC
  (`config/optionaldlc`, CS2's Workshop Tools) was left out unless
  `content.optional_dlc.<app>` names it, and a removed depot's
  directories that its manifest does not list were deleted too
  (`content.md` §6 rule 5, §10). Left live: a DLC depot that both
  clients install, compared, a delta update of a new build, and the
  Windows column. No captures yet.
- Open for the maintainer: cancelling a first install leaves the files
  already renamed into place in `apps/<installdir>` with no record
  (`ipc.md` §5.6), and `uninstall` refuses an app that is not installed,
  so nothing in ossteam removes them (seen with 22150: 349 files at 3 %).
