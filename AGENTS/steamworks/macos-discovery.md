---
name: macos-discovery
description: How macOS steam_api finds the client (Mach service com.valvesoftware.steam.ipctool) and what is still unverified
metadata:
  type: reference
---

macOS discovery (daca257, SPEC Q1, steamworks-abi.md §1.3): macOS
steam_api finds the client via the Mach service
`com.valvesoftware.steam.ipctool` (Version 100001 → 104; GetSteamPath 14
→ pid and path; replies of 32 and 544 bytes), not via files.
`discovery/macos.rs` checks the name in with `bootstrap_check_in` and
serves it on a thread. The library is copied to
`<data>/steam-sdk/osx/steamclient.dylib`; xtask runs lipo to make it
universal.

The CI run on efc15b5 showed that launchd accepts check-in of an
undeclared name (check-in succeeded and a look-up found it).

Unverified: that destroying the receive right frees the name. The tests
check it once the reply fix runs on CI
([waiting-push-macos-discovery-fix](../../docs/status/waiting-push-macos-discovery-fix.md)).
