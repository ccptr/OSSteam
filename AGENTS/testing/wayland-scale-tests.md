---
name: wayland-scale-tests
description: Test scale changes on Wayland in a headless sway, switching scale with swaymsg, without touching the desktop
metadata:
  type: reference
---

To test scale changes on Wayland without touching the desktop, run a
headless sway (`WLR_BACKENDS=headless sway -c <config>`) and switch
scale with `swaymsg output HEADLESS-1 scale N`.
