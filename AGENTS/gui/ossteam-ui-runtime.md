---
name: ossteam-ui-runtime
description: ossteam-ui runtime pitfalls found the hard way - winit's pre_present_notify only right before a present, drop the clipboard before winit's connection, a scrolled layer is damage, scroll children don't shrink, cosmic-text's fallback maps every face it tries, the gallery example
metadata:
  type: reference
---

Pitfalls in `crates/ossteam-ui` that cost a debugging round each
(2026-11-03, ADR 0004 step 3):

- **`pre_present_notify` only right before presenting.** On Wayland
  winit then waits for a frame callback before it delivers another
  `RedrawRequested`. If the software renderer then skips the frame
  (nothing changed), nothing is committed, the callback never comes and
  the window stops redrawing. Each renderer calls it itself; `app.rs`
  must not.
- **Drop the clipboard before winit's display connection.** The Wayland
  clipboard's worker thread segfaults in `wl_proxy_destroy` if it
  outlives the event loop. `app.rs` drops the window, renderer and
  clipboard in `ApplicationHandler::exiting`.
- **A scrolled layer is damage.** The software canvas moves a scroll
  layer's pixels and redraws only the revealed strip, but the presenter
  copies damage into older buffers and reports it to the compositor, so
  the moved rectangle must count as changed (`scrolled` in
  `render/software.rs`). A canvas-only test cannot catch this; look at
  a screenshot.
- **Children of a scrolling element don't shrink** (flexbox would
  squeeze them to the view, and nothing would scroll); `window.rs`
  sets `shrink = 0` on them.
- **cosmic-text's fallback maps every face it tries, for good.** For a
  character the first faces lack, it goes through its script and common
  lists, then through every face in the database, mapping each file and
  caching it (`FontSystem::get_font`). Shaping "▾" with only the UI
  family loaded made 72 Noto Sans files (16 MB) resident, so `text.rs`
  checks the UI face's character map first and merges the rest of the
  fonts before shaping. A character no font has still maps them all
  (open in the [status](../../docs/status/gui-toolkit-adr-0004.md)).
  To see it, count font files in `/proc/<pid>/maps` or sum `Rss` per
  file in `smaps`; `RssFile` in `status` is where it shows.
- `examples/gallery.rs` exercises everything. Drive it on the headless
  sway ([scale tests](../testing/wayland-scale-tests.md),
  [input](../testing/headless-sway-input.md)) with `wtype` and `grim`;
  `--exit-after-ms N` ends it, and it prints the first frame's time.
