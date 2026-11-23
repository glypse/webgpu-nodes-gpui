use super::{ColorSpace, Rgb};

/// A 3x3 matrix, row by row.
pub type Mat3 = [[f64; 3]; 3];

// Between linear sRGB and linear Display P3, both D65: CSS Color 4's
// rational matrices through XYZ, multiplied out. `palette` derives Display P3
// from the primaries and differs from these by about 3e-5, so they stay.
pub const SRGB_TO_DISPLAY_P3: Mat3 = [
    [0.822461969, 0.177538031, 0.0],
    [0.033194199, 0.966805801, 0.0],
    [0.017082631, 0.072397441, 0.910519929],
];
pub const DISPLAY_P3_TO_SRGB: Mat3 = [
    [1.224940176, -0.224940176, 0.0],
    [-0.042056955, 1.042056955, 0.0],
    [-0.019637555, -0.078636046, 1.0982736],
];

/// `m` as a WGSL `mat3x3f`, whose constructor takes columns.
pub fn wgsl_mat3(m: &Mat3) -> String {
    let column = |i: usize| {
        let parts: Vec<String> = m.iter().map(|row| format!("{:.9}", row[i])).collect();
        format!("vec3f({})", parts.join(", "))
    };
    format!("mat3x3f({}, {}, {})", column(0), column(1), column(2))
}

pub fn apply_mat3(m: &Mat3, Rgb { r, g, b }: Rgb) -> Rgb {
    let row = |i: usize| m[i][0] * r + m[i][1] * g + m[i][2] * b;
    Rgb::new(row(0), row(1), row(2))
}

/// A linear sRGB colour as linear RGB of `space`'s primaries.
pub fn to_gamut(rgb: Rgb, space: ColorSpace) -> Rgb {
    match space {
        ColorSpace::Srgb => rgb,
        ColorSpace::DisplayP3 => apply_mat3(&SRGB_TO_DISPLAY_P3, rgb),
    }
}

/// A linear RGB colour of `space`'s primaries as linear sRGB.
pub fn from_gamut(rgb: Rgb, space: ColorSpace) -> Rgb {
    match space {
        ColorSpace::Srgb => rgb,
        ColorSpace::DisplayP3 => apply_mat3(&DISPLAY_P3_TO_SRGB, rgb),
    }
}

fn clip(Rgb { r, g, b }: Rgb) -> Rgb {
    Rgb::new(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

/// A linear sRGB colour with each channel clamped to `space`'s gamut, still in
/// linear sRGB: components outside [0, 1] are what Display P3 adds to sRGB.
pub fn clip_to_gamut(rgb: Rgb, space: ColorSpace) -> Rgb {
    from_gamut(clip(to_gamut(rgb, space)), space)
}

/// Whether a linear sRGB colour is within `space`'s gamut, up to rounding.
pub fn in_gamut(rgb: Rgb, space: ColorSpace) -> bool {
    const EPSILON: f64 = 1e-5;
    let Rgb { r, g, b } = to_gamut(rgb, space);
    [r, g, b]
        .iter()
        .all(|n| *n >= -EPSILON && *n <= 1.0 + EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gamut_matrices_are_inverses() {
        let rgb = Rgb::new(0.2, 0.5, 0.9);
        let back = from_gamut(to_gamut(rgb, ColorSpace::DisplayP3), ColorSpace::DisplayP3);
        assert!(
            (back.r - rgb.r).abs() < 1e-6
                && (back.g - rgb.g).abs() < 1e-6
                && (back.b - rgb.b).abs() < 1e-6
        );
    }

    #[test]
    fn display_p3_holds_colours_past_srgb() {
        let past = Rgb::new(-0.1, 0.8, 0.2);
        assert!(!in_gamut(past, ColorSpace::Srgb));
        let p3 = from_gamut(Rgb::new(0.1, 0.8, 0.2), ColorSpace::DisplayP3);
        assert!(in_gamut(p3, ColorSpace::DisplayP3));
        assert!(clip_to_gamut(past, ColorSpace::Srgb).r == 0.0);
    }

    #[test]
    fn a_wgsl_matrix_lists_columns() {
        let m = wgsl_mat3(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
        assert_eq!(
            m,
            "mat3x3f(vec3f(1.000000000, 4.000000000, 7.000000000), vec3f(2.000000000, 5.000000000, 8.000000000), vec3f(3.000000000, 6.000000000, 9.000000000))"
        );
    }
}
