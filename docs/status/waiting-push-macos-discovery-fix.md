---
name: waiting-push-macos-discovery-fix
description: macOS CI on origin/main is red until 9487c6a (the Mach reply send-right fix) is pushed and runs
metadata:
  type: project
---

CI on origin/main (efc15b5) is red; the fix, 9487c6a, waits on a push.
On 2026-11-02 local main was ahead of origin with 9487c6a and the notes
commits, not pushed.

Both macOS runners failed `discovery::macos::tests::serves_steam_api`
and `a_service_someone_else_serves_is_left_alone`: no reply from the
Mach service. steam_api's `msgh_bits` 0x1413 makes a *send* right for
its reply port (0x14 = MAKE_SEND; checked in the SDK 1.65a dylib's
x86_64 and arm64 code), but steamworks-abi.md §1.3.1 called it
MAKE_SEND_ONCE and the server answered only send-once rights. 9487c6a
makes the server answer both and corrects the spec.

**How to apply:** the fix is only type-checked here, so the first CI run
after the push is its test. Pushing needs the maintainer. Once CI is
green, delete this note and update [macos-discovery](../../AGENTS/steamworks/macos-discovery.md).
