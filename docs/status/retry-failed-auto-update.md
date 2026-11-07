---
name: retry-failed-auto-update
description: A failed automatic update stays failed until someone resumes it (job 17 stood failed for two days until resumed by hand, 2026-11-06); whether the daemon should retry it later on its own is for the maintainer
metadata:
  type: project
---

An update queued by `content.auto_update` (`content.md` §10) that fails
stays `FAILED` and resumable until someone resumes or cancels it
(`ipc.md` §5.6). In the CS2 delta-update run, the automatic update
failed on 2026-11-05 after a burst of HTTP 503 and stood failed for two
days as job 17 until it was resumed by hand on 2026-11-06
([m1b-content](m1b-content.md), `docs/acceptance/m1b.md`). Retries now
wait once every server has failed (`content.md` §7, 39fd332), so such a
burst no longer fails a job in seconds, but a job that does fail is
still not tried again.

For the maintainer: whether a failed automatic update should be retried
later on its own, and if so when (for example at the next change or
after a delay).
