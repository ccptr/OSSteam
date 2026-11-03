---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes are proposed, not reviewed; the spike is under way on a branch and asks which renderer auto prefers
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
with IME and the clipboard, and its measures on Hyprland and on a
headless sway (Wayland, Xwayland, lavapipe), all within §20 with no
idle wakeups. Waiting on the maintainer: which renderer `auto` prefers
(the ADR's step 2 hands it back, since the software renderer meets §20
with half the memory), whether LIC-3 allows BSL-1.0 so `arboard` can
serve the clipboard on Windows (spike item 8; until then Windows has
none), and whether the folder picker goes through ashpd rather than
`rfd` (spike item 4). Left: scale factors, IME on X11,
Windows under Wine, a light desktop.
