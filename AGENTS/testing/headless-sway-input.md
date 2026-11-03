---
name: headless-sway-input
description: Driving keys, IME, the clipboard and the folder picker on a headless sway - wtype loses the first key, keyboard before the window, Xwayland needs setxkbmap, portals on a private bus
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
- **IME on X11** without a composing engine installed: ibus's own
  Ctrl-Shift-U entry composes through XIM. On a private bus
  (`dbus-run-session`) with `DISPLAY` set to sway's Xwayland and
  `GSETTINGS_BACKEND=memory`, start `ibus-daemon --xim
  --panel=disable`, `ibus engine xkb:us::eng`, then the app with
  `XMODIFIERS=@im=ibus` and no `WAYLAND_DISPLAY`; `xdotool key
  ctrl+shift+u`, `type e9`, `key space` composes "ue9" and commits é.
  ibus leaves `~/.config/ibus/bus/*-unix-<display>` behind; remove it.
- Clipboard checks: `wl-paste -n` / `wl-copy --paste-once` with
  `WAYLAND_DISPLAY=wayland-2`, `xclip -selection clipboard` with
  `DISPLAY` set; sway passes X11 copies on to Wayland.
- `VK_DRIVER_FILES=` set but empty hides every Vulkan driver; set it
  only to choose one (`/usr/share/vulkan/icd.d/lvp_icd.json` for lavapipe).
- Screenshots: `grim -o HEADLESS-1`; without `-o` grim may capture a
  black frame.
- **Portals (the folder picker) on a private bus.** Set the environment
  on `dbus-run-session` itself, not in the script it runs:
  `env -u DISPLAY WAYLAND_DISPLAY=wayland-2 XDG_CURRENT_DESKTOP=sway
  dbus-run-session -- script.sh`. The bus starts portals with its own
  environment, so otherwise they open on the desktop. With the real
  xdg-desktop-portal the GTK dialog opens on the sway, but it ignores
  `wtype`'s Return and Escape. To test the call itself, start
  `python3 -I crates/ossteam-ui/examples/fake_portal.py PATH cancel &`
  on the private bus before the app: it owns the portal's name and
  answers each request in turn. It serves Settings too: `scheme=N` sets
  `color-scheme` (default 1, dark), `change=N@MS` changes it with its
  signal MS ms after it starts, and `settings-v1` drops `ReadOne`.
