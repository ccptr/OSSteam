# ADR 0002: Daemon integration tests run the daemon in-process against a fake CM

| | |
|---|---|
| Status | Accepted |
| Date | 2026-10-17 |
| Requirements | §19.1 level 3, FAITH-4, SEC-3 |
| Code | `crates/ossteamd/src/testing/`, `crates/ossteamd/src/session/mod.rs` (`Dialer`) |

## Context

§19.1 level 3 runs a real daemon against a test-only fake CM, driven
through `ossteam-ipc`. The obvious setup has two problems:

- The daemon reaches CMs over WSS, and SEC-3 allows only the platform
  verifier, with no way to turn checks off. A fake WSS server would need
  a certificate that the OS trusts. That means installing a test CA on
  every developer machine and CI runner, or adding a trust override to
  the daemon.
- A spawned `ossteamd` binary would need a runtime setting to send it to
  the fake. FAITH-4 forbids building test doubles into release binaries,
  and a setting that redirects the daemon away from Steam would be one.

## Decision

The integration tests run the daemon inside the test process, as unit
tests of the `ossteamd` binary crate (`#[cfg(test)] mod testing`).
`main` splits into setup (directories, logging, the lock, the listener)
and `serve`, which starts the IPC server and the workers and runs the
core. The tests call `serve` with two parts replaced:

- **The dialer.** The session worker gets its CM connections from a
  `Dialer`. The daemon passes `Servers`, which uses the Steam Directory
  and `Connection::connect` (WSS, platform verifier). The tests pass a
  dialer that opens a loopback TCP pair and starts the fake CM on the
  other end. `Connection::with_transport` runs the I/O thread on either
  transport.
- **Token storage.** The daemon opens the OS keyring. The tests use the
  existing file backend in their own directory, so a test never touches
  the developer's keyring.

Everything above the transport is the daemon's own code: framing, jobs,
service calls, auth, logon, reconnection, PICS, the core and IPC. The
fake CM checks what the daemon sends. It decrypts the password with its
own RSA key and compares codes and tokens, so a protocol mistake fails a
test instead of being answered regardless.

## Consequences

- No test CA, no trust override and no redirect setting exist. The
  release binary's only path to a CM is the Steam Directory over WSS with
  the platform verifier (SEC-3). The fake, its transport and its dialer
  are compiled only for tests (FAITH-4).
- TLS and WebSocket framing are not covered by these tests. They are
  covered by `ossteam-net`'s unit tests and by the live probe.
- The tests run in parallel. Each one gets its own directory, endpoint
  and fake, and none of them change the environment. They work on
  Windows too, because the endpoint is passed in rather than derived from
  the user's SID.
- The PICS poll interval became a parameter, so the change-polling test
  takes a fraction of a second. The daemon still passes 60 s.
- The fake encodes our reading of the spec pages. A test that passes
  shows the daemon matches `cm.md`, `auth.md` and `pics.md`. It does not
  show that those pages match Steam: that is still the job of the live
  acceptance runs and §19.3.
