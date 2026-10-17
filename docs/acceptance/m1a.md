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
| `ossteam-net`: reconnection with backoff (NET-4) | ⏳ |
| `ossteam-auth`: credential and QR sessions, guard codes, token renewal, logon messages | unit tests; QR begin and poll live without an account 2026-10-17; login with an account ⏳ |
| Daemon session subsystem, state database, keyring storage | ⏳ |
| Licenses, PICS, change polling, PICS cache | ⏳ |
| Persona state (PLAY-2) | ⏳ |
| IPC: Daemon, Session and Library methods | ⏳ |
| CLI: `login`, `logout`, `account`, `status`, `persona`, `library`, `info`, `config`, `events`, `daemon`, `completions` | ⏳ |
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

✅ verified · ⏳ not yet run
