---
name: ci-stays-off
description: Don't switch CI's Linux/Windows/release/fuzz jobs back on during development; anything else switched off gets a RELEASE-BLOCKER marker
metadata:
  type: feedback
---

Linux and Windows builds, release builds and artifacts, and nightly
fuzzing are switched off in CI on purpose. Don't switch them back on
during development. Anything else switched off for now gets a
`RELEASE-BLOCKER` marker where it is switched off and an entry in
`docs/release-checklist.md`. What CI does run: [ci](../testing/ci.md).
