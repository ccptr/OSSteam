---
name: windows-wine
description: Cross-check Windows and run the workspace tests under Wine; what needs OSSTEAM_ABI_GAME, GUI runs on the headless sway in a scratch prefix with fonts, and what Wine can't cover
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

GUI runs (the `ossteam-ui` spike):

- Run them on the headless sway's Xwayland (`env -u WAYLAND_DISPLAY
  DISPLAY=:N wine …`, `N` from `ls -la /tmp/.X11-unix`, the socket
  created when that sway started), never on the desktop's display.
- Use a scratch prefix (`WINEPREFIX=<scratch>/wineprefix wineboot -i`
  with `WINEDLLOVERRIDES="mscoree,mshtml="`) and copy fonts into its
  `drive_c/windows/Fonts` (`/usr/share/wine/fonts/tahoma*.ttf`, Noto
  Sans): a new prefix has none there, and font loading finds nothing.
  `wineserver -k` with the same `WINEPREFIX` afterwards.
- Under Wine 11.21 any winit window's event loop spins (about 30,000
  wakeups a second: Wine keeps `QS_PAINT` set with no message to
  fetch), so idle wakeups, memory and thread switches can't be measured
  there. winevulkan, both renderers, frame times and the clipboard can.
- Wine reads `xdotool`'s keys through its own keymap: letters can
  arrive as other keys (`c` as `AudioVolumeDown`). Check the spike's
  input log before blaming the app.
