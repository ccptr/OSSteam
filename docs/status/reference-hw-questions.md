---
name: reference-hw-questions
description: reference-hw.md §5 answered 2026-11-03 - the Steam Deck will be the reference; budgets are judged on the dev PC (interim) until it is set up; download runs bypass the VPN and count when they reach a speed test of the same line
metadata:
  type: project
---

reference-hw.md §5 (e604d60) was answered on 2026-11-03:

- Q1 and Q5: the Steam Deck will be the Linux reference, but it needs
  setting up. Until then budgets are judged on the dev PC: §20 rows are
  ticked there, marked interim, and run again on the Deck later.
- Q2: Windows is the dev PC's dual boot, closer to release.
- Q3: no Mac yet.
- Q4: wired Ethernet to a line of up to 970 Mbit/s, about 800 in
  practice.

Download runs (Q4): a run counts as saturating when it reaches what a
speed test on the same line reaches just before, and runs bypass the VPN
with `novpn` (§3). That is the maintainer's exception, for these runs,
to the rule of keeping authenticated traffic on the VPN.

**How to apply:** take §20 numbers on the dev PC as interim results,
under §3's conditions.
