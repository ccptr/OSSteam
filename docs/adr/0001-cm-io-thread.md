# ADR 0001: One poll-driven I/O thread per CM connection

| | |
|---|---|
| Status | Accepted |
| Date | 2026-10-17 |
| Requirements | CONC-1, CONC-2, SYNC-1, §20 (idle wakeups) |
| Code | `crates/ossteam-net/src/{conn,transport}.rs` |

## Context

Draft 1 of CONC-1 gave each CM connection "one reader thread and one
writer thread, using blocking sockets". The primary transport is a
WebSocket inside TLS (NET-2). Neither rustls nor tungstenite can be split
into independent read and write halves: both keep one state machine per
connection. Two blocking threads would have to share it behind a lock,
and the reader would hold that lock while it waits for data. That leaves
two options, and both are bad:

- The reader holds the lock while blocked. A write then waits for the
  reader's read timeout, up to 1 s under CONC-2, which adds up to a second
  of latency to every request on an idle connection.
- The reader uses short read timeouts to give the lock back often. That
  means many wakeups per second on an idle connection, against the §20
  budget of at most one.

## Decision

Each CM connection has a single I/O thread. The socket is non-blocking
and is registered with a readiness poller (the `polling` crate: epoll,
kqueue or IOCP, used synchronously). The thread waits on the poller and
wakes up when one of these happens:

- the socket becomes readable, or writable while output is queued;
- another thread queues a message (`Poller::notify`);
- the next heartbeat is due;
- its `CancelToken` is cancelled (a hook registered with
  `CancelToken::on_cancel` calls `Poller::notify`).

After every wakeup it drains the outbox and reads every complete message
that is buffered. Callers still use the blocking API that CONC-1
describes: `request` and `call` wait on a one-shot channel with a
deadline, matched by job id.

Connection setup (TCP connect, TLS and WebSocket handshakes) is also
non-blocking and waits in slices of at most 1 s, so cancellation is
honoured throughout. The one exception is host name resolution, which
uses the system resolver and cannot be interrupted.

## Consequences

- Still synchronous: there is no async runtime and no futures (SYNC-1).
  This ADR does not fall under CONC-3, because nothing here is async.
- An idle connection wakes only for its heartbeats and for what the
  server sends.
- Writes go out immediately, whatever the reader is doing.
- There is one thread per connection where the draft had two, and the
  TCP fallback transport will use the same loop.
- `polling` and `socket2` (non-blocking connect) are added as
  dependencies.

## Amendment, 2026-10-19 (accepted 2026-11-03)

The I/O thread first also woke at the end of every 1 s cancellation slice
of CONC-2. With heartbeats on top, an idle logged-in connection then woke
1.03 times a second on the development machine (5 minutes against the
fake CM, heartbeat every 9 s), and the daemon as a whole 1.07 times, over
the §20 budget of one. A slice of 1 s can never stay under that budget once
anything else wakes the process, so the thread now waits for its next
heartbeat, and cancellation reaches it through the poller instead.
Connection setup keeps its slices: it is not idle.
