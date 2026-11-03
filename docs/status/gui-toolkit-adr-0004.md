---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes accepted 2026-11-03; spike done but for the maintainer's checks; the toolkit (step 3) has its elements, layout, focus, keys, lists, images and runtime on the branch; next the port
metadata:
  type: project
---

On 2026-11-02 the maintainer asked to replace gpui with an in-house GUI
toolkit, as a new crate on our own stack, renderer on `ash` (Vulkan),
not wgpu, plus our own software renderer (used when Vulkan is missing,
or forced with `--renderer`; the spike measures both, memory included). [ADR 0004](../adr/0004-own-gui-toolkit.md) was proposed with
these SPEC changes: GUI-1 rewritten, GUI-5 added (an idle window does
not wake), CONC-3 loses its GUI exception, WS-1 admits `ossteam-ui`,
LIC-4 names winit, and the §1 intro, §4, §5.1, §5.3, §16 title and §23
rows follow. `docs/spec/gui.md` §2.1, §3, §4.2, §6 and §7 now describe
the toolkit design.

Also on 2026-11-02 the maintainer chose Slang for the shaders; the ADR's
Shaders item says so.

The maintainer asked on 2026-11-02 to start the spike (ADR step 2)
before reviewing the ADR;
it is on the branch `ossteam-ui-spike`, with its results and what is
left in [adr-0004-spike](../acceptance/adr-0004-spike.md). The maintainer
chose to keep winit's client-side decorations (sctk-adwaita, which draws
with tiny-skia) for now. After the spike and the review: the toolkit and
the port on a branch. Until then `ossteam-gui` is on gpui.

By the end of 2026-11-02 the spike has both renderers, the text field
with IME and the clipboard, the folder picker, and its measures on
Hyprland and on a headless sway (Wayland, Xwayland, lavapipe), all
within §20 with no idle wakeups, except fonts read from a cold disk
cache (spike item 1). The maintainer then decided: `auto` prefers
Vulkan; `arboard` on every platform but Wayland, with BSL-1.0 added to
SPEC LIC-3; the folder picker through ashpd on Linux, `rfd` elsewhere.
The ADR says so.

On 2026-11-02 the maintainer also asked to replace gpui with
`ossteam-ui` once nothing blocks it. On 2026-11-03 they accepted the
ADR and its SPEC changes, and agreed to this order: the spike's scale
and Wine runs, then the toolkit (step 3) starting with loading only the
fonts the GUI uses (spike item 1, the cold first frame that breaks
§20's 500 ms), then the port (step 4) on a branch. Still the
maintainer's: a light desktop and a pick through the real portal
dialog.

The scale and Wine runs were done on 2026-11-03: sharp at 1.25, 1.5
and 2 with both renderers, the first frame at scale 1 as gpui's was;
under Wine both renderers draw and the clipboard works, but any winit
window spins there (Wine's bug), and a prefix without fonts made the
spike panic (spike items 10 and 11). Idle wakeups on Windows wait for
real Windows.

Step 3 began the same day with the fonts (`fonts.rs`): only the UI
family's files load before the first frame, the rest on the font
thread. The cold first frame went from about 2 s to 64 ms and the warm
one from 101 ms to 17 to 30 ms; no fonts at all is an error, not a
panic (spike items 1 and 10).

Also on 2026-11-03 the maintainer asked for a clean implementation that
does not port gpui code: the toolkit's API is its own (`panel()`,
`.row()`, `.pad()`, `.on_click(|state, cx| …)`, actions as plain types,
`"primary+f"` keystrokes), and the GUI's views will be written anew
against it. The ADR's Layout row and its Identity, Focus and Images
items were reworded to say so (an amendment to the accepted ADR). The
rest of step 3 landed then:

- `element.rs`, `style.rs`: one `Element<S>` type (panel, text, image,
  canvas, list, text input, overlay) with handlers that get `&mut S`;
  flexbox styles in logical pixels with inherited text properties,
  hover and keyboard-focus styles.
- `window.rs`: build, `taffy` layout in device pixels (whole pixels at
  any scale), paint into the scene, pointer, wheel, keys, IME, focus
  and Tab order, key contexts with actions bubbling up, scrolling the
  focused element into view, rows of a list built only in view,
  overlays kept inside the window, tooltips, timers and messages. No
  winit window in it, so `tests/window.rs` drives it headless (also
  under Wine).
- `app.rs`: winit's loop, `WaitUntil` for timers, a `Sender` for other
  threads, the appearance portal's change signal (`appearance::watch`).
- Images: a scene primitive drawn 1:1 by both renderers; the Vulkan
  renderer has the image pipeline (`shaders/image.slang`), a texture
  per image, freed after the application drops it.
- `examples/gallery.rs` shows all of it; on the headless sway it draws
  the same with both renderers, its first frame in 105 to 114 ms, no
  idle wakeups (one from Mesa's shader cache with Vulkan).

Bugs found and fixed on the way: the software renderer did not count
pixels it moved for a scroll as changed, so the compositor (and older
buffers) kept stale content; the runtime told winit a frame was coming
before a frame the software renderer then skipped, which on Wayland
stopped all later redraws; and the Wayland clipboard's worker crashed
when it outlived winit's connection at exit.

Resident memory in the gallery was 75 MB with Vulkan and 49 MB with
software (the spike: 55 and 28 MB). Of the 20 MB more, 16 MB was 72
Noto Sans files: the gallery's "▾" is not in Noto Sans, and shaping it
before the rest of the fonts had loaded sent cosmic-text through every
face it had, each mapped and kept. `text.rs` now merges the rest of the
fonts first when the UI face lacks a character; the gallery is at 58
and 31 MB, the rest being its larger heap and code and DejaVu Sans for
the "▾". (26 MB of the Vulkan figure is libLLVM, which Mesa's radeon
driver links; it was in the spike too.)

Open: a character whose fonts are not on cosmic-text's fallback lists
still makes it open faces until one has it, and one that no font has
opens them all: on this machine 2,637 files mapped, 489 MB resident
and 0.3 to 1.6 s on the UI thread for one call (Tangut, installed but
not on the lists: 220 files, 20 MB). Common scripts and emoji map one
font each. Steam names can carry such characters, so this wants our
own fallback (an index of which face covers which characters, built on
the font thread) before the friends list is ported; the maintainer to
decide.

The text field gained a secret mode for the login page's password
(bullets; Copy and Cut do nothing). Next: the port (step 4).
