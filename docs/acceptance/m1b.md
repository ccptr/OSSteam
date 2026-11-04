# M1b acceptance: Content

SPEC §21 M1b. **Exit:** on Linux and Windows, a title with DLC depots
installs, updates through a delta, survives a daemon kill in the middle of
a download and resumes, verifies and repairs a corrupted file, and
uninstalls cleanly. The download-throughput budget is met.

Each check's result must match the official client's on the same account
(§19.3): the same files, byte for byte, in the same layout. Fill in the
account used (a test account, never a personal one), the title, the date
and the build.

## Building blocks

| Item | Status |
|---|---|
| `content.md` and ADR 0003 (HTTPS only) | written 2026-10-17 |
| `ossteam-content`: server list, rotation, manifests, chunks, depot keys, throttle | built 2026-10-17; unit tests |
| Depot selection (CONT-2, `content.md` §6) | built 2026-10-17; unit tests |
| Downloader: retries, rotation, CDN auth tokens | built 2026-10-17; tests against the fake CDN |
| Install engine: install, delta update, verify, journal and resume, removed depots, links | built 2026-10-17; tests against the fake CDN |
| Library folders (`ipc.md` §5.5) | built 2026-10-17 |
| Jobs and their scheduling in the daemon (`ipc.md` §5.6) | built 2026-10-17 |
| Update detection and `content.auto_update` | built 2026-10-17 |
| Settings `content.bandwidth_limit`, `concurrent_jobs`, `download_threads`, `language` | built 2026-10-17 |
| CLI: `install`, `update`, `verify`, `uninstall`, `jobs`, `folders`; install state in `info` | built 2026-10-17 |
| Daemon integration tests against the fake CM and CDN | built 2026-10-17: `ossteamd/src/testing/m1b.rs` covers install with DLC, delta update, verify and repair, uninstall, resume after a stop, auto-update, a refused DLC key, folders and job controls |
| Install scripts (CONT-8): `install-scripts.md`; parsing, variables and due checks in `ossteam-content` | written 2026-10-17; unit tests |
| Install scripts: recorded with the install; on Windows, the registry, processes and the elevated helper; removal at verify, uninstall entries and removal at uninstall | built 2026-10-17; unit tests and a daemon test against a fake registry (`testing/m1b.rs`); Windows code type-checked only, never run |
| Install scripts before the first start: registry writes (§2.6), firewall rules, Run Process | ⏳ M1c, with launching |
| Depots shared from an app outside the library (`depotfromapp`, for example 228980): its info is fetched with the library's (`pics.md` §5) | built 2026-10-17; daemon test with `sharedinstall` and an update in the source app |
| Captures of the official client's content requests (`content.md` §13) | ⏳ needs a capture setup |

## Checklist

| Check | Linux x86_64 | Windows x86_64 |
|---|---|---|
| Install a title with DLC depots; files match the official client's | ⏳ | ⏳ |
| `info` shows the install: path, size, branch, build, depots | ✅ 2026-11-04, Half-Life 2: Demo (219) and Counter-Strike 2 (730); `build` only for an app with a `branches` section (219's depots come from app 220); the text marked no depot as installed until then (now `installed`), `--json` gives each installed depot's manifest | ⏳ |
| A new build is detected and installed as a delta (only changed chunks fetched) | ⏳ | ⏳ |
| Kill the daemon (SIGKILL / TerminateProcess) mid-download; the job resumes from its journal after a restart | ✅ 2026-11-04, two installs running (219 at 338 MiB, 18010 at 83 MiB): both went on from there. 18010's download total shrank by 1 MiB (237.1 to 236.1 MiB), the journal's lag (`content.md` §9) | ⏳ |
| Corrupt an installed file; `verify` repairs it and only it | ✅ 2026-11-04, 219: 16 bytes overwritten in a 127 MiB `.vpk`; verify repaired 1 file, fetched one chunk (0.9 MiB), the SHA-256 back to the original, no other file's time or size changed | ⏳ |
| `uninstall` removes the files, the manifests and the record, and nothing else | ✅ 2026-11-04, 219 in a second folder: `apps/Half-Life 2`, its three manifests and its rows gone; the folder's `.ossteam/library.toml` and the other apps' rows kept | ⏳ |
| A second library folder: install there, `folders list` shows it, removal refused while used | ✅ 2026-11-04: 219 installed there; removal refused with exit 8 while it was, allowed after the uninstall | ⏳ |
| `jobs pause`, `resume`, `cancel`, `priority` | ✅ 2026-11-04: pause held 219 at 155.7 MiB, resume went on; priority 5 on a queued job kept it queued behind the running one (`ipc.md` §5.6); cancel of 22150's first install at 3 % emptied staging and left it not installed, with the 349 files already in place left in `apps/` | ⏳ |
| `content.bandwidth_limit` holds the rate within 10 % | ✅ 2026-11-04, 18030: 2,031 KiB/s at a limit of 2,048 (40 s) and 3,954 at 4,096 (20 s), once each read was charged what it returned; before that, 490 to 690 KiB/s at 2,048 | ⏳ |
| `content.concurrent_jobs` 2 runs two installs at once | ✅ 2026-11-04: set while 219 ran, 18010 started at once | ⏳ |
| A beta branch without a password installs | ✅ 2026-11-04, Spacewar (480) `previous` (build 316058): depot 481 at that branch's manifest; the shared depot 229006 skipped, since app 228980 has no `previous` branch (`content.md` §6 rule 7, unverified against the official client); reinstalled from `public` after | ⏳ |
| Install scripts run (CONT-8) | n/a | ⏳ |
| §20: download saturates 1 Gbit/s on reference hardware with default settings | ⏳ | ⏳ |

The fake CDN tests check the daemon against our spec pages, not against
Steam. They do not tick any box here: each check still needs a live run
that matches the official client (§19.3).

✅ verified · ⏳ not yet run
