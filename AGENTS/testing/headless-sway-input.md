---
name: headless-sway-input
description: Driving keys, IME and the clipboard on a headless sway - wtype loses the first key, create the keyboard before the window, Xwayland needs setxkbmap and xdotool
metadata:
  type: reference
---

A headless sway (`WLR_LIBINPUT_NO_DEVICES=1`) has no keyboard until a
tool makes a virtual one, which changes how input tests behave
([headless sway](wayland-scale-tests.md)):

- **`wtype` loses its first key** unless it pauses first: the window
  gets keyboard focus only when wtype's keyboard appears, with the key
  already down, so the press arrives inside `wl_keyboard.enter`. Use
  `wtype -s 50 …`.
- **IME needs the keyboard before the window.** winit creates its
  `zwp_text_input_v3` when a keyboard appears, and sway sends it
  `enter` only on a later focus change. Start a long-lived
  `wtype -s 8000 -M shift -m shift &` first, then the window, then the
  input method: `cargo run -p ossteam-ui --example wayland_ime -- wait
  preedit:にほ:3:6 sleep:500 preedit: commit:日本` (steps in its docs).
  Only one input method can bind a seat: never run it on the desktop.
- **Xwayland keeps the first virtual keyboard's keymap**, so later
  `wtype` keys arrive as the wrong keys. Run `setxkbmap us` on sway's
  Xwayland display (`ls /tmp/.X11-unix`; it starts with the first X
  client) and type with `xdotool`. Even then its first key is sometimes
  never delivered (about 1 run in 6); rerun before suspecting the app.
- Clipboard checks: `wl-paste -n` / `wl-copy --paste-once` with
  `WAYLAND_DISPLAY=wayland-2`, `xclip -selection clipboard` with
  `DISPLAY` set; sway passes X11 copies on to Wayland.
- `VK_DRIVER_FILES=` set but empty hides every Vulkan driver; set it
  only to choose one (`/usr/share/vulkan/icd.d/lvp_icd.json` for lavapipe).
