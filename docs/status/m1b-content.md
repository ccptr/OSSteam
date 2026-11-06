---
name: m1b-content
description: M1b status - content engine, downloader, jobs and install scripts committed; most Linux live rows run 2026-11-04, the delta update 2026-11-06; CS2 matches the official tree once optional DLC is left out; a shared DLC depot and Windows left; cancelled first installs leave files
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
  were built in M1c on 2026-11-05 ([m1c-play](m1c-play.md)).
- Live on Linux, 2026-11-04 (m1b.md): info, a SIGKILL mid-download,
  verify, uninstall, a second folder, job controls, the bandwidth limit,
  two jobs at once and a beta branch. Counter-Strike 2's tree matched
  the official client's install of the same build, once optional DLC
  (`config/optionaldlc`, CS2's Workshop Tools) was left out unless
  `content.optional_dlc.<app>` names it, and a removed depot's
  directories that its manifest does not list were deleted too
  (`content.md` §6 rule 5, §10). Left live: a DLC depot that both
  clients install, compared, and the Windows column. No captures yet.
- Delta update, live 2026-11-06: CS2 from build 26393695 to 26445667,
  the official client's manifests and files afterwards, its download
  size within 256 bytes ([acceptance](../acceptance/m1b.md)). The run
  found three things, changed the same day: retries did not wait when
  every server had failed, so a burst of HTTP 503 right after the build
  appeared failed the automatic update in 25 s (now a retry waits for
  the first penalty to end, `content.md` §7); the server list was never
  fetched again after every server failed (now it is, §1); and `update`
  said "no updates to install" for an app whose failed job held it (now
  it names the job and how to resume it). The official client uses one
  `priority_class` of servers at a time (§1, written up; ossteam still
  rotates through all). Open for the maintainer: whether a failed
  automatic update should be retried later on its own; job 17 stood
  failed for two days.
- Open for the maintainer: cancelling a first install leaves the files
  already renamed into place in `apps/<installdir>` with no record
  (`ipc.md` §5.6), and `uninstall` refuses an app that is not installed,
  so nothing in ossteam removes them (seen with 22150: 349 files at 3 %).
