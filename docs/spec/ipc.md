# Client IPC protocol

| | |
|---|---|
| Status | Implemented for M0 (`Hello`, Daemon methods, subscriptions) |
| Requirements | SPEC §8 (IPC-1 to IPC-10), D-1, D-2, D-5, CONC-1 |
| Schema | `crates/ossteam-ipc/proto/ossteam/ipc/v1/ipc.proto` |
| Code | `crates/ossteam-ipc` (both sides), `crates/ossteamd/src/{core,server}.rs` |

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

| Topic | Events (M0) |
|---|---|
| `DAEMON` | `DaemonStopping`: sent once, just before shutdown closes connections |
| `NOTICES` | `Notice{level, text}` |
| `SESSION`, `LIBRARY`, `JOBS`, `GAMES` | Subscribable now; their events arrive with M1a to M1c |

## 5. Methods (M0)

| Method | Behaviour |
|---|---|
| `GetStatus` | Daemon version, pid, uptime, Steam connection state, account (unset when logged off), connected clients, active jobs, running games. In M0 the connection is `DISCONNECTED` and the counts are 0, which is the true state: the daemon has no Steam connection, jobs or games yet. |
| `GetVersion` | Daemon version, protocol version, and the OS and architecture the daemon was built for. |
| `Shutdown{force}` | Replies, then shuts down (§6). Once games can run (M1c), a request without `force` is refused with `CONFLICT` while any game the daemon launched is running (D-5). |
| `Subscribe`, `Unsubscribe` | See §4. |

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

The client library runs one reader thread per connection. That thread
routes each reply to the waiting call by id and queues events. Calls can
come from any thread, and each one has a timeout (30 s by default).

## 8. Error codes and CLI exit codes

| `ErrorCode` | CLI exit code (SPEC §15.3) |
|---|---|
| `NOT_LOGGED_IN` | 4 |
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
