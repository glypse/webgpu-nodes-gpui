use gpui::{Hsla, Rgba};

/// An OKLCH colour (the original's CSS notation, hue in degrees), clipped to sRGB.
pub fn oklch(l: f32, c: f32, h: f32) -> Hsla {
    oklch_a(l, c, h, 1.0)
}

pub fn oklch_a(l: f32, c: f32, h: f32, alpha: f32) -> Hsla {
    let (a, b) = {
        let h = h.to_radians();
        (c * h.cos(), c * h.sin())
    };
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    Rgba {
        r: encode(r),
        g: encode(g),
        b: encode(b),
        a: alpha,
    }
    .into()
}

/// Linear to sRGB transfer.
fn encode(linear: f32) -> f32 {
    let c = linear.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Hsla) -> [u8; 3] {
        let c = Rgba::from(c);
        [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn matches_css() {
        assert_eq!(rgb(oklch(1.0, 0.0, 0.0)), [255, 255, 255]);
        assert_eq!(rgb(oklch(0.145, 0.0, 0.0)), [10, 10, 10]);
        assert_eq!(rgb(oklch(0.922, 0.0, 0.0)), [229, 229, 229]);
        // Tailwind's red-ish destructive, rgb(231 0 11) in CSS.
        let [r, g, b] = rgb(oklch(0.577, 0.245, 27.325));
        assert!(
            r.abs_diff(231) <= 1 && g <= 1 && b.abs_diff(11) <= 2,
            "{r} {g} {b}"
        );
    }
}
