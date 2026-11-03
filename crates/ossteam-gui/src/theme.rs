// SPDX-License-Identifier: GPL-2.0-or-later
//! Colours for the system light and dark appearances (SPEC GUI-4). The
//! toolkit learns the appearance before the first frame and follows its
//! changes; views take the palette for it from [`Palette::for_view`].

use ossteam_ui::appearance::Appearance;
use ossteam_ui::scene::Color;
use ossteam_ui::window::ViewCx;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub focus_ring: Color,
    pub selection: Color,
    pub error: Color,
    pub warning: Color,
}

impl Palette {
    /// The palette for the window's appearance; light when the system
    /// has not said.
    pub fn for_view(cx: &ViewCx) -> Self {
        match cx.appearance {
            Some(Appearance::Dark) => Self::dark(),
            Some(Appearance::Light) | None => Self::light(),
        }
    }

    pub const fn light() -> Self {
        Self {
            background: Color::rgb(0xf6f7f9),
            surface: Color::rgb(0xffffff),
            surface_raised: Color::rgb(0xeceff3),
            border: Color::rgb(0xd5d9e0),
            text: Color::rgb(0x1b1f24),
            muted: Color::rgb(0x5b6470),
            accent: Color::rgb(0x2f6fdf),
            on_accent: Color::rgb(0xffffff),
            focus_ring: Color::rgb(0x1d4fb0),
            selection: Color::rgb(0xd6e4ff),
            error: Color::rgb(0xb42318),
            warning: Color::rgb(0x9a5b00),
        }
    }

    pub const fn dark() -> Self {
        Self {
            background: Color::rgb(0x16181c),
            surface: Color::rgb(0x1f2228),
            surface_raised: Color::rgb(0x2a2e36),
            border: Color::rgb(0x343943),
            text: Color::rgb(0xe8eaed),
            muted: Color::rgb(0x9aa3ad),
            accent: Color::rgb(0x5b8def),
            on_accent: Color::rgb(0x0d1117),
            focus_ring: Color::rgb(0xa8c4ff),
            selection: Color::rgb(0x23395e),
            error: Color::rgb(0xff7b72),
            warning: Color::rgb(0xe3b341),
        }
    }
}
