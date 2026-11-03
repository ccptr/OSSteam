// SPDX-License-Identifier: GPL-2.0-or-later
//! The coverage math both renderers use. `shaders/quad.slang` is the same
//! code in Slang; keep the two in step, or the renderers disagree.

/// The signed distance from `(px, py)`, relative to the rectangle's
/// centre, to a rounded rectangle of half size `(hw, hh)` and corner
/// radius `r`: negative inside.
#[inline]
pub fn rounded_rect_distance(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let r = r.min(hw).min(hh).max(0.0);
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// The error function, to within 1.5e-7 (Abramowitz and Stegun 7.1.26).
#[inline]
pub fn erf(x: f32) -> f32 {
    let s = x.signum();
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * a);
    let y = 1.0
        - (((((1.061_405_4 * t - 1.453_152) * t) + 1.421_413_7) * t - 0.284_496_74) * t
            + 0.254_829_6)
            * t
            * (-a * a).exp();
    s * y
}

/// A quad's colour at one pixel, premultiplied, as `[r, g, b, a]` in 0..1.
///
/// `(px, py)` is the pixel centre relative to the quad's centre.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn quad_pixel(
    px: f32,
    py: f32,
    hw: f32,
    hh: f32,
    radius: f32,
    border_width: f32,
    blur: f32,
    background: [f32; 4],
    border: [f32; 4],
) -> [f32; 4] {
    let d = rounded_rect_distance(px, py, hw, hh, radius);
    if blur > 0.0 {
        let a = background[3] * 0.5 * (1.0 - erf(d / (std::f32::consts::SQRT_2 * blur)));
        return [background[0] * a, background[1] * a, background[2] * a, a];
    }
    let outer = (0.5 - d).clamp(0.0, 1.0);
    if outer <= 0.0 {
        return [0.0; 4];
    }
    let inner = if border_width > 0.0 {
        let di = rounded_rect_distance(
            px,
            py,
            hw - border_width,
            hh - border_width,
            radius - border_width,
        );
        (0.5 - di).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Premultiply each, then mix border into background by `inner`.
    let mut out = [0.0; 4];
    let ba = background[3];
    let ea = border[3];
    for i in 0..3 {
        out[i] = (background[i] * ba * inner + border[i] * ea * (1.0 - inner)) * outer;
    }
    out[3] = (ba * inner + ea * (1.0 - inner)) * outer;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_is_negative_inside_and_zero_on_the_edge() {
        assert!(rounded_rect_distance(0.0, 0.0, 10.0, 5.0, 2.0) < 0.0);
        assert!(rounded_rect_distance(10.0, 0.0, 10.0, 5.0, 2.0).abs() < 1e-5);
        // A corner is cut by the radius.
        assert!(rounded_rect_distance(10.0, 5.0, 10.0, 5.0, 2.0) > 0.5);
    }

    #[test]
    fn erf_matches_known_values() {
        assert!(erf(0.0).abs() < 1e-6);
        assert!((erf(1.0) - 0.842_700_8).abs() < 1e-5);
        assert!((erf(-2.0) + 0.995_322_3).abs() < 1e-5);
    }

    #[test]
    fn a_quad_is_opaque_inside_and_half_covered_on_its_edge() {
        let c = [1.0, 0.0, 0.0, 1.0];
        let inside = quad_pixel(0.0, 0.0, 10.0, 10.0, 0.0, 0.0, 0.0, c, c);
        assert_eq!(inside, [1.0, 0.0, 0.0, 1.0]);
        let edge = quad_pixel(10.0, 0.0, 10.0, 10.0, 0.0, 0.0, 0.0, c, c);
        assert!((edge[3] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_border_shows_its_own_colour() {
        let bg = [1.0, 1.0, 1.0, 1.0];
        let border = [0.0, 0.0, 1.0, 1.0];
        let p = quad_pixel(-9.5, 0.0, 10.0, 10.0, 0.0, 2.0, 0.0, bg, border);
        assert_eq!(p, [0.0, 0.0, 1.0, 1.0]);
    }
}
