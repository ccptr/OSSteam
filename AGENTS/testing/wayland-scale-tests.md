---
name: wayland-scale-tests
description: Test scale changes on Wayland in a headless sway, switching scale with swaymsg (SWAYSOCK set), without touching the desktop
metadata:
  type: reference
---

To test scale changes on Wayland without touching the desktop, run a
headless sway (`WLR_BACKENDS=headless sway -c <config>`) and switch
scale with `swaymsg output HEADLESS-1 scale N`.

- **Set `SWAYSOCK`.** Outside sway, `swaymsg` cannot find the socket
  ("Unable to retrieve socket path"), and with `-q` it fails silently,
  so the runs happen at the old scale. Use
  `SWAYSOCK=/run/user/$(id -u)/sway-ipc.$(id -u).$(pgrep -x sway).sock`
  (the headless sway's pid, not the desktop's if that is sway too).
- winit's window opens at scale 1 and learns the output's scale after
  its first commit, so the first frame is at 1 on sway (gpui did the
  same); check the second frame and the `ScaleFactorChanged` event.
