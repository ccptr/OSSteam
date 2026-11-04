---
name: m2-vac
description: M2 VAC - vac.md drafted (3101149), not reviewed; how modules and calls flow, the host, and its 5 open questions
metadata:
  type: project
---

SPEC §14: no VAC code lands before `docs/spec/vac.md` is reviewed
([spec-review-gates](spec-review-gates.md)). vac.md is drafted (3101149),
not reviewed.

- Modules arrive as ClientServiceModule 830, calls as
  ClientServiceCall 831 → ClientServiceCallResponse 832 (the host fills
  in OS/system info, load address and exception record).
  FileVerification service (SignatureCheck, SteamServiceCheck).
- ClientVACBanStatus 782's layout is unclear (SteamKit2 and
  VacBanBlock_t disagree). Without VAC the server sees auth response 5,
  VACCheckTimedOut.
- The official Linux host is the 32-bit `ubuntu12_32/steamservice.so`,
  loaded into the `steam` process (dlopen, `runfunc`, VacProcessMonitor
  reading /proc); the Windows host (SteamService.exe) is known only from
  community guides.
- Plan: a separate `ossteam-vac-host`, the daemon relays unchanged, a
  `vac` build feature.
- §7 asks 5 questions: reading steamservice.so's code, logging unhandled
  EMsgs, calls while VAC is off, ban risk to the test account,
  Linux-first scope.
