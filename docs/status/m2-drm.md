---
name: m2-drm
description: M2 Steam DRM - drm.md drafted (13de005), not reviewed; SteamStub and CEG findings and its 4 open questions
metadata:
  type: project
---

SPEC §13: no DRM code lands before `docs/spec/drm.md` is reviewed
([spec-review-gates](spec-review-gates.md)). drm.md is drafted (13de005),
not reviewed.

- The SteamStub key is in the wrapper's own header (the client hands
  over no key). Flag bits match Valve's documented wrapping options
  0/6/32/38.
- Proton's lsteamclient has no wrapper code, which suggests the wrapper
  uses only public exports (unverified). Proton's steam.exe creates
  `Steam3Master_SharedMemLock` and `Global\Valve_SteamIPC_Class`.
- CEG: app info `config/cegpublickey` and `checkguid` (730 and 570 →
  client.dll; 41040 is free on demand). EMsgs
  5485/5486/5487/5574/851/896/897.
- Risk: verify could "repair" a CEG-customised file.
- §5 asks 4 questions: an observation build `steamstub-observe`, not
  decrypting the helper, test titles, CM capture.
