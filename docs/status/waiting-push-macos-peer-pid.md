---
name: waiting-push-macos-peer-pid
description: macOS CI on origin/main (8cc08e7) fails one test, the Hello.pid check; the fix waits on a push
metadata:
  type: project
---

CI on origin/main (8cc08e7, run 41763529905, 2026-11-04) is red on
both macOS runners in one test,
`testing::m1c::games_talk_to_the_game_endpoint`: a `Hello` naming pid 1
was not refused. interprocess reads `LOCAL_PEERCRED` on macOS, whose
`xucred` has no pid, so the daemon never checked `Hello.pid` there,
although `steamworks-abi.md` §3.1 says `LOCAL_PEERPID`. The fix,
`ossteam_ipc::endpoint::peer_pid`, reads `LOCAL_PEERPID` itself.

The same run passed the macOS discovery tests that 7e96d11 fixed
([macos-discovery](../../AGENTS/steamworks/macos-discovery.md)).

**How to apply:** the fix is only type-checked for macOS here, so the
first CI run after the push is its test. Pushing needs the maintainer.
Once CI is green, delete this note.
