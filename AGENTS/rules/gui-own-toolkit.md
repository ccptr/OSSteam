---
name: gui-own-toolkit
description: The GUI moves to our own toolkit ossteam-ui (winit, ash/Vulkan plus our own software renderer, ADR 0004); no third-party GUI framework or widget kit; gpui only until the port
metadata:
  type: feedback
---

The GUI is built on `ossteam-ui`, our own toolkit in `crates/ossteam-ui/`
(SPEC GUI-1, [ADR 0004](../../docs/adr/0004-own-gui-toolkit.md)): winit
for windows and input, Vulkan through `ash` for drawing (not wgpu) and
our own software renderer beside it (not tiny-skia), shaders in Slang
compiled by `cargo xtask shaders`, taffy for layout, cosmic-text and
swash for text. The GUI's own widgets
stay in `crates/ossteam-gui/src/widgets/`. Do not add a third-party GUI
framework or widget kit (`gpui-component`, egui, iced and the like).

The toolkit's API is its own, not a copy of gpui's (the maintainer,
2026-11-03): don't port gpui code or mirror its names. Write the GUI's
views and widgets anew against `ossteam-ui`.

ADR 0004 is accepted, but its port has not landed: `ossteam-gui` is
still on gpui. Fix bugs there as before, but don't build new gpui-only
machinery the port would have to redo.
