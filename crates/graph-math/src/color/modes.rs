use super::ColorSpace;
use super::{
    ACHROMATIC, Hsv, Lch, Rgb, decode_rgb, encode_rgb, hsv_to_rgb, linear_to_oklch, oklch_to_linear,
    rgb_to_hsv,
};

/// What three colour components are. RGB is linear (what the shader computes
/// with); HSV is of the sRGB-encoded colour, hue in [0, 1];
/// OKLch has its hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorMode {
    Rgb,
    Hsv,
    Oklch,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Rgb, ColorMode::Hsv, ColorMode::Oklch];

    pub fn parse(name: &str) -> Option<ColorMode> {
        match name {
            "rgb" => Some(ColorMode::Rgb),
            "hsv" => Some(ColorMode::Hsv),
            "oklch" => Some(ColorMode::Oklch),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ColorMode::Rgb => "rgb",
            ColorMode::Hsv => "hsv",
            ColorMode::Oklch => "oklch",
        }
    }
}

/// Three components of a colour, in a `ColorMode`.
pub type ColorComponents = [f64; 3];

/// The linear RGB of components in `mode`.
pub fn color_to_linear(mode: ColorMode, [x, y, z]: ColorComponents) -> Rgb {
    match mode {
        ColorMode::Rgb => Rgb::new(x, y, z),
        ColorMode::Hsv => decode_rgb(hsv_to_rgb(Hsv {
            h: x - x.floor(),
            s: y,
            v: z,
        })),
        ColorMode::Oklch => oklch_to_linear(Lch { l: x, c: y, h: z }, ColorSpace::Srgb),
    }
}

/// The components in `mode` of a linear RGB colour. A hue that doesn't show
/// (grey, black) is `turns` instead, so it survives going through them.
pub fn linear_to_color(mode: ColorMode, rgb: Rgb, turns: f64) -> ColorComponents {
    match mode {
        ColorMode::Rgb => [rgb.r, rgb.g, rgb.b],
        ColorMode::Hsv => {
            let Hsv { h, s, v } = rgb_to_hsv(encode_rgb(rgb));
            [if s > 0.0 && v > 0.0 { h } else { turns }, s, v]
        }
        ColorMode::Oklch => {
            let Lch { l, c, h } = linear_to_oklch(rgb);
            if c < ACHROMATIC {
                [l, 0.0, turns * 360.0]
            } else {
                [l, c, h]
            }
        }
    }
}

/// The same colour's components in another mode, keeping a hidden hue.
pub fn convert_color_mode(
    from: ColorMode,
    to: ColorMode,
    components: ColorComponents,
) -> ColorComponents {
    let [x, _, z] = components;
    let turns = match from {
        ColorMode::Hsv => x,
        ColorMode::Oklch => z / 360.0,
        ColorMode::Rgb => 0.0,
    };
    linear_to_color(to, color_to_linear(from, components), turns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_keeps_the_colour() {
        let original = color_to_linear(ColorMode::Hsv, [0.6, 0.5, 0.7]);
        let mut components = [0.6, 0.5, 0.7];
        let mut mode = ColorMode::Hsv;
        for next in [ColorMode::Rgb, ColorMode::Oklch, ColorMode::Hsv] {
            components = convert_color_mode(mode, next, components);
            mode = next;
            let linear = color_to_linear(mode, components);
            assert!((linear.r - original.r).abs() < 1e-5);
            assert!((linear.g - original.g).abs() < 1e-5);
            assert!((linear.b - original.b).abs() < 1e-5);
        }
        assert!((components[0] - 0.6).abs() < 1e-5);
    }

    #[test]
    fn conversion_keeps_the_hue_of_a_grey() {
        let oklch = convert_color_mode(ColorMode::Hsv, ColorMode::Oklch, [0.25, 0.0, 0.5]);
        assert!((oklch[2] - 90.0).abs() < 1e-5);
    }
}
