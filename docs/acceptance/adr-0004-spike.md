# ADR 0004 spike: ossteam-ui

[ADR 0004](../adr/0004-own-gui-toolkit.md) step 2. The spike shows whether
our own toolkit meets §20 and GUI-4 with both renderers before the port
starts. The gpui spike it is compared with is in [m0.md](m0.md).

The harness is `crates/ossteam-ui/examples/spike.rs`: a header, a search
box with a shadow, and a virtual list of 10,000 rows with a scrollbar
and a status bar. It prints one JSON line: the time from the start of
`main` to the first presented frame and its parts, the scale factor and
appearance the first frame used, the renderer and why it was chosen,
the event-loop wakeups while idle and what caused each, and the resident
memory. `--bench scroll` scrolls the list by one row per frame to its end
and adds each frame's work (scene, draw and present, without waiting for
an earlier frame) and the intervals between frames.

```sh
cargo run --release -p ossteam-ui --example spike -- --renderer vulkan
cargo run --release -p ossteam-ui --example spike -- --renderer software --bench scroll
```

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
with no validation messages.

Items:

1. **Fonts, cold: open.** `cosmic-text` reads the header of every font
   file the system has (2,703 faces here) before the first text can be
   shaped. Warm that takes about 60 ms and overlaps opening the window;
   with a cold disk cache it took 967 ms, which alone breaks §20's
   500 ms. Loading only the faces the GUI asks for, and the fallback
   faces as text needs them, would fix it; to be designed with the
   toolkit (step 3).
2. **Idle wakeups: ours are none.** With `WAYLAND_DEBUG=1`, every wakeup
   after the first frame's configure and focus events is Hyprland's
   `xdg_wm_base.ping`, every 1.5 s, which the window must answer. The
   toolkit draws nothing and the app sees no event. To be checked on a
   headless sway, which does not ping.
3. **Software renderer, two missed frames: open.** Frame work stays
   under 1 ms except for one frame of 7.55 ms, longer than the 6.06 ms
   refresh interval; at 60 Hz it would not be missed. Not yet traced.

## Left

- The text field with IME, and the clipboard (as m0.md item 1).
- Scale factors 1.25, 1.5 and 2 on a headless sway, and whether the
  first frame is drawn at the output's scale.
- X11 (Xwayland), Windows under Wine (winevulkan and the software
  renderer), and lavapipe under headless sway (needs Mesa's
  `vulkan-swrast`, not installed here).
- A light desktop, checked by the maintainer (desktop settings are
  theirs to change).
- macOS and real Windows, with the hardware ([status](../status/hardware-mac-windows.md)).
