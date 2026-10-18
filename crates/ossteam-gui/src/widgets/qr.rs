// SPDX-License-Identifier: GPL-2.0-or-later
//! A QR code drawn from a login URL (`gui.md` §4.1). The matrix is computed
//! here; nothing is fetched.

use gpui::{App, Bounds, canvas, div, fill, point, prelude::*, px, size};
use qrcode::{Color, EcLevel, QrCode};

/// The modules of `data` as rows of dark (true) and light cells, with the
/// four-module quiet zone the standard asks for.
pub fn matrix(data: &str) -> Option<Vec<Vec<bool>>> {
    const QUIET: usize = 4;
    let code = QrCode::with_error_correction_level(data, EcLevel::M).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let full = width + 2 * QUIET;
    let mut rows = vec![vec![false; full]; full];
    for (i, c) in colors.iter().enumerate() {
        rows[QUIET + i / width][QUIET + i % width] = *c == Color::Dark;
    }
    Some(rows)
}

/// The code of `data` in a square of `side` logical pixels, dark on white
/// whatever the appearance, as scanners expect.
pub fn qr_code(data: &str, side: f32, cx: &App) -> gpui::AnyElement {
    let Some(rows) = matrix(data) else {
        let p = crate::theme::palette(cx);
        return div()
            .text_color(p.error)
            .child("this login URL does not fit in a QR code")
            .into_any_element();
    };
    div()
        .size(px(side))
        .bg(gpui::white())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<gpui::Pixels>, (), window, _| {
                    let n = rows.len() as f32;
                    let cell = bounds.size.width / n;
                    for (y, row) in rows.iter().enumerate() {
                        for (x, &dark) in row.iter().enumerate() {
                            if dark {
                                let origin = point(
                                    bounds.origin.x + cell * x as f32,
                                    bounds.origin.y + cell * y as f32,
                                );
                                window.paint_quad(fill(
                                    Bounds::new(origin, size(cell, cell)),
                                    gpui::black(),
                                ));
                            }
                        }
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::matrix;

    #[test]
    fn a_login_url_has_a_square_matrix_with_a_quiet_zone() {
        let m = matrix("https://s.team/q/1/1234567890123456789").unwrap();
        assert!(m.iter().all(|row| row.len() == m.len()));
        assert!(m[0].iter().all(|&c| !c));
        // The top left finder pattern starts after the quiet zone.
        assert!(m[4][4] && m[4][10] && m[10][4]);
        assert!(!m[5][5]);
    }
}
