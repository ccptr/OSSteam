---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes are proposed, not reviewed; nothing is built yet
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

Waiting on the maintainer's review. After it: the spike (ADR step 2),
then the toolkit and the port on a branch. Until then `ossteam-gui` is
on gpui.
