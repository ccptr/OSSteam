// SPDX-License-Identifier: GPL-2.0-or-later
//! Colours for the system light and dark appearances (SPEC GUI-4). The
//! root view keeps the palette for the window's appearance in a global, so
//! widgets read it with [`palette`].

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
