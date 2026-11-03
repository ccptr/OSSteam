// SPDX-License-Identifier: GPL-2.0-or-later
//! The session bus connection for desktop portal calls (appearance, the
//! folder picker). Every portal call made with ashpd must use it.
//!
//! Connecting the bus socket here rather than in zbus matters: zbus
//! connects on the `blocking` crate's thread pool, whose last thread then
//! wakes every 500 ms for the life of the process (GUI-5).

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use ashpd::zbus;

/// A new connection to the session bus.
pub(crate) async fn session_bus() -> zbus::Result<zbus::Connection> {
    match session_bus_socket() {
        Some(socket) => {
            zbus::connection::Builder::async_io_unix_stream(socket)
                .build()
                .await
        }
        // An address this does not handle: zbus connects it.
        None => zbus::Connection::session().await,
    }
}

/// The session bus's socket, from `DBUS_SESSION_BUS_ADDRESS` or else
/// `$XDG_RUNTIME_DIR/bus`.
fn session_bus_socket() -> Option<UnixStream> {
    let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
        return UnixStream::connect(std::path::Path::new(&dir).join("bus")).ok();
    };
    // `unix:path=/run/user/1000/bus,guid=…`, possibly several separated by `;`.
    for entry in address.split(';') {
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        for param in params.split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            let Some(value) = percent_decode(value) else {
                continue;
            };
            let socket = match key {
                "path" => UnixStream::connect(std::ffi::OsStr::from_bytes(&value)),
                #[cfg(target_os = "linux")]
                "abstract" => {
                    use std::os::linux::net::SocketAddrExt;
                    std::os::unix::net::SocketAddr::from_abstract_name(&value)
                        .and_then(|addr| UnixStream::connect_addr(&addr))
                }
                _ => continue,
            };
            if let Ok(socket) = socket {
                return Some(socket);
            }
        }
    }
    None
}

/// The local path a `file://` URI names, as the portal's file chooser
/// returns it (`file:///home/a%20b`). `None` for any other URI.
pub(crate) fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // An empty host or `localhost`; the path starts at the next `/`.
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if &rest[..i] == "localhost" => &rest[i..],
        _ => return None,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    Some(PathBuf::from(std::ffi::OsString::from_vec(percent_decode(
        path,
    )?)))
}

/// Undoes `%XX` escapes (D-Bus addresses, URIs).
fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn percent_decodes() {
        assert_eq!(
            super::percent_decode("/run/user/1000/a%2cb").as_deref(),
            Some(&b"/run/user/1000/a,b"[..])
        );
        assert_eq!(super::percent_decode("%2"), None);
    }

    #[test]
    fn file_uris() {
        let path = |uri| super::file_uri_path(uri);
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
