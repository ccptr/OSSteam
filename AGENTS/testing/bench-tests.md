---
name: bench-tests
description: The ignored bench tests (10k-app library, gui_states with test logins bob/erin/dave/carol) and how to run and use them
metadata:
  type: reference
---

Run the workspace's ignored bench tests with `--bin ossteamd` (ossteamd
has no lib target):

- `gui_bench_library_of_10k_apps` (`ossteamd/src/testing/gui_bench.rs`)
  serves 10k fake-CM apps. Start the GUI with the `XDG_RUNTIME_DIR` it
  prints.
- `gui_bench_states` (`testing/gui_states.rs`) serves a small library in
  every state, plus accounts bob (email code BOB12), erin (authenticator
  ERIN5), dave (confirmation, auto-approved after 10 s) and a QR login
  that signs in as carol. Passwords are `<name>-password`.
  - `touch <root>/update-sleeper` publishes an update to the Sleeper app.
  - The bench game takes `-report` to run the shim checks.
  - Bench apps have no install directory, so Install fails there.
  - Bench jobs finish fast, so pause one with `ossteam jobs pause N`.

The bench daemon's RSS means nothing, because the fake CM runs in the
same process.
