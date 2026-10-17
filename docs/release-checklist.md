# Release checklist

Things switched off during development that must be restored before any
release. Each is marked `RELEASE-BLOCKER` at the place it was switched off;
`grep -rn RELEASE-BLOCKER .github docs crates` lists them.

- [ ] **CI builds for Linux and Windows** (`.github/workflows/ci.yml`,
  build matrix): `linux-x86_64`, `linux-aarch64` and `windows-x86_64` are
  commented out. During development Linux is built and tested locally and
  Windows is only cross-checked (`cargo check --target
  x86_64-pc-windows-gnu`), so PLAT-1 is not verified for them until
  these return.
- [ ] **Nightly fuzzing** (`.github/workflows/fuzz.yml`): the schedule is
  commented out because it builds on Linux. SEC-5 requires it.
