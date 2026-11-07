---
name: spec-changes-cli-6
description: SPEC changes awaiting the maintainer's sign-off - new rule CLI-6 (the CLI and the GUI show times in the local time zone) and the reworded §5.1 line for ossteam-ipc; the behaviour was asked for, the wording and chosen details were not
metadata:
  type: project
---

These SPEC changes were made by agents and need the maintainer's
sign-off ([spec-changes](../../AGENTS/rules/spec-changes.md)):

- **New rule CLI-6** (SPEC §15.1, after CLI-5): the CLI and the GUI show
  times in the local time zone. Added in 465eaca (2026-11-06), built in
  19042b6; today it covers the branch "updated" dates of `info` and the
  GUI's App page.
- **SPEC §5.1**, the crate list: `ossteam-ipc` is now described as
  "client IPC protocol + blocking client library; the leaf clients'
  local time (CLI-6)", since the CLI and the GUI may depend only on
  `ossteam-ipc` and `ossteam-types`, and `ossteam-types` does no I/O.
  Changed in 465eaca.

The maintainer asked for it on 2026-11-06 ("read the time zone"). What
needs sign-off is the wording and the details the agents chose: a clock
time names its zone (`UTC`, a letters-only abbreviation, or
`UTC±HH:MM`, always the offset on Windows); times fall back to UTC when
the zone cannot be read; `--json` keeps Unix times.
