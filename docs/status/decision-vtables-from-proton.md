---
name: decision-vtables-from-proton
description: Unconfirmed decision - vtables come from Proton, in MSVC order, reversed per overload run off Windows
metadata:
  type: project
---

vtables come from Proton (facts only). `vtables.txt` is in MSVC order;
build.rs reverses overload runs (GetStat/GetStat_2…) on non-Windows
targets (00dca80). Unverified against a native game.
