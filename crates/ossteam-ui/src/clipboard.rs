// SPDX-License-Identifier: LGPL-2.1-or-later
//! The system clipboard, for text (ADR 0004): `smithay-clipboard` on
//! Wayland, on winit's own connection, and `arboard` on X11, Windows and
//! macOS.
//!
//! Both are synchronous: reading waits for the application that owns the
//! clipboard to send its text. On Wayland a client may take the clipboard
//! only with the serial of a recent key or pointer press (m0.md item 1);
//! `smithay-clipboard` follows the seat itself, so it must be created
//! before the press that copies.

use raw_window_handle::HasDisplayHandle;

use crate::text_field::TextClipboard;

pub struct Clipboard {
    inner: Inner,
}

enum Inner {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    Wayland(smithay_clipboard::Clipboard),
    Arboard(arboard::Clipboard),
    /// No clipboard could be opened: copying does nothing and pasting
    /// finds nothing.
    Unavailable,
}

impl Clipboard {
    /// Opens the clipboard of the display `window` is on.
    ///
    /// On Wayland the clipboard uses the window's connection, so it must
    /// be dropped before the event loop is.
    pub fn new(window: &impl HasDisplayHandle) -> Self {
        let display = window.display_handle().map(|h| h.as_raw());
        let inner = match display {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Ok(raw_window_handle::RawDisplayHandle::Wayland(h)) => {
                // SAFETY: the display is winit's, which outlives the
                // clipboard as this function's documentation requires.
                Inner::Wayland(unsafe { smithay_clipboard::Clipboard::new(h.display.as_ptr()) })
            }
            _ => match arboard::Clipboard::new() {
                Ok(c) => Inner::Arboard(c),
                Err(e) => {
                    tracing::warn!("no clipboard: {e}");
                    Inner::Unavailable
                }
            },
        };
        Self { inner }
    }

    /// Which clipboard this is: `wayland`, `arboard` or `unavailable`.
    pub fn kind(&self) -> &'static str {
        match self.inner {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Inner::Wayland(_) => "wayland",
            Inner::Arboard(_) => "arboard",
            Inner::Unavailable => "unavailable",
        }
    }
}

impl TextClipboard for Clipboard {
    fn read_text(&mut self) -> Option<String> {
        let read: Result<String, String> = match &mut self.inner {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Inner::Wayland(c) => c.load().map_err(|e| e.to_string()),
            Inner::Arboard(c) => c.get_text().map_err(|e| e.to_string()),
            Inner::Unavailable => Err("no clipboard".into()),
        };
        read.map_err(|e| tracing::debug!("clipboard empty or unreadable: {e}"))
            .ok()
    }

    fn write_text(&mut self, text: &str) {
        match &mut self.inner {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            Inner::Wayland(c) => c.store(text),
            Inner::Arboard(c) => {
                if let Err(e) = c.set_text(text) {
                    tracing::warn!("copying to the clipboard: {e}");
                }
            }
            Inner::Unavailable => {
                let _ = text;
            }
        }
    }
}
