#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""A stand-in desktop portal FileChooser for a private session bus, for
testing the folder picker without a dialog (AGENTS/testing/headless-sway-input.md).

Each OpenFile call takes the next answer from argv: a path (picked, sent
as a file:// URI) or "cancel". Answers come 300 ms after the call.
"""
import sys
import urllib.parse

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

XML = """
<node>
  <interface name="org.freedesktop.portal.FileChooser">
    <method name="OpenFile">
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="o" name="handle" direction="out"/>
    </method>
    <property name="version" type="u" access="read"/>
  </interface>
</node>
"""
REQUEST_XML = """
<node>
  <interface name="org.freedesktop.portal.Request">
    <method name="Close"/>
  </interface>
</node>
"""

answers = list(sys.argv[1:])
loop = GLib.MainLoop()


def log(*a):
    print("fake_portal:", *a, file=sys.stderr, flush=True)


def on_call(conn, sender, path, iface, method, params, invocation):
    title, options = params[1], params[2]
    token = options.get("handle_token", "t")
    handle = "/org/freedesktop/portal/desktop/request/%s/%s" % (
        sender[1:].replace(".", "_"),
        token,
    )
    request_info = Gio.DBusNodeInfo.new_for_xml(REQUEST_XML).interfaces[0]
    reg = conn.register_object(
        handle,
        request_info,
        lambda c, s, p, i, m, a, inv: inv.return_value(None),
        None,
        None,
    )
    answer = answers.pop(0) if answers else "cancel"
    log("OpenFile", repr(title), dict(options), "->", answer)
    invocation.return_value(GLib.Variant("(o)", (handle,)))

    def respond():
        if answer == "cancel":
            body = GLib.Variant("(ua{sv})", (1, {}))
        else:
            uri = "file://" + urllib.parse.quote(answer)
            body = GLib.Variant(
                "(ua{sv})", (0, {"uris": GLib.Variant("as", [uri])})
            )
        conn.emit_signal(
            sender, handle, "org.freedesktop.portal.Request", "Response", body
        )
        conn.unregister_object(reg)
        return False

    GLib.timeout_add(300, respond)


def on_get(conn, sender, path, iface, prop):
    return GLib.Variant("u", 4)


def on_bus(conn, name):
    info = Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0]
    conn.register_object(
        "/org/freedesktop/portal/desktop", info, on_call, on_get, None
    )


Gio.bus_own_name(
    Gio.BusType.SESSION,
    "org.freedesktop.portal.Desktop",
    Gio.BusNameOwnerFlags.NONE,
    on_bus,
    lambda c, n: log("ready"),
    lambda c, n: (log("name lost"), loop.quit()),
)
loop.run()
