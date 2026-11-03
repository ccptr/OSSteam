---
name: gui-own-toolkit
description: The GUI is on our own toolkit ossteam-ui (winit, ash/Vulkan plus our own software renderer, ADR 0004); no third-party GUI framework or widget kit
metadata:
  type: feedback
---

The GUI is built on `ossteam-ui`, our own toolkit in `crates/ossteam-ui/`
(SPEC GUI-1, [ADR 0004](../../docs/adr/0004-own-gui-toolkit.md)): winit
for windows and input, Vulkan through `ash` for drawing (not wgpu) and
our own software renderer beside it (not tiny-skia), shaders in Slang
compiled by `cargo xtask shaders`, taffy for layout, cosmic-text and
swash for text. The GUI's own controls
are in `crates/ossteam-gui/src/widgets.rs`. Do not add a third-party GUI
framework or widget kit (egui, iced and the like).

The toolkit's API is its own, not a copy of another framework's (the
maintainer, 2026-11-03): don't port another framework's code or mirror
its names. Write the GUI's views and widgets anew against `ossteam-ui`.

The port (ADR 0004 step 4) removed the previous framework from the
workspace on 2026-11-03; don't bring it, or its patches, back.
