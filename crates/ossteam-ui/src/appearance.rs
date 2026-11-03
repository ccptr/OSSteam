// SPDX-License-Identifier: GPL-2.0-or-later
//! The system's light or dark appearance (GUI-4, gui.md §6). On Linux it
//! is asked of the desktop portal as the process starts, so the first
//! frame already has it; elsewhere winit reports it with the window.

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

/// Calls `changed` on a thread of its own whenever the desktop's
/// appearance changes, as the portal reports it (Linux). Elsewhere winit
/// reports changes with the window, so this does nothing. The thread
/// waits on the bus socket and does not wake otherwise (GUI-5).
pub fn watch(changed: impl Fn(Appearance) + Send + 'static) {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let spawned = std::thread::Builder::new()
            .name("appearance".into())
            .spawn(move || {
                let end = match ossteam_dbus::Connection::session() {
                    Ok(mut bus) => crate::portal::watch_color_scheme(&mut bus, |scheme| {
                        changed(from_scheme(scheme));
                    }),
                    Err(e) => e,
                };
                tracing::info!("not watching the appearance: {end}");
            });
        if let Err(e) = spawned {
            tracing::warn!("starting the appearance thread: {e}");
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = changed;
}

/// The portal's `color-scheme`: 1 is prefer dark; no preference (0) and
/// prefer light (2) are light, as is anything newer.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn from_scheme(scheme: u32) -> Appearance {
    match scheme {
        1 => Appearance::Dark,
        _ => Appearance::Light,
    }
}

/// `org.freedesktop.appearance color-scheme`; no preference is light.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn portal_appearance() -> Option<Appearance> {
    let mut bus = ossteam_dbus::Connection::session().ok()?;
    crate::portal::color_scheme(&mut bus).ok().map(from_scheme)
}
