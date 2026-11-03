// SPDX-License-Identifier: GPL-2.0-or-later
//! The folder picker (ADR 0004): the desktop portal's file chooser
//! through our own D-Bus client on Linux (`portal.rs`), and `rfd`'s
//! dialogs on Windows and macOS.
//!
//! [`pick_folder`] blocks until the user picks or cancels. Where it may
//! run:
//!
//! - **Linux and Windows:** on a thread of its own, with the answer sent
//!   back to the event loop. The window must keep drawing and answering
//!   the compositor meanwhile: on Linux the dialog is another process's.
//! - **macOS:** on the main thread. AppKit's panels run only there, and
//!   the panel's modal loop keeps the application's events flowing.

use std::fmt;
use std::path::PathBuf;

/// The picker could not be shown, or its answer was not a local folder.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Asks the user for one folder; `Ok(None)` if they cancel. `title` is
/// the dialog's title, such as "Add library folder".
pub fn pick_folder(title: &str) -> Result<Option<PathBuf>, Error> {
    imp::pick_folder(title)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod imp {
    use std::path::PathBuf;

    use super::Error;

    pub fn pick_folder(title: &str) -> Result<Option<PathBuf>, Error> {
        let picked = crate::dbus::Connection::session()
            .and_then(|mut bus| crate::portal::pick_folder(&mut bus, title));
        let uri = match picked {
            Ok(Some(uri)) => uri,
            Ok(None) => return Ok(None),
            Err(e) => return Err(Error(format!("the desktop portal's file chooser: {e}"))),
        };
        crate::portal::file_uri_path(&uri).map(Some).ok_or_else(|| {
            Error(format!(
                "the file chooser answered {uri}, not a local folder"
            ))
        })
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use std::path::PathBuf;

    use super::Error;

    pub fn pick_folder(title: &str) -> Result<Option<PathBuf>, Error> {
        Ok(rfd::FileDialog::new().set_title(title).pick_folder())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    windows,
    target_os = "macos"
)))]
mod imp {
    use std::path::PathBuf;

    use super::Error;

    pub fn pick_folder(_: &str) -> Result<Option<PathBuf>, Error> {
        Err(Error("no folder picker on this platform".into()))
    }
}
