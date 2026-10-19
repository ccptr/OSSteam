# M1a acceptance: Session and library

SPEC §21 M1a. **Exit:** on Linux and Windows, a test account logs in
through every login path, and `ossteam library` lists exactly the apps its
licenses grant, with DLC grouped under the parent app. After a restart the
session resumes from the stored refresh token.

Each check's result must match the official client's on the same account
(§19.3). Fill in the account used (a test account, never a personal one),
the date and the build.

## Building blocks

| Item | Status |
|---|---|
| `ossteam-proto`: vendored protos at a pinned commit, `xtask update-protos` | ✅ 2026-10-17 |
| `ossteam-net`: directory, WSS transport, framing, Multi, jobs, service calls, heartbeats | ✅ unit tests and fuzz; live probe without an account 2026-10-17 |
| Reconnection with backoff (NET-4), in the daemon's session worker | built; unit tests for its parts; drop and reconnect against the fake CM 2026-10-17; live ⏳ (needs an account) |
| `ossteam-auth`: credential and QR sessions, guard codes, logon messages | unit tests; QR begin and poll live without an account 2026-10-17; login with an account ⏳ |
| Access-token renewal (AUTH-3) | ⏳ comes with the first WebAPI use |
| Daemon session subsystem, state database, keyring storage | built; integration tests without an account; live QR start and cancel through `ossteam login --qr` 2026-10-17 |
| Licenses, PICS, change polling, PICS cache | built 2026-10-17; unit tests; live run ⏳ (needs an account) |
| Persona state (PLAY-2) | built 2026-10-17; live run ⏳ (needs an account and a friend) |
| IPC: Daemon methods | ✅ M0 |
| IPC: Session methods | built 2026-10-17 |
| IPC: Library methods `ListApps`, `GetApp`; `AppChanged` | built 2026-10-17 |
| IPC: `GetAsset` and the image cache (`pics.md` §9) | built 2026-10-17; unit tests; live CDN fetch 2026-10-17; through a logged-in library ⏳ (needs an account) |
| CLI: `login`, `logout`, `account`, `status`, `events`, `daemon`, `completions` | built 2026-10-17 |
| CLI: `persona`, `library`, `info` | built 2026-10-17 |
| IPC: Settings methods `GetSettings`, `SetSetting`; `SettingChanged`; overrides in the state database | built 2026-10-17; unit and integration tests |
| CLI: `config` | built 2026-10-17; live run against an isolated daemon 2026-10-17 |
| Daemon integration tests against a fake CM (§19.1 level 3, ADR 0002) | built 2026-10-17: `ossteamd/src/testing/m1a.rs` covers every login path, wrong password and code, `--no-remember`, resume, revocation, reconnection, two accounts, the library with DLC, change polling and persona state |
| `cm.md` §7 and `auth.md` §8 captures of the official client | ⏳ needs a capture setup |

## Checklist

| Check | Linux x86_64 | Windows x86_64 |
|---|---|---|
| Credential login, no Steam Guard | ⏳ | ⏳ |
| Credential login, email code | ⏳ | ⏳ |
| Credential login, mobile authenticator code | ⏳ | ⏳ |
| Credential login, approval in the mobile app | ⏳ | ⏳ |
| QR login, including a rotated challenge | ⏳ | ⏳ |
| Wrong password and wrong code fail with exit code 4 | ⏳ | ⏳ |
| `--no-remember` leaves no stored token | ⏳ | ⏳ |
| Restart resumes from the stored token | ⏳ | ⏳ |
| Revoked token: daemon stops retrying and asks for login | ⏳ | ⏳ |
| Connection drop: `status` shows `connecting`, then reconnects | ⏳ | ⏳ |
| Two remembered accounts; `account switch` | ⏳ | ⏳ |
| `library` lists exactly the licensed apps, DLC under parents | ⏳ | ⏳ |
| PICS change polling picks up an app update | ⏳ | ⏳ |
| `persona online / invisible / offline` shows on a friend's client | ⏳ | ⏳ |
| §20: CLI round trip ≤ 50 ms p95, idle RSS ≤ 60 MB, idle wakeups ≤ 1/s | ⏳ | ⏳ |

Development machine, for comparison only (`reference-hw.md` §4; not
results): on 2026-10-19 the release `gui_bench_library_of_10k_apps`
daemon, logged on to the fake CM (heartbeat every 9 s, PICS polled every
60 s), idle for 5 minutes, woke 0.23 times a second across its own
threads (CM I/O 0.14, PICS 0.08), from voluntary context switches in
`/proc/<pid>/task/*/status`. Before ADR 0001's amendment its CM I/O
thread alone woke 1.03 times a second. 100 runs of `ossteam status`
(release) took 2.4 ms at p95. Its RSS (80 MB) is not the daemon's: the
fake CM and its 10,000-app fixture share the process. The real daemon,
logged out, sat at 11 MB with no wakeups in a minute.

The fake-CM tests check the daemon against our spec pages, not against
Steam. They do not tick any box here: each check still needs a live run
that matches the official client (§19.3).

✅ verified · ⏳ not yet run
