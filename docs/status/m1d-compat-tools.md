---
name: m1d-compat-tools
description: M1d status - compat tool list, mappings, chain launch committed; prefix install scripts and real Proton runs left
metadata:
  type: project
---

Committed: spec `docs/spec/compat-tools.md` (682de1c); tool list,
mappings, manifests and chains in `ossteam-launch/src/compat/`
(02c4a02); settings and install platform, schema 6 (4e7086a); chain
launch with a deferred Launch reply while tool jobs run (e46bba8); GetApp
`compat`, `info` output and `uninstall --remove-compat-data`, schema 7
(67d6d46); steamclient.so copies and the abstract socket
`ossteam-game-<uid>` (a5fbf2f); runbook `docs/acceptance/m1d.md`
(3b87938). Daemon tests: `crates/ossteamd/src/testing/m1d.rs`.

Left:
- Install scripts in the prefix (compat-tools §7), blocked on
  [install-scripts-2-6](install-scripts-2-6.md).
- Live runs. Nothing has run under a real Proton, SLR or container yet.
- The `use_tool_subprocess_reaper` SIGINT stop path is untested against
  a real tool.
