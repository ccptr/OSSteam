---
name: ci
description: CI runs only cargo check/clippy/test on macOS, so test Linux natively and Windows by cross-check plus Wine; macOS can only be type-checked
metadata:
  type: reference
---

CI runs on macOS only, and only `cargo check`, `cargo clippy` and
`cargo test` ([ci-stays-off](../rules/ci-stays-off.md)).

So test locally: Linux natively, Windows by cross-check plus Wine
([windows-wine](windows-wine.md)). macOS-only code can be type-checked
with `--target aarch64-apple-darwin` but not built (ring's C does not
cross-compile without a macOS SDK); its tests first run on CI.
