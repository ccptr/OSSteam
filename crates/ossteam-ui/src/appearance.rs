// SPDX-License-Identifier: GPL-2.0-or-later
//! The system's light or dark appearance (GUI-4, gui.md §6). On Linux it
//! is asked of the desktop portal as the process starts, so the first
//! frame already has it; elsewhere winit reports it with the window.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl From<winit::window::Theme> for Appearance {
    fn from(theme: winit::window::Theme) -> Self {
        match theme {
            winit::window::Theme::Light => Appearance::Light,
            winit::window::Theme::Dark => Appearance::Dark,
        }
    }
}

/// The portal's answer, asked for on a thread started with the process.
pub struct StartupAppearance(Option<mpsc::Receiver<Option<Appearance>>>);

impl StartupAppearance {
    /// Starts the query; other platforms report the appearance with the
    /// window, so there is nothing to ask.
    pub fn query() -> Self {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let (sender, receiver) = mpsc::sync_channel(1);
            let spawned = std::thread::Builder::new()
                .name("appearance".into())
                .spawn(move || {
                    let _ = sender.send(portal_appearance());
                });
            if spawned.is_ok() {
                return Self(Some(receiver));
            }
        }
        Self(None)
    }

    /// The answer, if it comes within `timeout`.
    pub fn wait(self, timeout: Duration) -> Option<Appearance> {
        self.0?.recv_timeout(timeout).ok().flatten()
    }
}

/// `org.freedesktop.appearance color-scheme`; no preference is light.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn portal_appearance() -> Option<Appearance> {
    use ashpd::desktop::settings::{ColorScheme, Settings};
    let scheme = async_io::block_on(async {
        let bus = match session_bus_socket() {
            Some(socket) => {
                ashpd::zbus::connection::Builder::async_io_unix_stream(socket)
                    .build()
                    .await?
            }
            None => ashpd::zbus::Connection::session().await?,
        };
        Settings::with_connection(bus).await?.color_scheme().await
    })
    .ok()?;
    Some(match scheme {
        ColorScheme::PreferDark => Appearance::Dark,
        ColorScheme::PreferLight | ColorScheme::NoPreference => Appearance::Light,
    })
}

/// The session bus's socket, connected here rather than by zbus: zbus
/// connects on the `blocking` crate's thread pool, whose last thread then
/// wakes every 500 ms for the life of the process (GUI-5). `None` for an
/// address this does not handle, which zbus then connects.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn session_bus_socket() -> Option<std::os::unix::net::UnixStream> {
    use std::os::unix::net::UnixStream;
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
            let Some(value) = unescape_address(value) else {
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

/// Undoes a D-Bus address value's `%XX` escapes.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn unescape_address(value: &str) -> Option<Vec<u8>> {
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

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    #[test]
    fn address_unescapes() {
        assert_eq!(
            super::unescape_address("/run/user/1000/a%2cb").as_deref(),
            Some(&b"/run/user/1000/a,b"[..])
        );
        assert_eq!(super::unescape_address("%2"), None);
    }
}
