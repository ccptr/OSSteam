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
installing one; how to drive the spike with it, `wtype` and `xdotool` is
in [headless-sway-input](../../AGENTS/testing/headless-sway-input.md).

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
   so on Linux the toolkit should call the portal through ashpd with
   this connection (step 3; the ADR names `rfd`).
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
8. **No clipboard on Windows: waits on a licence decision.** `arboard`
   uses `clipboard-win` and `error-code` on Windows, both BSL-1.0, which
   is not in SPEC LIC-3's allow-list. The spike leaves `arboard` out of
   Windows builds, so the field there copies nothing and pastes nothing,
   and says so in the log. `cargo deny check licenses` still reports the
   two crates, because it joins dependency edges across its targets
   (`arboard` is needed on Linux and macOS); none of them is built.

## For the maintainer

**Which renderer `auto` prefers.** The software renderer meets §20
with less memory than Vulkan (28 MB against 55 MB idle at 1600×1000;
102 MB for lavapipe), so, as the ADR's step 2 says, which renderer
`auto` prefers is the maintainer's call.
What bears on it: both renderers draw scrolling and typing in 1 to 2 ms
a frame; the software renderer's weak spot is the first frames after a
resize at 4K (item 5); Vulkan costs 20 ms more at startup and twice the
memory, and lavapipe five times the software renderer's (item 6).

**The clipboard on Windows** (item 8). Either LIC-3 allows BSL-1.0
(the Boost licence, which the FSF lists as compatible with the GPL; a
SPEC change), and `arboard` is used on Windows as the ADR says; or the
toolkit writes its own Windows clipboard (`OpenClipboard` and
`CF_UNICODETEXT` through `windows-sys`, about 80 lines; an ADR change),
and `cargo-deny` still needs `arboard` gone from the graph or an
exception for the two crates.

**The folder picker** (item 4): through ashpd with the toolkit's own
bus connection rather than `rfd`, so it cannot start zbus's thread
pool (an ADR change).

## Left

- Scale factors 1.25, 1.5 and 2 on a headless sway, and whether the
  first frame is drawn at the output's scale.
- IME on X11 (needs an XIM engine that composes, such as ibus-anthy or
  fcitx5-mozc; the maintainer's to install).
- Windows under Wine (winevulkan and the software renderer).
- A light desktop, checked by the maintainer (desktop settings are
  theirs to change).
- macOS and real Windows, with the hardware ([status](../status/hardware-mac-windows.md)).
