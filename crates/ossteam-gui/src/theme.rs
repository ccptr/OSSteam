// SPDX-License-Identifier: GPL-2.0-or-later
//! Colours for the system light and dark appearances (SPEC GUI-4). The
//! root view keeps the palette for the window's appearance in a global, so
//! widgets read it with [`palette`].

use std::time::Duration;

use gpui::{App, Global, Hsla, WindowAppearance, rgb};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_raised: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub focus_ring: Hsla,
    pub selection: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
}

impl Global for Palette {}

/// The palette of the window being drawn.
pub fn palette(cx: &App) -> Palette {
    cx.try_global::<Palette>()
        .copied()
        .unwrap_or_else(Palette::light)
}

impl Palette {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    fn light() -> Self {
        Self {
            background: rgb(0xf6f7f9).into(),
            surface: rgb(0xffffff).into(),
            surface_raised: rgb(0xeceff3).into(),
            border: rgb(0xd5d9e0).into(),
            text: rgb(0x1b1f24).into(),
            muted: rgb(0x5b6470).into(),
            accent: rgb(0x2f6fdf).into(),
            on_accent: rgb(0xffffff).into(),
            focus_ring: rgb(0x1d4fb0).into(),
            selection: rgb(0xd6e4ff).into(),
            error: rgb(0xb42318).into(),
            warning: rgb(0x9a5b00).into(),
        }
    }

    fn dark() -> Self {
        Self {
            background: rgb(0x16181c).into(),
            surface: rgb(0x1f2228).into(),
            surface_raised: rgb(0x2a2e36).into(),
            border: rgb(0x343943).into(),
            text: rgb(0xe8eaed).into(),
            muted: rgb(0x9aa3ad).into(),
            accent: rgb(0x5b8def).into(),
            on_accent: rgb(0x0d1117).into(),
            focus_ring: rgb(0xa8c4ff).into(),
            selection: rgb(0x23395e).into(),
            error: rgb(0xff7b72).into(),
            warning: rgb(0xe3b341).into(),
        }
    }
}

/// How long the window waits for [`StartupAppearance`] before its first
/// frame. The query starts with the process and normally answers before
/// gpui has opened the window, so this is only reached without a portal.
const STARTUP_APPEARANCE_WAIT: Duration = Duration::from_millis(50);

/// The system appearance, asked of the desktop portal as the process
/// starts. On Linux gpui reports `Light` until its own portal query
/// answers, which is after the first frame, so a dark desktop would see the
/// window flash light (m0.md, GUI-4). The root view uses this answer until
/// gpui reports an appearance of its own.
pub struct StartupAppearance(Option<crossbeam_channel::Receiver<WindowAppearance>>);

impl StartupAppearance {
    /// Starts the query; other platforms report the appearance at once.
    pub fn query() -> Self {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let (sender, receiver) = crossbeam_channel::bounded(1);
            let spawned = std::thread::Builder::new()
                .name("appearance".into())
                .spawn(move || {
                    if let Some(appearance) = portal_appearance() {
                        let _ = sender.send(appearance);
                    }
                });
            if spawned.is_ok() {
                return Self(Some(receiver));
            }
        }
        Self(None)
    }

    /// The answer, if it comes within [`STARTUP_APPEARANCE_WAIT`].
    pub fn wait(self) -> Option<WindowAppearance> {
        self.0?.recv_timeout(STARTUP_APPEARANCE_WAIT).ok()
    }
}

/// `org.freedesktop.appearance color-scheme`, mapped as gpui maps it.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn portal_appearance() -> Option<WindowAppearance> {
    use ashpd::desktop::settings::{ColorScheme, Settings};
    let scheme = async_io::block_on(async { Settings::new().await?.color_scheme().await }).ok()?;
    Some(match scheme {
        ColorScheme::PreferDark => WindowAppearance::Dark,
        ColorScheme::PreferLight | ColorScheme::NoPreference => WindowAppearance::Light,
    })
}
