# ADR 0004 spike: ossteam-ui

[ADR 0004](../adr/0004-own-gui-toolkit.md) step 2. The spike shows whether
our own toolkit meets §20 and GUI-4 with both renderers before the port
starts. The gpui spike it is compared with is in [m0.md](m0.md).

The harness is `crates/ossteam-ui/examples/spike.rs`: a header, a search
field with a shadow that filters the list as you type, a virtual list of
10,000 rows with a scrollbar, and a status bar. It prints one JSON line:
the time from the start of `main` to the first presented frame and its
parts, the scale factor and appearance the first frame used, the
renderer and why it was chosen, the event-loop wakeups while idle and
what caused each, each thread's context switches while idle, the
resident memory, and what the field holds, the IME events it got and
the time from each input to the frame that shows it. `--bench scroll`
scrolls the list by one row per frame to its end; `--bench type` types
"hollow kingdom" into the field and deletes it again, one edit per
frame. Both add each frame's work (scene, filtering, draw and present,
without waiting for an earlier frame or for the display) and the
intervals between frames.

```sh
cargo run --release -p ossteam-ui --example spike -- --renderer vulkan
cargo run --release -p ossteam-ui --example spike -- --renderer software --bench scroll
cargo run --release -p ossteam-ui --example spike -- --renderer software --bench type
```

`crates/ossteam-ui/examples/wayland_ime.rs` is a scripted input method
(`zwp_input_method_v2`, which sway has) for testing IME without
installing one, and `examples/fake_portal.py` a stand-in portal file
chooser for testing the folder picker (Ctrl+O in the spike) without a
dialog; how to drive the spike with them, `wtype` and `xdotool` is in
[headless-sway-input](../../AGENTS/testing/headless-sway-input.md).

## Linux x86_64: Arch Linux, Hyprland (Wayland), 1× scale, 2026-11-02

AMD Radeon RX 7900 XTX (RADV, Mesa 26.2.5), display at 165 Hz, window
1265×684 (tiled). Release build, rustc 1.98.1.

| Measure | Vulkan | Software |
|---|---|---|
| First frame, warm (3 runs) | 100.5 / 100.8 / 101 ms (one 1,009 ms run, cold fonts: item 1) | 101.0 / 102.1 / 102.3 ms |
| Renderer created at | 23 to 26 ms | 5 to 6 ms |
| Fonts loaded at (2,703 faces) | ~61 ms warm, 967 ms cold | ~58 ms |
| Appearance at the first frame | `Dark`, from the portal, as the desktop | same |
| Scale factor at window creation and first frame | 1 and 1 (the output is at 1) | same |
| Wakeups while idle | 2 in 3 s, both the compositor's `xdg_wm_base.ping` (item 2) | same |
| Resident memory, idle | 51.9 MB | 23.3 MB |
| Scroll 9,981 rows, one per frame: frame work p50 / p99 / max | 0.29 / 0.44 / 1.59 ms | 0.63 / 0.88 / 7.55 ms |
| Frames with work over 16 ms | 0 | 0 |
| Missed frames (interval over 1.5 refreshes) | 0 of 9,982 | 2 of 9,982 (item 3) |
| Resident memory while scrolling, peak | 53.6 MB | 25.3 MB |
| Pixels redrawn per frame, mean | the whole window | 37,945 (the revealed strip, the scrollbar, the status bar) |

A debug build with `VK_LAYER_KHRONOS_validation` loaded scrolled 300 rows
with no validation messages. These runs predate the text field and the
fix in item 4.

## Linux x86_64: headless sway 1.12 (Wayland and Xwayland), 1× scale, 2026-11-02

The same machine, a headless sway (`WLR_BACKENDS=headless`) with one
1600×1000 output; sway paces frames at 60 Hz and reports no refresh
rate, so missed frames are read from the intervals. The window opens at
900×640 and sway tiles it to 1600×1000 one frame later. lavapipe is Mesa
26.2.5's (llvmpipe, LLVM 23.1.3), chosen with `VK_DRIVER_FILES`; X11 is
sway's Xwayland. 3 runs each for startup and idle.

| Measure | RADV, Wayland | lavapipe, Wayland | Software, Wayland | RADV, X11 | Software, X11 |
|---|---|---|---|---|---|
| First frame, warm | 100.4 / 100.9 / 102.8 ms | 110.7 / 111.1 ms (151.3 ms on the first run) | 101.5 / 101.5 / 107.6 ms | 102.5 / 104.8 ms (164.3 ms on the first run) | 103.1 / 106.9 / 108.1 ms |
| Renderer created at | 22 to 23 ms | 33 to 34 ms (84 ms on the first run) | 2 to 3 ms | 35 to 37 ms (114 ms on the first run) | 17 ms |
| Appearance at the first frame | `Dark`, from the portal (known at 2 ms) | same | same | same | same |
| Wakeups while idle, any thread, from 1.5 s to 4.5 s | 0 | 0 | 0 | 0 | 0 |
| Resident memory, idle, 1600×1000 | 54.8 MB | 102.3 MB | 27.9 MB | 57.4 MB | 28.3 MB |
| Scroll 9,970 rows: work p50 / p99 / max | 0.38 / 0.68 / 7.47 ms | 0.35 / 0.53 / 1.73 ms (item 6) | 1.06 / 1.51 / 11.92 ms | 0.53 / 0.80 / 3.65 ms | 0.86 / 1.49 / 11.47 ms |
| Scroll: longest interval between frames | 17.8 ms | 25.3 ms (one frame) | 19.0 ms | 19.4 ms | 11.9 ms (not paced, item 7) |
| Scroll: resident memory, peak | 61.5 MB | 118.7 MB | 38.9 MB | 64.7 MB | 35.3 MB |
| Type 28 edits: work p50 / max | 0.66 / 9.68 ms | 0.68 / 3.88 ms | 0.90 / 13.77 ms | 0.67 / 5.04 ms | 0.80 / 15.66 ms |
| Type: longest interval between frames | 16.8 ms | 17.8 ms | 17.2 ms | 16.3 ms | 16.1 ms |

The frames over 5 ms in these benches come right after sway resizes the
window, not from scrolling or typing (item 5): with the window floating,
so never resized, the worst frame in the software
renderer's scroll bench is 2.4 ms and in its type bench 4.9 ms (the
first keystroke: building the 10,000 lower-cased names for the filter
takes 1.9 ms of it, then 0.3 ms a keystroke), and Vulkan's 1.5 and
2.9 ms. The software rows were measured before it drew on 8 threads
(item 5), which changes only frames that redraw much of the window.

### The text field, IME and the clipboard

Driven on the headless sway with real key events (`wtype` on Wayland,
`xdotool` on X11), the IME test client, `wl-copy`/`wl-paste` and
`xclip`; each with both renderers.

- **Typing and editing: works on Wayland and X11.** Characters, Backspace
  and Delete by grapheme (a flag, `e` with a combining accent),
  Ctrl+Left/Right and Ctrl+Backspace by word, Home/End, Shift to
  select, Ctrl+A, Escape to clear; the list filters on every edit. The
  editing model's unit tests cover the same (`text_field.rs`).
- **Clipboard: works on Wayland and X11, both directions.** Ctrl+C put
  "hollow" where `wl-paste` and `xclip` read it (X11 copies reach
  Wayland through sway), Ctrl+X cut "kingdom ", and Ctrl+V pasted text
  `wl-copy` or `xclip` had set. Opening the clipboard after the first
  frame takes 0.03 to 0.3 ms.
- **IME: works on Wayland.** The field enables `zwp_text_input_v3`
  through winit; the test input method composed "にほ" with the cursor
  after に, converted it to "日本" (the clause marked), cleared the
  composition and committed "日本", then composed "ご" with the caret
  hidden and committed "語": the field ends with "game 日本語". The
  composition is drawn at the cursor with an underline, the clause being
  converted with a double-thick one, and the caret where the IME puts
  it; both renderers draw it alike (screenshots differ by at most one
  level in a channel). The caret rectangle winit gives the compositor
  for the candidate window came back to the input method as 2×19 px, the
  caret's size, after every change. Not tested on X11: no XIM engine
  that composes is installed here (ibus has only its keyboard layouts).
- **Input to presented frame:** p50 0.8 to 3.3 ms on Wayland and X11,
  both renderers. The first composition with Japanese text took 34 ms,
  once: loading the CJK fallback font on first use.

### The folder picker

On Linux the toolkit calls the portal's file chooser through ashpd on
its own bus connection (item 4); Ctrl+O in the spike opens it on a
thread and the answer comes back to the event loop. On the headless
sway, each run on a private session bus (`dbus-run-session`):

- **With xdg-desktop-portal and its GTK back end:** the dialog opens on
  the sway with the spike's title ("Add library folder") in folder mode,
  and the spike keeps drawing beside it: sway tiles the two, which
  resizes the spike's window (item 9). GTK took typed text from `wtype`
  but ignored its Return and Escape, so these runs never answered; a
  pick through the real dialog is still to be checked by hand.
- **With `fake_portal.py` in the portal's place**, both renderers: a
  folder named with a space and a non-ASCII letter came back as that
  path, and a cancel as none, each 0.3 s after the call (the stand-in's
  delay). After the answer the event loop woke once with no event, at
  0.3 s, then not at all for 4.7 s; the picker's thread had ended and
  the `async-io` thread switched twice.

### Scale factors, 2026-11-03

The same headless sway, its output set to 1.25, 1.5 and 2 with
`swaymsg output HEADLESS-1 scale N` before each run, then changed while
the window was open (1 → 2 → 1.5 → 1.25 → 1). Both renderers, Wayland.

| Measure | Vulkan | Software |
|---|---|---|
| First frame, at 1.25 / 1.5 / 2 | 102.2 / 100.3 / 97.7 ms | 99.3 / 98.2 / 100.0 ms |
| Scale of the first frame | 1, at all three | same |
| Scale of the second frame | the output's, 0.4 to 0.6 ms after the first | the output's, 1.3 to 1.6 ms after the first |
| Live changes | each redrawn at the new scale in one frame | same |
| Resident memory, idle, 1600×1000 | 54.8 to 55.1 MB | 28.2 MB |

- **The first frame is drawn at scale 1, as gpui's was (m0.md item 3).**
  winit opens the window at 1, and sway sends the fractional scale only
  after the surface's first commit, so the first frame cannot know it.
  The second frame, at the output's scale, comes about a millisecond
  later. The first is also drawn at the 900×640 the window asked for;
  sway tiles it to 1600×1000 with the scale. Whether the first frame
  shows is not seen.
- **Sharp and laid out as at 1×**, at each scale: text, the field's
  border and caret, the rows and the scrollbar keep their logical sizes
  and are drawn at device resolution. The two renderers' screenshots
  differ in under 0.1 % of pixels beyond 3 % per channel (glyph edges).
  At 1.5 sway gives the window 1599×999 device pixels (its 1066×666
  logical size rounded down); the spike fills them.
- After the live changes the window is the same as one opened at that
  scale (screenshots at 2 identical), and the event loop was idle
  between changes.

### Windows under Wine, 2026-11-03

`x86_64-pc-windows-gnu` release build, Wine 11.21, on the headless
sway's Xwayland (`DISPLAY=:2`), in a scratch prefix with Wine's Tahoma
and Noto Sans copied into `C:\windows\Fonts` (item 10). winevulkan on
the host's RADV.

| Measure | Vulkan | Software |
|---|---|---|
| First frame (3 runs) | 64.1 / 64.6 / 74.1 ms | 59.0 / 60.6 / 62.4 ms |
| Renderer created at | 53 to 55 ms | 31 to 36 ms |
| Appearance at the first frame | `Light`, from winit: Wine reports light | same |
| Scroll 9,970 rows: work p50 / p99 / max | 0.44 / 0.64 / 1.15 ms | 1.19 / 1.79 / 2.54 ms |
| Scroll: frame intervals | 16.1 ms p50, one of 27.2 ms | 2.2 ms p50: not paced, as on X11 (item 7) |
| Type 28 edits: work p50 / max | 0.72 / 5.21 ms | 0.84 / 7.04 ms |
| Clipboard (`arboard`) | Ctrl+V pasted text `xclip` had set; Ctrl+X put the cut text where `xclip` read it | same |
| Wakeups while idle | about 30,000 a second (item 11) | same |

Resident memory and per-thread switches are not reported under Wine
(the working set it reports is 17.4 MB for both renderers, which is not
the Linux process's). Both renderers draw the window alike: screenshots
differ in 0.02 % of pixels beyond 3 % per channel. Ctrl+C could not be
checked: Wine read `xdotool`'s keys through a different keymap (`c`
arrived as `AudioVolumeDown`).

Items:

1. **Fonts, cold: open.** `cosmic-text` reads the header of every font
   file the system has (2,703 faces here) before the first text can be
   shaped. Warm that takes about 60 ms and overlaps opening the window;
   with a cold disk cache it took 967 ms, which alone breaks §20's
   500 ms. Loading only the faces the GUI asks for, and the fallback
   faces as text needs them, would fix it; to be designed with the
   toolkit (step 3).
2. **Idle wakeups: none.** With `WAYLAND_DEBUG=1`, every wakeup on
   Hyprland after the first frame's configure and focus events is the
   compositor's `xdg_wm_base.ping`, every 1.5 s, which the window must
   answer. On the headless sway, which does not ping, the event loop
   does not wake between the frame after the resize and the deadline,
   and no thread switches at all from 1.5 s to 4.5 s, on Wayland and
   X11, before and after a copy, with every renderer (lavapipe's 12
   threads included).
3. **Software renderer, two missed frames on Hyprland: not reproduced.**
   Frame work stayed under 1 ms except for one frame of 7.55 ms, longer
   than the 6.06 ms refresh interval. A rerun timing each step showed
   the first two frames at 3.2 and 3.6 ms, copying the whole frame into
   each of softbuffer's two new buffers (item 5), and every later frame
   under 1 ms.
4. **Portal query woke the process every 500 ms: fixed.** zbus connects
   to the session bus on the `blocking` crate's thread pool, and the
   pool's last thread never exits: it woke every 500 ms for the life of
   the process, against GUI-5. The appearance query now connects the
   bus socket itself (`DBUS_SESSION_BUS_ADDRESS`, `unix:path=` or
   `unix:abstract=`) and gives it to zbus, so the pool never starts.
   The same applies to the folder picker: `rfd`'s portal back end lets
   zbus connect, while ashpd's file chooser requests take a connection,
   so on Linux the toolkit calls the portal through ashpd with this
   connection (`portal.rs`); the maintainer agreed and the ADR says so.
5. **Resizing with the software renderer: open.** After a resize the
   window gets new shared-memory buffers from softbuffer, and the first
   write to each costs page faults: copying a full 1600×1000 frame into
   one took about 5 ms, a 3840×2160 one 24 ms. With the full redraw
   (15 ms at 3840×2160 on 8 threads), the first frame after a resize
   at 4K took 45 to 50 ms and the next 27 ms; later frames 1 to 2 ms.
   Vulkan's frame after a resize at 4K took 17 ms (the swapchain is
   rebuilt), later frames 1 to 2 ms. So an interactive resize at 4K
   draws at about 20 frames a second with the software renderer. The
   software renderer now draws on up to 8 threads, each taking every
   n-th band (it was 4 threads in blocks: 50 ms then 45 ms).
6. **lavapipe: meets §20, the most memory.** Its frame work is low
   because llvmpipe draws on its own threads while the renderer waits,
   which counts as waiting; one frame interval of 25.3 ms in the scroll
   bench is the only frame it missed. It uses 102 MB idle, against
   28 MB for the software renderer at the same size, and loads 33 ms
   into startup. Today `auto` takes lavapipe when it is the only Vulkan
   device.
7. **Software renderer on X11 is not paced.** softbuffer's X11 present
   returns at once, so the scroll bench drew frames 1 to 2 ms apart
   instead of one per refresh. Harmless when frames come from input, as
   in the GUI, but a scroll animation would draw more frames than the
   display shows.
8. **Clipboard on Windows: resolved.** `arboard` uses `clipboard-win`
   and `error-code` on Windows, both BSL-1.0, which SPEC LIC-3 did not
   allow. The maintainer added BSL-1.0 to LIC-3 on 2026-11-02, so
   `arboard` serves the clipboard on Windows too, as the ADR says.
   Under Wine it pastes and cuts (the Wine section).
9. **Software renderer crashed when the window shrank: fixed.** The
   canvas keeps the damage of recent frames to bring an older buffer up
   to date, and it kept that history across a resize: when the picker's
   dialog tiled the window to half its width, rectangles from the wider
   frames were copied into the narrower buffer, out of bounds. The
   history is now dropped on a resize, so a buffer older than it is
   copied whole; a unit test covers it.
10. **No fonts: the spike panicked; open.** A fresh Wine prefix has no
    files in `C:\windows\Fonts` (Wine's own fonts live outside it), so
    `fontdb` found no faces and cosmic-text panicked shaping the first
    line ("no default font found"). Windows always has Segoe UI, but
    the toolkit must not panic: with no usable face it reports that
    and exits, and it looks for the platform's UI font by name (Segoe
    UI, the system font on macOS, fontconfig's sans-serif on Linux)
    instead of relying on cosmic-text's fallback lists. Part of the
    font work in step 3 (item 1).
11. **Idle under Wine: Wine and winit spin; not ours.** Under Wine 11.21
    the event loop woke about 30,000 times a second with no events.
    `WINEDEBUG=+server` shows Wine's queue keeping `QS_PAINT` in its
    wake bits while `get_message` finds nothing, so winit's
    `MsgWaitForMultipleObjectsEx(QS_ALLINPUT)` returns at once. A bare
    winit 0.30.13 window (no ossteam code, never drawn) does the same.
    Idle wakeups on Windows wait for real Windows.

## Decisions

The maintainer decided on 2026-11-02:

- **`auto` prefers Vulkan**, with the software renderer when no Vulkan
  device can be created. lavapipe still counts as a Vulkan device.
- **`arboard` on every platform but Wayland**, with BSL-1.0 added to
  SPEC LIC-3 (item 8).
- **The folder picker through ashpd on Linux** (item 4), `rfd` on
  Windows and macOS.

## Left

- A pick through the portal's real dialog, by hand.
- IME on X11 (needs an XIM engine that composes, such as ibus-anthy or
  fcitx5-mozc; the maintainer's to install).
- A light desktop, checked by the maintainer (desktop settings are
  theirs to change).
- macOS and real Windows, with the hardware ([status](../status/hardware-mac-windows.md)):
  idle wakeups, memory, Ctrl+C and the software renderer's pacing on
  Windows are not visible under Wine.
