use gpui::{Hsla, Rgba};
use graph_math::color::{Lch, oklch_to_linear_unmapped, srgb_encode};

/// An OKLCH colour (hue in degrees), clipped to sRGB.
pub fn oklch(l: f32, c: f32, h: f32) -> Hsla {
    oklch_a(l, c, h, 1.0)
}

pub fn oklch_a(l: f32, c: f32, h: f32, alpha: f32) -> Hsla {
    let linear = oklch_to_linear_unmapped(Lch {
        l: l as f64,
        c: c as f64,
        h: h as f64,
    });
    let encode = |v: f64| srgb_encode(v).clamp(0.0, 1.0) as f32;
    Rgba {
        r: encode(linear.r),
        g: encode(linear.g),
        b: encode(linear.b),
        a: alpha,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Hsla) -> [u8; 3] {
        let c = Rgba::from(c);
        [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn matches_reference_values() {
        assert_eq!(rgb(oklch(1.0, 0.0, 0.0)), [255, 255, 255]);
        assert_eq!(rgb(oklch(0.145, 0.0, 0.0)), [10, 10, 10]);
        assert_eq!(rgb(oklch(0.922, 0.0, 0.0)), [229, 229, 229]);
        // Destructive red, rgb(231 0 11).
        let [r, g, b] = rgb(oklch(0.577, 0.245, 27.325));
        assert!(
            r.abs_diff(231) <= 1 && g <= 1 && b.abs_diff(11) <= 2,
            "{r} {g} {b}"
        );
    }
}
