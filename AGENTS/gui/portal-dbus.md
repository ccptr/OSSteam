---
name: portal-dbus
description: The desktop portal goes through ossteam-ui's own blocking D-Bus client (dbus/, portal.rs); waits sleep in read and never wake; don't bring zbus back (its blocking pool woke every 500 ms)
metadata:
  type: reference
---

On Linux, `ossteam-ui` calls the desktop portal (the colour scheme, its
change signal, the file chooser) through its own D-Bus client,
`src/dbus/` (wire format in `wire.rs`, connection and login in
`mod.rs`), with the portal's calls in `src/portal.rs` (ADR 0004,
amended 2026-11-03). It is synchronous: each call blocks its thread, and
a thread waiting for a signal sleeps in `read`, so it costs no wakeups
(GUI-5). It does only what the portal needs: local sockets, `EXTERNAL`,
no file descriptors, and calls to us answered with an error (`Ping`
excepted).

- **A portal request's answer is a signal.** Add the match for the
  request path (`/org/freedesktop/portal/desktop/request/<sender>/<token>`)
  before the call; signals that come while a call waits are queued for
  `next_signal`. Portals before 0.9 choose the path; `pick_folder` adds
  a second match then.
- **Settings version 1** has no `ReadOne`, and `Read` wraps the value in
  a second variant; `color_scheme` falls back and unwraps.
- **A killed dialog** answers response 2 ("ended"), not 1 (cancelled), so
  the picker reports an error.
- **Don't bring zbus back** (or `ashpd`, or `rfd`'s portal back end): with
  async-io, zbus connected the bus on the `blocking` crate's pool, whose
  last thread then woke every 500 ms for the life of the process.
- Testing: unit tests run against a bus on a socket pair
  (`dbus::test_bus`); live runs use `fake_portal.py` or the real portal on
  a private bus ([headless sway input](../testing/headless-sway-input.md)).
  Check idle threads with [idle wakeups](../testing/idle-wakeups.md).
