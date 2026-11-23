//! Colour conversions. Colours are held as linear sRGB (what shaders compute
//! with), components unbounded above for HDR and below for colours past sRGB;
//! HSV is of the sRGB-encoded colour, and hex and on-screen swatches are
//! encoded in the colour space shown.

use palette::{
    Hsv as PaletteHsv, LinSrgb, Oklab, Oklch, Srgb as PaletteSrgb,
    convert::FromColorUnclamped,
    encoding::{FromLinear, IntoLinear, Srgb as SrgbEncoding},
    hues::{OklabHue, RgbHue},
};

use super::{ColorSpace, Mat3, clip_to_gamut, from_gamut, to_gamut};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(self) -> Rgb {
        Rgb::new(self.r, self.g, self.b)
    }
}

/// Hue, saturation and value in [0, 1].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hsv {
    pub h: f64,
    pub s: f64,
    pub v: f64,
}

/// OKLch: lightness in [0, 1], chroma (about [0, 0.37] in sRGB), hue in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

type Lab = Oklab<f64>;

pub fn srgb_encode(linear: f64) -> f64 {
    <SrgbEncoding as FromLinear<f64, f64>>::from_linear(linear.max(0.0))
}

pub fn srgb_decode(encoded: f64) -> f64 {
    <SrgbEncoding as IntoLinear<f64, f64>>::into_linear(encoded.max(0.0))
}

pub fn encode_rgb(Rgb { r, g, b }: Rgb) -> Rgb {
    Rgb::new(srgb_encode(r), srgb_encode(g), srgb_encode(b))
}

pub fn decode_rgb(Rgb { r, g, b }: Rgb) -> Rgb {
    Rgb::new(srgb_decode(r), srgb_decode(g), srgb_decode(b))
}

/// A linear sRGB colour, gamma-encoded in `space` (whose transfer curve is sRGB's).
pub fn encode_in(color: Rgb, space: ColorSpace) -> Rgb {
    encode_rgb(to_gamut(color, space))
}

/// The linear sRGB colour of one gamma-encoded in `space`.
pub fn decode_from(color: Rgb, space: ColorSpace) -> Rgb {
    from_gamut(decode_rgb(color), space)
}

/// HSV of an (encoded) RGB colour, clamped to [0, 1] first.
pub fn rgb_to_hsv(rgb: Rgb) -> Hsv {
    let hsv = PaletteHsv::from_color_unclamped(PaletteSrgb::new(
        rgb.r.clamp(0.0, 1.0),
        rgb.g.clamp(0.0, 1.0),
        rgb.b.clamp(0.0, 1.0),
    ));
    Hsv {
        h: hsv.hue.into_positive_degrees() / 360.0,
        s: hsv.saturation,
        v: hsv.value,
    }
}

pub fn hsv_to_rgb(Hsv { h, s, v }: Hsv) -> Rgb {
    let hue = ((h % 1.0) + 1.0) % 1.0;
    let rgb =
        PaletteSrgb::from_color_unclamped(PaletteHsv::new(RgbHue::from_degrees(hue * 360.0), s, v));
    Rgb::new(rgb.red, rgb.green, rgb.blue)
}

// Björn Ottosson's OKLab matrices, as f64 constants to emit as WGSL. The CPU
// conversions use `palette`; a test checks these agree with it.
pub const LINEAR_RGB_TO_LMS: Mat3 = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
];
pub const LMS_TO_OKLAB: Mat3 = [
    [0.2104542553, 0.793617785, -0.0040720468],
    [1.9779984951, -2.428592205, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.808675766],
];
pub const OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.3963377774, 0.2158037573],
    [1.0, -0.1055613458, -0.0638541728],
    [1.0, -0.0894841775, -1.291485548],
];
pub const LMS_TO_LINEAR_RGB: Mat3 = [
    [4.0767416621, -3.3077115913, 0.2309699292],
    [-1.2684380046, 2.6097574011, -0.3413193965],
    [-0.0041960863, -0.7034186147, 1.707614701],
];

fn linear_to_oklab(rgb: Rgb) -> Lab {
    Oklab::from_color_unclamped(LinSrgb::new(rgb.r, rgb.g, rgb.b))
}

/// Linear RGB of an OKLab colour, outside [0, 1] when out of the sRGB gamut.
fn oklab_to_linear(lab: Lab) -> Rgb {
    let linear = LinSrgb::from_color_unclamped(lab);
    Rgb::new(linear.red, linear.green, linear.blue)
}

fn oklch_to_oklab(l: f64, c: f64, h: f64) -> Lab {
    Oklab::from_color_unclamped(Oklch::new(l, c, OklabHue::from_degrees(h)))
}

/// Linear RGB of an OKLch colour as is: outside [0, 1] when out of the sRGB gamut.
pub fn oklch_to_linear_unmapped(Lch { l, c, h }: Lch) -> Rgb {
    oklab_to_linear(oklch_to_oklab(l, c, h))
}

/// OKLch of a linear RGB colour.
pub fn linear_to_oklch(rgb: Rgb) -> Lch {
    let lch = Oklch::from_color_unclamped(linear_to_oklab(rgb));
    Lch {
        l: lch.l,
        c: lch.chroma,
        h: lch.hue.into_positive_degrees(),
    }
}

/// Linear sRGB of an OKLch colour, mapped into `space`'s gamut as CSS Color 4
/// does (and the standard library's OKLch Gamut Map group): lightness and hue
/// kept, chroma lowered until the colour fits or its clip is less than a just
/// noticeable difference away. Clipping each channel instead shifts hue and
/// lightness.
pub fn oklch_to_linear(Lch { l, c, h }: Lch, space: ColorSpace) -> Rgb {
    const JND: f64 = 0.02;
    if l >= 1.0 {
        return Rgb::new(1.0, 1.0, 1.0);
    }
    if l <= 0.0 {
        return Rgb::new(0.0, 0.0, 0.0);
    }
    let clip = |rgb: Rgb| clip_to_gamut(rgb, space);
    let off_by = |lab: Lab, clipped: Rgb| {
        let mapped = linear_to_oklab(clipped);
        (mapped.l - lab.l)
            .hypot(mapped.a - lab.a)
            .hypot(mapped.b - lab.b)
    };
    let fits = |rgb: Rgb| {
        let clipped = clip(rgb);
        (rgb.r - clipped.r)
            .hypot(rgb.g - clipped.g)
            .hypot(rgb.b - clipped.b)
            <= 1e-5
    };
    let origin = oklch_to_oklab(l, c, h);
    let origin_clip = clip(oklab_to_linear(origin));
    if off_by(origin, origin_clip) < JND {
        return origin_clip;
    }
    let (mut low, mut high) = (0.0, c);
    let mut low_in_gamut = true;
    for _ in 0..16 {
        let mid = (low + high) / 2.0;
        let lab = oklch_to_oklab(l, mid, h);
        let rgb = oklab_to_linear(lab);
        let inside = fits(rgb);
        let near = off_by(lab, clip(rgb)) < JND;
        if (low_in_gamut && inside) || near {
            low_in_gamut = low_in_gamut && (inside || !near);
            low = mid;
        } else {
            high = mid;
        }
    }
    clip(oklab_to_linear(oklch_to_oklab(l, low, h)))
}

fn hex_byte(n: f64) -> String {
    format!("{:02X}", (n.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// `#RRGGBB` (or `#RRGGBBAA` with `alpha`) of a linear colour, encoded in `space`.
pub fn linear_to_hex(color: Rgba, alpha: bool, space: ColorSpace) -> String {
    let Rgb { r, g, b } = encode_in(color.rgb(), space);
    let tail = if alpha {
        hex_byte(color.a)
    } else {
        String::new()
    };
    format!("#{}{}{}{tail}", hex_byte(r), hex_byte(g), hex_byte(b))
}

/// The linear colour of a hex string (`#RGB`, `#RGBA`, `#RRGGBB` or
/// `#RRGGBBAA`, `#` optional) encoded in `space`, or `None` if it isn't one.
pub fn hex_to_linear(hex: &str, space: ColorSpace) -> Option<Rgba> {
    let trimmed = hex.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits: String = if digits.len() == 3 || digits.len() == 4 {
        digits.chars().flat_map(|c| [c, c]).collect()
    } else {
        digits.to_owned()
    };
    if digits.len() != 6 && digits.len() != 8 {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(digits.get(i..i + 2)?, 16)
            .ok()
            .map(|n| f64::from(n) / 255.0)
    };
    let rgb = decode_from(Rgb::new(byte(0)?, byte(2)?, byte(4)?), space);
    let a = if digits.len() == 8 { byte(6)? } else { 1.0 };
    Some(Rgba::new(rgb.r, rgb.g, rgb.b, a))
}

const GREY: f64 = 1e-6;

/// Below this saturation or chroma a colour's hue doesn't show. The ramp shader
/// tests against the same value, so both sides must agree.
pub const ACHROMATIC: f64 = 1e-4;

/// HSV (of the sRGB-encoded colour) of a linear one, keeping `previous`'s
/// hue for a grey and its hue and saturation for black, which have none: so
/// a hue survives going through them.
pub fn hsv_keeping_hue(color: Rgb, previous: Hsv) -> Hsv {
    let next = rgb_to_hsv(encode_rgb(color));
    if next.v < GREY {
        return Hsv { v: 0.0, ..previous };
    }
    if next.s < GREY {
        Hsv {
            h: previous.h,
            ..next
        }
    } else {
        next
    }
}

/// OKLch of a linear colour, keeping `previous`'s hue for a grey, which has none.
pub fn lch_keeping_hue(color: Rgb, previous: Lch) -> Lch {
    let next = linear_to_oklch(color);
    if next.c < ACHROMATIC {
        Lch {
            c: 0.0,
            h: previous.h,
            ..next
        }
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::apply_mat3;

    fn close(actual: f64, expected: f64, digits: i32) {
        let tolerance = 0.5 * 10f64.powi(-digits);
        assert!(
            (actual - expected).abs() < tolerance,
            "{actual} vs {expected}"
        );
    }

    fn close_rgb(actual: Rgb, expected: Rgb, digits: i32) {
        close(actual.r, expected.r, digits);
        close(actual.g, expected.g, digits);
        close(actual.b, expected.b, digits);
    }

    #[test]
    fn round_trips_the_srgb_transfer_function() {
        for n in [0.0, 0.001, 0.2, 0.5, 1.0] {
            close(srgb_decode(srgb_encode(n)), n, 6);
        }
        close(srgb_encode(0.214), 0.5, 2);
    }

    #[test]
    fn converts_between_rgb_and_hsv() {
        let hsv = rgb_to_hsv(Rgb::new(1.0, 0.0, 0.0));
        assert_eq!((hsv.h, hsv.s, hsv.v), (0.0, 1.0, 1.0));
        let hsv = rgb_to_hsv(Rgb::new(0.0, 0.5, 0.5));
        assert_eq!((hsv.h, hsv.s, hsv.v), (0.5, 1.0, 0.5));
        let hsv = rgb_to_hsv(Rgb::new(0.3, 0.3, 0.3));
        assert_eq!((hsv.h, hsv.s, hsv.v), (0.0, 0.0, 0.3));
        for hsv in [
            Hsv {
                h: 0.1,
                s: 0.4,
                v: 0.9,
            },
            Hsv {
                h: 0.7,
                s: 1.0,
                v: 0.2,
            },
            Hsv {
                h: 0.95,
                s: 0.5,
                v: 0.5,
            },
        ] {
            let back = rgb_to_hsv(hsv_to_rgb(hsv));
            close(back.h, hsv.h, 4);
            close(back.s, hsv.s, 4);
            close(back.v, hsv.v, 4);
        }
    }

    #[test]
    fn converts_between_linear_rgb_and_oklch() {
        // Reference values from Björn Ottosson's OKLab post.
        let white = linear_to_oklch(Rgb::new(1.0, 1.0, 1.0));
        close(white.l, 1.0, 3);
        close(white.c, 0.0, 3);
        let red = linear_to_oklch(Rgb::new(1.0, 0.0, 0.0));
        close(red.l, 0.628, 2);
        close(red.c, 0.2577, 2);
        close(red.h, 29.23, 2);
        let rgb = Rgb::new(0.2, 0.5, 0.1);
        close_rgb(
            oklch_to_linear(linear_to_oklch(rgb), ColorSpace::Srgb),
            rgb,
            5,
        );
    }

    #[test]
    fn reads_and_writes_srgb_encoded_hex() {
        let hex = |r, g, b, a, alpha| linear_to_hex(Rgba::new(r, g, b, a), alpha, ColorSpace::Srgb);
        assert_eq!(
            hex(1.0, 0.0, srgb_decode(128.0 / 255.0), 1.0, false),
            "#FF0080"
        );
        assert_eq!(hex(2.0, -1.0, 0.0, 0.5, true), "#FF000080");
        let parsed = hex_to_linear("#ff0080", ColorSpace::Srgb);
        let Some(parsed) = parsed else {
            panic!("parses")
        };
        close_rgb(
            parsed.rgb(),
            Rgb::new(1.0, 0.0, srgb_decode(128.0 / 255.0)),
            4,
        );
        let Some(short) = hex_to_linear("f008", ColorSpace::Srgb) else {
            panic!("parses")
        };
        close_rgb(short.rgb(), Rgb::new(1.0, 0.0, 0.0), 4);
        close(short.a, 136.0 / 255.0, 4);
        assert_eq!(hex_to_linear("#12345", ColorSpace::Srgb), None);
        assert_eq!(hex_to_linear("nothex", ColorSpace::Srgb), None);
        assert_eq!(hex_to_linear("#é12345", ColorSpace::Srgb), None);
    }

    #[test]
    fn reads_and_writes_hex_in_display_p3() {
        let outside = oklch_to_linear(
            Lch {
                l: 0.7015,
                c: 0.1559,
                h: 203.59,
            },
            ColorSpace::DisplayP3,
        );
        let hex = linear_to_hex(
            Rgba::new(outside.r, outside.g, outside.b, 1.0),
            false,
            ColorSpace::DisplayP3,
        );
        assert_eq!(hex, "#16B6C7");
        let Some(back) = hex_to_linear(&hex, ColorSpace::DisplayP3) else {
            panic!("parses")
        };
        close_rgb(back.rgb(), outside, 2);
        let white = Rgba::new(1.0, 1.0, 1.0, 1.0);
        assert_eq!(
            linear_to_hex(white, false, ColorSpace::DisplayP3),
            "#FFFFFF"
        );
    }

    #[test]
    fn keeps_the_hue_of_greys() {
        let previous = Hsv {
            h: 0.3,
            s: 0.5,
            v: 0.5,
        };
        assert_eq!(hsv_keeping_hue(Rgb::new(0.2, 0.2, 0.2), previous).h, 0.3);
        let black = hsv_keeping_hue(Rgb::default(), previous);
        assert_eq!((black.h, black.s, black.v), (0.3, 0.5, 0.0));
        let grey = lch_keeping_hue(
            Rgb::new(0.2, 0.2, 0.2),
            Lch {
                l: 0.5,
                c: 0.1,
                h: 120.0,
            },
        );
        assert_eq!((grey.c, grey.h), (0.0, 120.0));
    }

    #[test]
    fn maps_lightness_extremes_to_white_and_black() {
        let lch = |l| Lch { l, c: 0.3, h: 40.0 };
        assert_eq!(
            oklch_to_linear(lch(1.2), ColorSpace::Srgb),
            Rgb::new(1.0, 1.0, 1.0)
        );
        assert_eq!(oklch_to_linear(lch(-0.1), ColorSpace::Srgb), Rgb::default());
    }

    #[test]
    fn maps_out_of_gamut_colours_into_the_gamut() {
        for space in ColorSpace::ALL {
            for l in [0.15, 0.35, 0.55, 0.75, 0.92] {
                for c in [0.05, 0.12, 0.22, 0.32] {
                    for h in [0.0, 72.0, 144.0, 216.0, 288.0] {
                        let mapped = oklch_to_linear(Lch { l, c, h }, space);
                        assert!(crate::color::in_gamut(mapped, space), "{l} {c} {h}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_codegen_oklab_matrices_match_palette() {
        for rgb in [
            Rgb::new(0.2, 0.5, 0.1),
            Rgb::new(1.0, 0.0, 0.0),
            Rgb::new(0.05, 0.9, 1.4),
        ] {
            let lms = apply_mat3(&LINEAR_RGB_TO_LMS, rgb);
            let lab = apply_mat3(
                &LMS_TO_OKLAB,
                Rgb::new(lms.r.cbrt(), lms.g.cbrt(), lms.b.cbrt()),
            );
            let expected = linear_to_oklab(rgb);
            close(lab.r, expected.l, 7);
            close(lab.g, expected.a, 7);
            close(lab.b, expected.b, 7);

            let back = apply_mat3(&LMS_TO_LINEAR_RGB, {
                let lms = apply_mat3(&OKLAB_TO_LMS, lab);
                Rgb::new(lms.r.powi(3), lms.g.powi(3), lms.b.powi(3))
            });
            close_rgb(back, rgb, 6);
        }
    }

    #[test]
    fn keeps_colours_past_srgb_but_within_display_p3() {
        let lch = Lch {
            l: 0.7015,
            c: 0.1559,
            h: 203.59,
        };
        let exact = oklch_to_linear_unmapped(lch);
        assert!(exact.r < 0.0);
        close_rgb(oklch_to_linear(lch, ColorSpace::DisplayP3), exact, 6);
    }
}
