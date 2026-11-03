---
name: windows-wine
description: Cross-check Windows and run the workspace tests under Wine; what needs OSSTEAM_ABI_GAME and what Wine can't cover
metadata:
  type: reference
---

Windows: `cargo check --target x86_64-pc-windows-gnu --workspace` (and
clippy). Tests under Wine:

```sh
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine WINEDEBUG=-all \
  cargo test --offline --target x86_64-pc-windows-gnu --workspace
```

- The steamclient ABI test (`ossteam-steamclient/tests/abi.rs`) needs
  `OSSTEAM_ABI_GAME` set to a game built for the target's arch (commands
  in abi.rs's docs); without it, it tries `clang-cl` and fails under
  Wine. See [abi-test](../steamworks/abi-test.md).
- Registry tests write under `HKCU\Software\ossteam-test` and clean up
  after themselves (`grep ossteam-test ~/.wine/user.reg` should come up
  empty).
- Not covered under Wine: `reaper.rs` (Linux only) and
  `ossteam-steamclient/tests/pump.rs` (Unix only).
- Wine is not Windows: UAC, runas `--script-helper`, real HKLM and
  firewall behaviour stay unverified until a real Windows run.
