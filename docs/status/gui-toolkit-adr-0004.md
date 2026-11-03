---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes accepted 2026-11-03; the spike is nearly done on a branch, and the toolkit (step 3) has started there
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
panic (spike items 1 and 10). Next in step 3: elements, layout, focus,
lists and images, up to what the GUI uses.
