---
name: abi-test
description: The C++ ABI test (tests/abi.rs) - what it covers, and the struct-layout gotchas it found
metadata:
  type: reference
---

C++ ABI test (463253d, SPEC §19.1 level 4, steamworks-abi.md §6.1):
`ossteam-steamclient/tests/abi.rs` plus `tests/abi/game.cpp`. Passes
with GCC and Clang, x86_64 and i686.

Since 49b90f8 it also runs for Windows x86_64/i686 under Wine (game built
with `clang --target=*-pc-windows-msvc`, linked by `*-w64-mingw32-g++`;
`OSSTEAM_ABI_GAME`, see [windows-wine](../testing/windows-wine.md)).

Gotchas:
- CSteamID is pack(1) in the SDK.
- `#pragma pack(push, MACRO)` pushes a label and doesn't expand the
  macro.
- It found Windows UserStatsReceived_t and ValidateAuthTicketResponse_t
  with CSteamID at offset 16 instead of 12 (Proton's generated struct
  layouts agree on 12; fixed in 49b90f8).
- abi.rs had been loading a stale `target/<profile>/` copy of the
  library; it loads `deps/` now.
