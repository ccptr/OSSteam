---
name: portal-wakeups
description: zbus connecting the session bus starts the blocking crate's pool, which wakes every 500 ms forever; connect the socket yourself
metadata:
  type: reference
---

When zbus (under ashpd, with async-io) connects to the session bus, it
runs the socket's `connect` on the `blocking` crate's thread pool, and
the pool's last thread never exits: it wakes every 500 ms for the life
of the process, which breaks GUI-5. `ossteam-ui`'s `appearance.rs`
connects the socket itself and passes it to
`zbus::connection::Builder::async_io_unix_stream`, then uses ashpd's
`with_connection` constructors. Any other portal call must do the same
(ashpd's request builders take `.connection(…)`); `rfd`'s portal back
end does not. Check with per-thread switches
([idle wakeups](../testing/idle-wakeups.md)): a `blocking-N` thread that
switches twice a second is this.
