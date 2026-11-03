---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes are proposed, not reviewed; the spike is nearly done on a branch, the maintainer's three decisions are in, and the port waits on what is left
metadata:
  type: project
---

On 2026-11-02 the maintainer asked to replace gpui with an in-house GUI
toolkit, as a new crate on our own stack, renderer on `ash` (Vulkan),
not wgpu, plus our own software renderer (used when Vulkan is missing,
or forced with `--renderer`; the spike measures both, memory included). [ADR 0004](../adr/0004-own-gui-toolkit.md) is proposed, with
these SPEC changes: GUI-1 rewritten, GUI-5 added (an idle window does
not wake), CONC-3 loses its GUI exception, WS-1 admits `ossteam-ui`,
LIC-4 names winit, and the §1 intro, §4, §5.1, §5.3, §16 title and §23
rows follow. `docs/spec/gui.md` §2.1, §3, §4.2, §6 and §7 now describe
the toolkit design.

Also on 2026-11-02 the maintainer chose Slang for the shaders; the ADR's
Shaders item says so.

The ADR and its SPEC changes still wait on the maintainer's review. The
maintainer asked on 2026-11-02 to start the spike (ADR step 2) meanwhile;
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
`ossteam-ui` once nothing blocks it. Still in the way: the review of
the ADR and its SPEC changes (it is still Proposed), the cold-font
first frame (spike item 1, which breaks §20's 500 ms), and the spike's
Left list (scale factors, Windows under Wine, a light desktop, a pick
through the real portal dialog). The port is ADR steps 3 and 4: the
toolkit itself (elements, layout, focus, lists, images), then the GUI
on it.
