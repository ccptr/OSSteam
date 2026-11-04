# Client IPC protocol

| | |
|---|---|
| Status | Implemented: `Hello`, Daemon methods, subscriptions (M0); Session methods, `ListApps`, `GetApp` (M1a) |
| Requirements | SPEC §8 (IPC-1 to IPC-10), D-1, D-2, D-5, CONC-1, NET-4, AUTH-1 to AUTH-4 |
| Schema | `crates/ossteam-ipc/proto/ossteam/ipc/v1/ipc.proto` |
| Code | `crates/ossteam-ipc` (both sides), `crates/ossteamd/src/{core,server}.rs`, `crates/ossteamd/src/session/` |

This is the protocol between `ossteamd` and its clients: the `ossteam`
CLI, `ossteam-gui`, and any third-party client. The `.proto` file is the
normative message schema. This page covers everything around it.

## 1. Endpoints

| OS | Endpoint | Lock file |
|---|---|---|
| Linux | Unix socket `$XDG_RUNTIME_DIR/ossteam/daemon.sock` | `$XDG_RUNTIME_DIR/ossteam/daemon.lock` |
| macOS | Unix socket `$TMPDIR/ossteam/daemon.sock` | `$TMPDIR/ossteam/daemon.lock` |
| Windows | Named pipe `\\.\pipe\ossteam-<SID>` | `%LOCALAPPDATA%\ossteam\data\daemon.lock` |

`<SID>` is the string SID of the user, such as `S-1-5-21-…`.

**Unix.** The `ossteam` directory is created with mode 0700. The daemon
tightens an existing directory it owns to 0700. Both the daemon and clients
refuse a directory that is not owned by the effective uid, or that grants
any group or other permission bits. The socket is created with mode 0600.
On Linux the mode is set before `bind`, so there is no umask race. On macOS
it is set right after `bind`, and the 0700 directory is what protects it.
If `XDG_RUNTIME_DIR` is unset on Linux, clients and the daemon fail with an
error that says so. They do not fall back to `/tmp`.

**Windows.** The pipe is created with the security descriptor
`D:P(A;;GA;;;<SID>)`: a protected DACL with a single entry that grants
generic-all to the user's SID. There is nothing to clean up, because a
named pipe disappears with its last handle.

**Test isolation (Windows, debug builds).** Unix tests give a daemon its
own endpoint and folders by pointing `XDG_RUNTIME_DIR` or `TMPDIR` and the
XDG variables at a temporary directory. Windows takes the pipe name from
the SID and the folders from the shell's known folders, so no variable
moves them. Debug builds therefore read `OSSTEAM_TEST_ROOT`, an absolute
directory path. When it is set, the pipe is `ossteam-test-<directory name>`,
the game pipe is `ossteam-game-test-<directory name>`, the lock file is
`<directory>\data\daemon.lock`, and the daemon's config, data, cache and
state folders are `config`, `data`, `cache` and `state` under it. The
pipes keep the same security descriptor. Release builds ignore the
variable.

**Peer check (IPC-2).** After `accept`, before reading anything, the daemon
checks the peer:

- Linux: the effective uid from `SO_PEERCRED` must equal the daemon's.
- macOS: the effective uid from `getpeereid` (via `LOCAL_PEERCRED`) must equal the daemon's.
- Windows: the daemon opens the client process, which it finds with
  `GetNamedPipeClientProcessId`, and its token user SID must equal the
  daemon's.

A peer that fails the check is disconnected without a reply.

Nothing listens on TCP (IPC-3).

## 2. Framing

Each frame is a 4-byte little-endian length `n` followed by `n` bytes of a
protobuf-encoded `Envelope`. Frames longer than 16 MiB (16 777 216 bytes)
are a protocol error: the receiver closes the connection without reading
the body. A connection that ends in the middle of a frame is an I/O error.
One that ends between frames is a clean close.

## 3. Connection lifecycle

1. The client connects and sends `Envelope{id: 0, hello: Hello{…}}`.
   `Hello.protocol` is the client's protocol version, `kind` is one of
   `CLI`, `GUI` or `OTHER`, and `name` and `version` identify the program.
2. The daemon waits at most 5 s for this frame (on Unix; Windows named
   pipes have no read timeouts, so there the wait is unbounded). Anything other than a
   `Hello` is answered with `Error{code: PROTOCOL}`, and the connection is
   closed.
3. If `protocol.major` differs from the daemon's major version, the daemon
   answers `Error{code: INCOMPATIBLE_VERSION, detail: "supported: 1.0 to
   1.x"}` with `id = 0` and closes. A client with a newer minor version is
   accepted.
4. Otherwise the daemon answers with its own `Hello`, with `kind = DAEMON`.
   A client must check the major version in that reply as well.
5. From then on the client sends only `Request`s. The daemon sends
   `Response`s, `Error`s and `Event`s.

**Request ids.** The client picks a non-zero `id` for each request, and the
reply (`Response` or `Error`) carries the same `id`. The ids of outstanding
requests must be unique on the connection. Requests on one connection are
processed in the order they are sent. A request with `id = 0` is answered
with `Error{id: 0, code: PROTOCOL}`.

**Unknown methods.** A `Request` whose `method` is unset, as happens when a
newer client calls a method this daemon does not know, is answered with
`Error{code: UNSUPPORTED}`. The connection stays open.

**Unknown results.** A `Response` with no `result` that a client can
decode, which happens when an older client receives a newer result kind,
should be treated by the client as `UNSUPPORTED`.

**Connection-level errors.** An `Error` with `id = 0` that arrives after the
handshake concerns the connection as a whole, such as a malformed envelope.
The daemon closes the connection after a malformed envelope.

**Back-pressure.** The daemon queues up to 1024 outgoing frames per
connection. A client that lets that queue fill up has stopped reading. The
daemon then drops it from its state and sends it nothing further.

## 4. Events

A connection receives no events until it subscribes. `Subscribe{topics}`
adds topics, `Unsubscribe{topics}` removes them, and both reply with the
connection's complete topic set. `TOPIC_UNSPECIFIED`, or a number this
daemon does not know, is rejected with `INVALID_ARGUMENT`, and the topic set
is left unchanged. Events always have `id = 0`.

| Topic | Events |
|---|---|
| `DAEMON` | `DaemonStopping`: sent once, just before shutdown closes connections; `SettingChanged{setting}` (§5.4) |
| `NOTICES` | `Notice{level, text}` |
| `SESSION` | `SessionChanged`, `LoginPrompt`, `LoginEnded` (§5.2) |
| `LIBRARY` | `AppChanged{app_ids, complete}` (§5.3) |
| `JOBS` | `JobStateChanged{job}`, `JobProgress{job_id, progress}` (§5.6) |
| `GAMES` | `GameStarted{game}`, `GameExited{app_id, exit_code \| signal, seconds}`, `ShimReport{app_id, pid, launched, interfaces, unknown_versions, unimplemented}` (§5.7) |

## 5. Methods

### 5.1 Daemon

| Method | Behaviour |
|---|---|
| `GetStatus` | Daemon version, pid, uptime, the session (§5.2: connection state, active account, `logged_on`, `session_reason`), connected clients, active jobs, the games the daemon launched that are running, and on Linux `game_isolation` (field 12): `available`, `reason` when not, `launches_blocked` and `holds_official_entries` (`steamworks-abi.md` §1.1.6). |
| `GetVersion` | Daemon version, protocol version, and the OS and architecture the daemon was built for. |
| `Shutdown{force}` | Replies, then shuts down (§6). A request without `force` is refused with `CONFLICT` while any game the daemon launched is running (D-5). With `force`, the games keep running. |
| `Subscribe`, `Unsubscribe` | See §4. |

### 5.2 Session

The session belongs to the daemon (IPC-9). Its state is what `GetStatus`
returns and what `SessionChanged` carries:

| Field | Meaning |
|---|---|
| `connection` | `CONNECTED` while a CM connection is up, `CONNECTING` while one is being attempted or a retry is scheduled (NET-4), `DISCONNECTED` otherwise |
| `account` | The active account: the one logged on, or the one being reconnected. Unset when there is none |
| `logged_on` | Whether `account` is logged on right now |
| `reason` / `session_reason` | Why the session last ended or failed to resume, in words for people. Empty after a successful logon |
| `login_required` (event only) | The stored token was rejected or is missing; the account must log in again |
| `persona_state` | The active account's chosen persona state; unspecified without an account |

A connection can be `CONNECTED` with `logged_on` false: during a login, and
between connecting and the logon answer.

| Method | Behaviour |
|---|---|
| `LoginWithCredentials{username, password, remember}` | Starts a credential login (`auth.md` §2). The response comes once Steam has accepted the start, which may take a while if servers must be tried first, so clients should allow 90 s. Wrong passwords and other refusals fail here with `NOT_LOGGED_IN`; Steam being unreachable or rate-limiting fails with `NETWORK`. |
| `LoginWithQr{remember}` | Starts a QR login (`auth.md` §3). |
| `SubmitGuardCode{code, kind}` | Answers an `EMAIL_CODE` or `DEVICE_CODE` prompt. `kind` may be left unspecified when the login offers one kind of code. A rejected code fails with `NOT_LOGGED_IN`; the login stays open for another try. Without a login waiting, `CONFLICT`. |
| `CancelLogin` | Ends the login in progress, if any, and succeeds either way. |
| `Logout` | Logs the active account off and deletes its stored token. `NOT_LOGGED_IN` when there is no active account; `CONFLICT` while a game the daemon launched runs. |
| `ListAccounts` | The remembered accounts (those with a stored token) and the active account's Steam id, or 0. |
| `SwitchAccount{steam_id}` | Logs off the active account and logs on as a remembered one (`auth.md` §6). Replies once that logon has succeeded or failed. `NOT_FOUND` for an account that is not remembered; `CONFLICT` while a game the daemon launched runs. |
| `SetPersonaState{state}` | Stores `ONLINE`, `INVISIBLE` or `OFFLINE` for the active account and sends it to Steam if the account is logged on; it is sent again after every logon (`pics.md` §8). Replies at once, then `SessionChanged` carries the new state. `NOT_LOGGED_IN` without an active account. |

**Login flow.** A client subscribes to `SESSION` first, then starts a
login. After `LoginStartedResponse` the daemon sends:

- `LoginPrompt{kind, hint, qr_url}` for each thing Steam needs: a QR URL to
  show (again whenever it rotates), a code to enter, or an approval to make
  in the Steam mobile app or by email. Several prompts may be open at
  once; the user satisfies any one of them, and the daemon keeps polling
  meanwhile, so approving in the app finishes the login even while a code
  prompt waits.
- exactly one `LoginEnded`, with the account on success or an `Error`
  otherwise. On success the account is already logged on.

One login runs at a time; a second one fails with `CONFLICT`. A login
belongs to the connection that started it: if that connection closes, the
daemon cancels the login. Starting a login while an account is active logs
that account off first (`auth.md` §6).

### 5.3 Library

The library is the set of apps the active account's licenses grant
(`pics.md` §7). The daemon answers from the last library the PICS worker
built (§7), so these calls never wait on Steam's servers. All fail with
`NOT_LOGGED_IN` without an active account.

| Method | Behaviour |
|---|---|
| `ListApps{filter, installed_only}` | Top-level apps sorted by name, each with its DLC. `filter` keeps apps whose name contains it, ignoring case, or whose id it is; a matching DLC is listed under its parent even if the parent does not match. `complete` is false until the license list has arrived and every needed info has been fetched or refused, and again after a fetch fails, until the retry succeeds. `packages_without_info` names granted packages whose apps cannot be listed. `installed_only` keeps installed apps, and `update_available` marks an installed app whose branch has manifests other than the installed ones (`gui.md` §5). |
| `GetApp{app_id}` | The app's summary and its cached info: platforms, install directory, depots, branches and launch options, on Linux its compatibility tool (`compat-tools.md` §8), and the last `ShimReport` sent for the app on the active account, which the daemon stores (`gui.md` §5). `NOT_FOUND` when the app is not in the library. An app whose info is not available has only its summary. |
| `GetAsset{app_id, kind, high_dpi}` | The absolute path of one of the app's images (`HEADER`, `CAPSULE`, `HERO`, `HERO_BLUR`, `LOGO`), downloaded into the cache first if it is not there (`pics.md` §9). This is the one Library method that may wait on the network; clients should allow 90 s. Requests for the same image share one download. `NOT_FOUND` when the app is not in the library, its info is not available, the info names no such image, or the CDN has none (remembered until the daemon restarts); `NETWORK` when the download fails; `INVALID_ARGUMENT` for an unknown kind. |

`AppChanged` lists the apps that were added to or removed from the
library, or whose info was fetched again, and whether the library is now
complete. It comes after the first library is built, after license
changes, and after change polling (`pics.md` §4) fetched something.
Clients read the apps again with `ListApps` or `GetApp`.

### 5.4 Settings

Settings come in three layers (D-9). An override set with `SetSetting`
wins over `ossteamd.toml` in the config directory, and the file wins over
the built-in default. The daemon reads the file once at start, so edits to
it apply after a restart. Overrides are kept in the state database (D-6)
and apply at once. Keys are dotted, as in the file: `[secrets]
file_fallback = true` and `secrets.file_fallback = true` are the same key.
An unknown key in the file is logged as a warning and ignored; a known key
with a value of the wrong type stops the daemon from starting, with the
file and key named.

| Method | Behaviour |
|---|---|
| `GetSettings` | Every setting this daemon knows, sorted by key: its kind, the value in effect, where that value comes from (`DEFAULT`, `FILE` or `OVERRIDE`), the default, the file's value if the file sets one, and a description. |
| `SetSetting{key, value, reset}` | Stores `value` as the key's override, or with `reset` removes the override so the file or default applies again. Replies with the setting as it is afterwards. `NOT_FOUND` for an unknown key; `INVALID_ARGUMENT` for a value of the wrong form. Resetting a key that has no override succeeds. |

`SettingChanged` on `DAEMON` carries the setting whenever a `SetSetting`
changed its value or its source.

Values are written as text: `true` or `false` for `BOOL`, a decimal
number for `INTEGER`, and one of the words the description lists for
`STRING`. In `ossteamd.toml` they are TOML booleans, integers and strings.
These keys exist:

| Key | Kind | Default | Effect |
|---|---|---|---|
| `content.auto_update` | `STRING` | `always` | `always` queues an update job when an installed app gets a new build; `never` only marks the app (`content.md` §10) |
| `content.bandwidth_limit` | `INTEGER` | `0` | KiB/s for all downloads together, 0 to 2^30; 0 is no limit. Applies at once |
| `content.concurrent_jobs` | `INTEGER` | `1` | Jobs that run at once, 1 to 8 |
| `content.download_threads` | `INTEGER` | `8` | Chunk downloads at once per job, 1 to 64. Applies to jobs started after the change |
| `content.language` | `STRING` | `english` | The language whose depots are installed, as Steam names languages (`german`, `schinese`, ...) |
| `compat.default_tool` | `STRING` | `proton_experimental` | Linux: the compatibility tool for apps without a Linux version that no other mapping covers; empty for none (`compat-tools.md` §2) |
| `compat.tool.<app id>` | `STRING` | empty | Linux: the user's compatibility tool for one app, a tool's name, a selectable alias, or `none` for no tool (`compat-tools.md` §2); empty for Valve's mapping. A family of keys: `GetSettings` lists those with an override (`compat-tools.md` §8) |
| `discovery.take_over_official` | `BOOL` | `false` | Linux: take over an official Steam client's `~/.steam` entries while it is not running, hand them back afterwards, and yield them when it starts (`steamworks-abi.md` §1.1.5). The description states its two risks; the CLI prints them when it is turned on, and the GUI asks to confirm |
| `secrets.file_fallback` | `BOOL` | `false` | When no OS keyring is available, store refresh tokens in `secrets.json` in the data directory, mode 0600 (D-8, `auth.md` §4). A change applies to the next token read or write. |
| `workshop.cache_limit` | `INTEGER` | `0` | MiB for the cached Workshop items of all library folders together, 0 to 2^30; 0 is no limit. Over it, the least recently used cached items are deleted (`workshop.md` §4) |

**Secrets (IPC-10).** The password and Steam Guard codes travel only in
their request. Both sides zero every frame buffer after use, the client
library zeroes its copy of the request once it is sent, and the daemon
moves the value into a type that is zeroed on drop.

### 5.5 Library folders

A library folder (SPEC §17, `content.md` §8) is a directory that holds
installed apps. Each one is marked by `.ossteam/library.toml`, which
records the folder's id, a random 128-bit number written in hex. The
daemon keeps the list of folders in its state database. The default
folder is `library` in the data directory; it is created and added the
first time a folder is needed, and it cannot be removed.

| Method | Behaviour |
|---|---|
| `ListLibraryFolders` | Every folder: its absolute path, whether it is the default, the free space on its file system, and the apps installed in it. A folder that cannot be read (an unplugged drive) is listed with `available` false. |
| `AddLibraryFolder{path}` | Adds a folder. `path` must be absolute; the directory is created if it does not exist. A directory that already has a `library.toml` keeps its id, but apps installed there by another daemon are not known to this one. `CONFLICT` if the folder, or a folder with the same id, is already in the list; `INVALID_ARGUMENT` for a relative path or one that is inside another folder of the list, or contains one. |
| `RemoveLibraryFolder{path}` | Removes a folder from the list. Its files are left alone, `library.toml` included. `CONFLICT` while an app is installed there or a job uses it, and for the default folder; `NOT_FOUND` for a folder not in the list. |

None of these need an account.

### 5.6 Jobs

A job installs, updates, verifies or uninstalls one app (`content.md` §9).
Jobs belong to the active account: they are created for it, and they run
only while it is logged on, because they need depot keys and request
codes, and once its library is complete, because what it owns decides
which depots are installed (`content.md` §6). Jobs of an account that is
not active wait.

| Field | Meaning |
|---|---|
| `job_id` | Chosen by the daemon, never reused |
| `app_id`, `kind` | `INSTALL`, `UPDATE`, `VERIFY` or `UNINSTALL` |
| `state` | `QUEUED`, `RUNNING`, `PAUSED`, `DONE`, `FAILED` or `CANCELLED` |
| `priority` | Higher runs first; jobs of equal priority run in the order they were created. Default 0 |
| `progress` | The engine's progress (`content.md` §9): the phase, file bytes to write and written, bytes to download and downloaded (counting earlier runs of a resumed job), files to write and written |
| `error` | Why a `FAILED` job failed, as an `Error` whose code is the one the job would have failed a request with |
| `folder`, `branch` | Where the app goes and from which branch |

`content.concurrent_jobs` jobs run at once (default 1). A running job
whose account goes offline is queued again and continues from its
journal after the next logon. A job that fails, for example because no
content server answered, stays resumable: `ResumeJob` queues it again,
and it continues from its journal. Jobs that were running when the
daemon stopped are queued again when it starts. `DONE` and `CANCELLED`
jobs are listed until the daemon restarts; `FAILED` jobs stay until they
are resumed or cancelled, and count as unfinished.

| Method | Behaviour |
|---|---|
| `Install{app_id, folder, branch}` | Queues an install and replies with the job. `folder` empty means the default folder; `branch` empty means `public`. Before queueing, the daemon checks what it can without the network: the app is in the library (`NOT_FOUND`), its info names an install directory and the branch exists and needs no password (`UNSUPPORTED` for a password-protected branch, `NOT_FOUND` for an unknown one), the folder is in the list (`NOT_FOUND`), and the app is not installed and has no unfinished job (`CONFLICT`). Entitlement and disk space are checked when the job runs. |
| `Update{app_ids, all}` | Queues an update of each named app, or with `all` of every installed app whose update is known (`content.md` §10). Replies with the jobs created. An app with an unfinished job gets no second one; `NOT_FOUND` names an app that is not installed. An update of an app that has no new manifests finishes at once and writes nothing. |
| `Verify{app_id}` | Queues a verify of an installed app. `NOT_FOUND` when it is not installed, `CONFLICT` when it has an unfinished job. |
| `Uninstall{app_id, remove_compat_data}` | Queues an uninstall (`content.md` §12); with `remove_compat_data`, on Linux, the app's prefix goes too (`compat-tools.md` §6.1). An unfinished install, update or verify of the app is cancelled first. `NOT_FOUND` when the app is neither installed nor being installed. |
| `ListJobs` | Every job of the active account, unfinished ones first. |
| `PauseJob{job_id}` | A queued or running job becomes `PAUSED`; a running one stops within a second and keeps its journal. |
| `ResumeJob{job_id}` | A paused or failed job is queued again. |
| `CancelJob{job_id}` | Ends a job that has not finished. Staged files are deleted. What an install or update already put in place stays: the app is left as it was before the job only if no file had been renamed yet, and is otherwise marked as needing an update. |
| `SetJobPriority{job_id, priority}` | Changes the priority of an unfinished job. A running job is not stopped by it. |

The job methods need an active account (`NOT_LOGGED_IN`) and
`NOT_FOUND` for a job id that is not the active account's. Pausing,
resuming or cancelling a job in a state where that makes no sense
(cancelling a finished job, resuming a running one) is `CONFLICT`.

Events on `JOBS`:

- `JobStateChanged{job}` whenever a job is created or its state, priority
  or error changes;
- `JobProgress{job_id, progress}` while a job runs, at most ten times a
  second for each job.

### 5.7 Games

Launching is specified in `launch.md`; §8 there lists the methods
(`GetLaunchOptions`, `Launch`, `ListRunning`, `StopGame`), the running
game they return and the events on `GAMES`. `GetApp` lists the same
launch options without `qualifies`. `ShimReport` is specified in
`steamworks-abi.md` §4.

## 6. Daemon lifecycle

**Single instance (D-1).** On start the daemon creates the runtime
directory, then takes an exclusive advisory lock on the lock file, and only
after that binds the endpoint. Holding the lock makes it safe to remove a
stale socket file left by a crash. The daemon writes its PID into the lock
file for people to read, but the PID is never used for decisions. If the
lock is already held, the new process repeatedly tries to connect and
call `GetStatus` for up to 3 s:

- If the other daemon answers, the new process exits with status 0.
- If the lock becomes free during that time (the other daemon stopped), the
  new process exits with status 1 and does not take over, because the other
  daemon may have been stopped on purpose.
- Otherwise it logs that the lock is held by a process that does not answer,
  and exits with status 1.

**Autostart (D-2).** When a connect fails because nothing is listening
(`NotFound` or `ConnectionRefused`), a client that allows autostart runs
`ossteamd --background` and keeps retrying the connection for up to 10 s.
It looks for the program in this order: `$OSSTEAM_DAEMON`, then `ossteamd`
next to the client's own executable, then `ossteamd` on `PATH`. With
`--background`, the daemon calls `setsid()` on Unix so that it leaves the
client's session. On Windows it is created with
`CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`, plus
`CREATE_BREAKAWAY_FROM_JOB` when the client's job object allows it. When
several clients start the daemon at once, one process wins the lock and the
others exit. A client reports that it started the daemon only if the PID
in `GetStatus` is the PID of the process it spawned.

**Shutdown (D-5).** A `Shutdown` request, `SIGINT` or `SIGTERM` (or a
console control event on Windows) ends the core loop. The daemon then:

1. queues the `Shutdown` response (for a request) and `DaemonStopping` to
   every subscriber of `DAEMON`;
2. stops accepting connections, which removes the Unix socket file;
3. waits up to 2 s for the per-connection writers to flush their queues;
4. exits, and the OS releases the lock.

A client that wants to know when the daemon is gone waits for its
connection to close. `ossteam daemon stop` does this, with a 10 s limit, so
that `daemon stop && daemon start` always gets a fresh daemon.

## 7. Threads

The design follows CONC-1. One acceptor thread runs the endpoint. Each
connection gets a reader thread, which does the handshake and then
forwards requests, and a writer thread, which drains that connection's
queue. A single core thread owns all state and handles every request in
order. A blocking `accept` cannot be interrupted, so on shutdown the daemon
sets a flag and then connects to itself once to wake the acceptor.

The session worker (`crates/ossteamd/src/session/`) is a separate thread
that owns the CM connection and the keyring, because both block: network
calls take seconds and a locked keyring may wait for the user. The core
forwards session requests to it, and it reports replies, state changes and
prompts back on the core's command channel. The core keeps the state
database and answers `GetStatus` and `ListAccounts` itself, so they never
wait on the network. A session request that waits on Steam is answered
when the worker reports back.

The PICS worker (`crates/ossteamd/src/pics.rs`) is another thread. The
session worker tells it the active account, gives it a handle for
requests after each logon, and forwards the license list. It fetches
product info, polls for changes, writes the PICS cache, and reports each
new library to the core, which answers Library methods from it. `GetApp`
reads the cache file itself, which is safe because the worker replaces
files by renaming.

Image downloads for `GetAsset` run on four threads of their own
(`crates/ossteamd/src/assets.rs`). The core answers a request for an image
already in the cache itself, and otherwise queues one download per image
and answers every waiting request when it finishes.

Content jobs (`crates/ossteamd/src/content/`, `content.md` §9) are
scheduled by the core, which keeps them in the state database and decides
which run: queued jobs of the active account, highest priority first, at
most `content.concurrent_jobs` at once, and only while the account is
logged on. CONC-1 names a scheduler thread; a separate one would only
relay what the core already knows, so the core does it. Each running job
gets a thread of its own, which fetches keys, manifests and request codes
through the session's request handle and runs the install engine with
`content.download_threads` download threads. It reports progress, its
journal and its result back on the core's command channel. All jobs share
one HTTPS agent, one bandwidth throttle and one content server list. To
pause, cancel or shut down, the core cancels the job's token; the job's
threads stop within a second (CONC-2), and on shutdown the core waits for
them and stores their last journals before it exits.

The client library runs one reader thread per connection. That thread
routes each reply to the waiting call by id and queues events. Calls can
come from any thread, and each one has a timeout (30 s by default),
except `Launch`, whose reply can wait for tool installs (`launch.md` §8)
and which ends when the reply comes or the connection closes.

## 8. Error codes and CLI exit codes

| `ErrorCode` | CLI exit code (SPEC §15.3) |
|---|---|
| `NOT_LOGGED_IN` | 4 (also authentication failures) |
| `NOT_FOUND` | 5 |
| `NOT_OWNED` | 6 |
| `UNSUPPORTED` | 7 |
| `CONFLICT` | 8 |
| `NETWORK` | 9 |
| `PROTOCOL`, `INCOMPATIBLE_VERSION`, `INVALID_ARGUMENT`, `INTERNAL`, `UNSPECIFIED` | 1 |

Exit code 3 (daemon unavailable) is produced by the client itself when it
cannot connect to the daemon or start it.

## 9. Compatibility rules

- Within major version 1, changes only add fields, messages, enum values
  and methods. Field numbers are never reused or renumbered.
- A client must tolerate enum values it does not know. Prost decodes them
  as raw integers.
- Removing or changing the meaning of anything requires a new major
  version and a new package, `ossteam.ipc.v2`. The daemon may serve
  several major versions at once.
