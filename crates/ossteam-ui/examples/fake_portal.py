#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""A stand-in desktop portal for a private session bus, for testing the
folder picker without a dialog and the appearance without changing the
desktop (AGENTS/testing/headless-sway-input.md).

FileChooser: each OpenFile call takes the next answer from argv: a path
(picked, sent as a file:// URI) or "cancel". Answers come 300 ms after
the call.

Settings: `org.freedesktop.appearance color-scheme` is 1 (prefer dark)
unless an argument `scheme=N` says otherwise; each `change=N@MS` argument
sets it to N, with its SettingChanged signal, MS milliseconds after the
portal starts. Version 1 of Settings, with Read but no ReadOne, if an
argument is `settings-v1`.
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
  <interface name="org.freedesktop.portal.Settings">
    <method name="Read">
      <arg type="s" name="namespace" direction="in"/>
      <arg type="s" name="key" direction="in"/>
      <arg type="v" name="value" direction="out"/>
    </method>
    <method name="ReadOne">
      <arg type="s" name="namespace" direction="in"/>
      <arg type="s" name="key" direction="in"/>
      <arg type="v" name="value" direction="out"/>
    </method>
    <signal name="SettingChanged">
      <arg type="s" name="namespace"/>
      <arg type="s" name="key"/>
      <arg type="v" name="value"/>
    </signal>
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

answers = []
scheme = 1
changes = []
settings_v1 = False
for arg in sys.argv[1:]:
    if arg.startswith("scheme="):
        scheme = int(arg[len("scheme="):])
    elif arg.startswith("change="):
        value, at = arg[len("change="):].split("@")
        changes.append((int(value), int(at)))
    elif arg == "settings-v1":
        settings_v1 = True
    else:
        answers.append(arg)
loop = GLib.MainLoop()


def log(*a):
    print("fake_portal:", *a, file=sys.stderr, flush=True)


def on_settings(sender, method, params, invocation):
    namespace, key = params[0], params[1]
    if method == "ReadOne" and settings_v1:
        invocation.return_dbus_error(
            "org.freedesktop.DBus.Error.UnknownMethod", "no ReadOne in version 1"
        )
        return
    if (namespace, key) != ("org.freedesktop.appearance", "color-scheme"):
        invocation.return_dbus_error(
            "org.freedesktop.portal.Error.NotFound", "no such setting"
        )
        return
    log(method, namespace, key, "from", sender, "->", scheme)
    value = GLib.Variant("u", scheme)
    if method == "Read":
        # Version 1 put the value in a second variant.
        value = GLib.Variant("v", value)
    invocation.return_value(GLib.Variant("(v)", (value,)))


def change_scheme(conn, value):
    global scheme
    scheme = value
    log("SettingChanged color-scheme ->", value)
    conn.emit_signal(
        None,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        "SettingChanged",
        GLib.Variant(
            "(ssv)",
            ("org.freedesktop.appearance", "color-scheme", GLib.Variant("u", value)),
        ),
    )
    return False


def on_call(conn, sender, path, iface, method, params, invocation):
    if iface == "org.freedesktop.portal.Settings":
        on_settings(sender, method, params, invocation)
        return
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
    if iface == "org.freedesktop.portal.Settings":
        return GLib.Variant("u", 1 if settings_v1 else 2)
    return GLib.Variant("u", 4)


def on_bus(conn, name):
    for info in Gio.DBusNodeInfo.new_for_xml(XML).interfaces:
        conn.register_object(
            "/org/freedesktop/portal/desktop", info, on_call, on_get, None
        )
    for value, at in changes:
        GLib.timeout_add(at, change_scheme, conn, value)


Gio.bus_own_name(
    Gio.BusType.SESSION,
    "org.freedesktop.portal.Desktop",
    Gio.BusNameOwnerFlags.NONE,
    on_bus,
    lambda c, n: log("ready"),
    lambda c, n: (log("name lost"), loop.quit()),
)
loop.run()
