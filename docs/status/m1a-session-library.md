---
name: m1a-session-library
description: M1a status - idle wakeups fixed (f24cdc6), GUI row dry-run; only the live runs are left
metadata:
  type: project
---

Idle wakeups (§20): the CM I/O thread capped every poller wait at the
1 s CONC-2 slice, so it woke 1.03 times a second when logged on and
idle. `CancelToken::on_cancel` (ossteam-types) now wakes the poller:
0.14/s for CM I/O, 0.23/s for the whole daemon (f24cdc6; see
[conc-2-reword](conc-2-reword.md)). Dev-box numbers are in m1a.md (CLI
p95 2.4 ms), labelled as not results. The GUI row was dry-run on the
gui_states bench (4a986d3).

Left: the live runs. The test account is signed in again since
2026-11-03 (a wrong password, *Remember me* off and QR passed live
through the GUI that day; see
[gui-toolkit-adr-0004](gui-toolkit-adr-0004.md)).
