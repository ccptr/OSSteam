---
name: m0-foundations
description: M0 status - acceptance items 1-2 closed (clipboard, startup flash), DPI done on Wayland
metadata:
  type: project
---

Open items 1–2 in `docs/acceptance/m0.md` are closed (1176c32).

- The clipboard failure was Wayland's rule that a selection needs a press
  serial. The spike now takes `wtype a` plus `OSSTEAM_SPIKE_CLIPBOARD`.
- The two-frame light flash at startup is fixed by
  `theme::StartupAppearance` (its own portal query on a thread, waiting
  at most 50 ms). It was found with a 165 Hz screen recording and
  ffmpeg's `signalstats` YAVG. Not checked on a light desktop.
- The DPI item is done on Wayland via a headless sway (0254d99). Images
  were requested once per (app, kind), so a rise in scale kept the 1x
  art; bench apps now carry `image2x`. Wayland's first frame is still
  drawn at scale 1.
