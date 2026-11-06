// SPDX-License-Identifier: LGPL-2.1-or-later
//! The desktop portal (`org.freedesktop.portal.Desktop`) through our own
//! D-Bus client (`ossteam-dbus`, ADR 0004): the appearance setting and its
//! change signal, and the file chooser. Each call blocks the thread that
//! makes it, and waits in a read that does not wake until the bus
//! sends something (GUI-5).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use ossteam_dbus::{self as dbus, Connection, Message, Value};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const FILE_CHOOSER: &str = "org.freedesktop.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.portal.Request";
const APPEARANCE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME: &str = "color-scheme";

/// `org.freedesktop.appearance color-scheme`: 0 no preference, 1 prefer
/// dark, 2 prefer light.
pub fn color_scheme(bus: &mut Connection) -> dbus::Result<u32> {
    let read = |member| {
        Message::call(
            DESKTOP,
            DESKTOP_PATH,
            SETTINGS,
            member,
            vec![
                Value::Str(APPEARANCE.into()),
                Value::Str(COLOR_SCHEME.into()),
            ],
        )
    };
    let reply = match bus.call(read("ReadOne")) {
        // Settings version 1 has only `Read`, whose value is in a second
        // variant.
        Err(dbus::Error::Remote(name, _)) if name == dbus::UNKNOWN_METHOD => {
            bus.call(read("Read"))?
        }
        r => r?,
    };
    match reply.body.first().map(Value::unwrap) {
        Some(&Value::U32(scheme)) => Ok(scheme),
        _ => Err(dbus::Error::Unexpected(
            "the portal's color-scheme is not a number",
        )),
    }
}

/// Calls `changed` with each new `color-scheme` until the bus goes away,
/// which is the error returned.
pub fn watch_color_scheme(bus: &mut Connection, mut changed: impl FnMut(u32)) -> dbus::Error {
    let rule = format!(
        "type='signal',sender='{DESKTOP}',path='{DESKTOP_PATH}',interface='{SETTINGS}',\
         member='SettingChanged',arg0='{APPEARANCE}',arg1='{COLOR_SCHEME}'"
    );
    if let Err(e) = bus.add_match(&rule) {
        return e;
    }
    loop {
        let m = match bus.next_signal() {
            Ok(m) => m,
            Err(e) => return e,
        };
        if !m.is_signal(DESKTOP_PATH, SETTINGS, "SettingChanged") {
            continue;
        }
        if let [Value::Str(namespace), Value::Str(key), value] = &m.body[..]
            && namespace == APPEARANCE
            && key == COLOR_SCHEME
            && let Value::U32(scheme) = value.unwrap()
        {
            changed(*scheme);
        }
    }
}

/// Asks the user for one folder with the portal's file chooser; the URI
/// it answers, or `None` if they cancel.
///
/// The answer comes as a `Response` signal on a request object whose path
/// we choose with `handle_token`. The match for it is added before the
/// call, so that an answer cannot come before we listen.
pub fn pick_folder(bus: &mut Connection, title: &str) -> dbus::Result<Option<String>> {
    static TOKENS: AtomicU32 = AtomicU32::new(0);
    let token = format!("ossteam{}", TOKENS.fetch_add(1, Ordering::Relaxed));
    let sender = bus.unique_name().trim_start_matches(':').replace('.', "_");
    let expected = format!("{DESKTOP_PATH}/request/{sender}/{token}");
    let rule = |path: &str| {
        format!(
            "type='signal',sender='{DESKTOP}',path='{path}',interface='{REQUEST}',member='Response'"
        )
    };
    bus.add_match(&rule(&expected))?;

    let options = Value::dict([
        ("handle_token", Value::Str(token)),
        ("directory", Value::Bool(true)),
        ("multiple", Value::Bool(false)),
    ]);
    let reply = bus.call(Message::call(
        DESKTOP,
        DESKTOP_PATH,
        FILE_CHOOSER,
        "OpenFile",
        vec![Value::Str(String::new()), Value::Str(title.into()), options],
    ))?;
    let Some(Value::Path(handle)) = reply.body.first() else {
        return Err(dbus::Error::Unexpected(
            "the file chooser did not answer a request",
        ));
    };
    // Portals before 0.9 choose the path themselves.
    if *handle != expected {
        bus.add_match(&rule(handle))?;
    }

    loop {
        let m = bus.next_signal()?;
        if !m.is_signal(handle, REQUEST, "Response") {
            continue;
        }
        return match &m.body[..] {
            [Value::U32(0), results, ..] => Ok(match results.get("uris") {
                Some(Value::Array(_, uris)) => {
                    uris.first().and_then(Value::as_str).map(str::to_owned)
                }
                _ => None,
            }),
            [Value::U32(1), ..] => Ok(None),
            _ => Err(dbus::Error::Unexpected(
                "the file chooser ended without an answer",
            )),
        };
    }
}

/// The local path a `file://` URI names, as the portal's file chooser
/// returns it (`file:///home/a%20b`). `None` for any other URI.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri.strip_prefix("file://")?;
    // An empty host or `localhost`; the path starts at the next `/`.
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if &rest[..i] == "localhost" => &rest[i..],
        _ => return None,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        dbus::percent_decode(path)?,
    )))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;
    use ossteam_dbus::test_bus::{self, error, reply, signal};
    use ossteam_dbus::{Kind, Type, UNKNOWN_METHOD};

    fn member(m: &Message) -> &str {
        m.member.as_deref().unwrap_or_default()
    }

    #[test]
    fn color_scheme_falls_back_to_read() {
        let (mut bus, _t) = test_bus::connect(|m, out| {
            out.push(match member(m) {
                "ReadOne" => error(m, UNKNOWN_METHOD),
                _ => {
                    assert_eq!(
                        m.body,
                        [
                            Value::Str(APPEARANCE.into()),
                            Value::Str(COLOR_SCHEME.into())
                        ]
                    );
                    let v = |v| Value::Variant(Box::new(v));
                    reply(m, vec![v(v(Value::U32(1)))])
                }
            });
            true
        });
        assert_eq!(color_scheme(&mut bus).unwrap(), 1);
    }

    #[test]
    fn color_scheme_errors_pass_on() {
        let (mut bus, _t) = test_bus::connect(|m, out| {
            out.push(error(m, "org.freedesktop.portal.Error.NotFound"));
            true
        });
        assert!(
            matches!(color_scheme(&mut bus), Err(dbus::Error::Remote(n, _)) if n.ends_with("NotFound"))
        );
    }

    #[test]
    fn watch_reports_only_the_color_scheme() {
        let (mut bus, t) = test_bus::connect(|m, out| {
            assert_eq!(member(m), "AddMatch");
            out.push(reply(m, vec![]));
            let changed = |ns: &str, key: &str, v| {
                let body = vec![
                    Value::Str(ns.into()),
                    Value::Str(key.into()),
                    Value::Variant(Box::new(v)),
                ];
                signal(DESKTOP_PATH, SETTINGS, "SettingChanged", body)
            };
            out.extend([
                changed(APPEARANCE, "accent-color", Value::U32(9)),
                changed("org.gnome.desktop.interface", COLOR_SCHEME, Value::U32(9)),
                changed(APPEARANCE, COLOR_SCHEME, Value::U32(2)),
                changed(APPEARANCE, COLOR_SCHEME, Value::Str("dark".into())),
                changed(APPEARANCE, COLOR_SCHEME, Value::U32(1)),
            ]);
            false
        });
        let mut seen = Vec::new();
        let end = watch_color_scheme(&mut bus, |s| seen.push(s));
        t.join().unwrap();
        assert_eq!(seen, [2, 1]);
        assert!(matches!(end, dbus::Error::Io(_)), "{end:?}");
    }

    /// A file chooser that answers `response` and `results`, with the
    /// signal before the method's reply when `early`; the messages it saw.
    fn chooser(
        response: u32,
        results: Value,
        early: bool,
    ) -> (Connection, Arc<Mutex<Vec<Message>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let (bus, _t) = test_bus::connect(move |m, out| {
            log.lock().unwrap().push(m.clone());
            if member(m) != "OpenFile" {
                out.push(reply(m, vec![]));
                return true;
            }
            let token = m.body[2]
                .get("handle_token")
                .and_then(Value::as_str)
                .unwrap();
            let path = format!("{DESKTOP_PATH}/request/1_42/{token}");
            let answer = signal(
                &path,
                REQUEST,
                "Response",
                vec![Value::U32(response), results.clone()],
            );
            let other = signal(
                &format!("{DESKTOP_PATH}/request/1_42/x"),
                REQUEST,
                "Response",
                vec![Value::U32(0)],
            );
            let returned = reply(m, vec![Value::Path(path)]);
            if early {
                out.extend([other, answer, returned]);
            } else {
                out.extend([returned, other, answer]);
            }
            true
        });
        (bus, seen)
    }

    fn uris(list: &[&str]) -> Value {
        let list = list.iter().map(|u| Value::Str((*u).into())).collect();
        Value::dict([("uris", Value::Array(Type::Str, list))])
    }

    #[test]
    fn a_picked_folder() {
        for early in [false, true] {
            let (mut bus, seen) = chooser(0, uris(&["file:///mnt/a%20b"]), early);
            assert_eq!(
                pick_folder(&mut bus, "Add").unwrap().as_deref(),
                Some("file:///mnt/a%20b")
            );
            let seen = seen.lock().unwrap();
            // The match first, then the call with its options.
            assert_eq!(
                seen.iter().map(member).collect::<Vec<_>>(),
                ["AddMatch", "OpenFile"]
            );
            let rule = seen[0].body[0].as_str().unwrap();
            assert!(
                rule.contains("path='/org/freedesktop/portal/desktop/request/1_42/ossteam"),
                "{rule}"
            );
            let open = &seen[1];
            assert_eq!(
                (open.kind, open.destination.as_deref()),
                (Kind::Call, Some(DESKTOP))
            );
            assert_eq!(open.body[1], Value::Str("Add".into()));
            assert_eq!(open.body[2].get("directory"), Some(&Value::Bool(true)));
            assert_eq!(open.body[2].get("multiple"), Some(&Value::Bool(false)));
        }
    }

    #[test]
    fn cancelled_or_empty_is_none() {
        let (mut bus, _) = chooser(1, Value::dict([]), false);
        assert_eq!(pick_folder(&mut bus, "Add").unwrap(), None);
        let (mut bus, _) = chooser(0, uris(&[]), false);
        assert_eq!(pick_folder(&mut bus, "Add").unwrap(), None);
        let (mut bus, _) = chooser(2, Value::dict([]), false);
        assert!(pick_folder(&mut bus, "Add").is_err());
    }

    #[test]
    fn a_request_path_the_portal_chose() {
        let (mut bus, _t) = test_bus::connect(|m, out| {
            if member(m) == "OpenFile" {
                let path = "/org/freedesktop/portal/desktop/request/old/1";
                out.push(reply(m, vec![Value::Path(path.into())]));
                out.push(signal(
                    path,
                    REQUEST,
                    "Response",
                    vec![Value::U32(0), uris(&["file:///x"])],
                ));
            } else {
                out.push(reply(m, vec![]));
            }
            true
        });
        assert_eq!(
            pick_folder(&mut bus, "Add").unwrap().as_deref(),
            Some("file:///x")
        );
    }

    #[test]
    fn file_uris() {
        let path = |uri| file_uri_path(uri);
        assert_eq!(
            path("file:///home/a%20b/Spiele").as_deref(),
            Some(Path::new("/home/a b/Spiele"))
        );
        assert_eq!(
            path("file://localhost/mnt/games").as_deref(),
            Some(Path::new("/mnt/games"))
        );
        assert_eq!(
            path("file:///mnt/%C3%A9t%C3%A9").as_deref(),
            Some(Path::new("/mnt/été"))
        );
        assert_eq!(path("file://server/share"), None);
        assert_eq!(path("https://example.org/"), None);
    }
}
