//! Conversions between linear RGB, XYZ, LMS, OKLab and OKLch, after Björn
//! Ottosson's OKLab.

use std::f64::consts::PI;

use graph_math::color as matrices;
use shader_graph::types::WgslType;

use crate::builder::{Frame, GroupBuilder, GroupSpec, Ref, vector};
use crate::groups::helpers::{call_one, vec3_in, vec3_out};

pub const LINEAR_RGB_TO_XYZ: &str = "std-linear-rgb-to-xyz";
pub const XYZ_TO_LINEAR_RGB: &str = "std-xyz-to-linear-rgb";
pub const XYZ_TO_OKLAB: &str = "std-xyz-to-oklab";
pub const OKLAB_TO_XYZ: &str = "std-oklab-to-xyz";
pub const LINEAR_RGB_TO_OKLAB: &str = "std-linear-rgb-to-oklab";
pub const OKLAB_TO_LINEAR_RGB: &str = "std-oklab-to-linear-rgb";
pub const OKLAB_TO_OKLCH: &str = "std-oklab-to-oklch";
pub const OKLCH_TO_OKLAB: &str = "std-oklch-to-oklab";
pub const LINEAR_RGB_TO_OKLCH: &str = "std-linear-rgb-to-oklch";
pub const OKLCH_TO_LINEAR_RGB: &str = "std-oklch-to-linear-rgb";

const CONVERSION: &str = "Color Space Conversion";

/// `v` through `matrix`, a row at a time.
fn transform(g: &GroupBuilder<'_>, v: Ref, matrix: &matrices::Mat3) -> Ref {
    let row = |r: &[f64; 3]| g.op("dot", (v, vector(r)));
    g.vec(
        WgslType::Vec3f,
        (row(&matrix[0]), row(&matrix[1]), row(&matrix[2])),
    )
    .first()
}

/// `v` to the power of `exponent`, keeping each component's sign.
fn signed_pow(g: &GroupBuilder<'_>, v: Ref, exponent: f64) -> Ref {
    let sign = g.op("sign", v);
    let magnitude = g.op("pow", (g.op("abs", v), exponent));
    g.op("mul", (sign, magnitude)).first()
}

/// To OKLab from a space whose matrix `to_lms` takes to the cones' responses (LMS).
fn to_oklab(g: &GroupBuilder<'_>, from: Ref, to_lms: &matrices::Mat3, space: &str) -> Ref {
    let lms = g.frame(
        Frame::new("LMS").note(format!("The cones' responses to the {space} colour.")),
        || transform(g, from, to_lms),
    );
    let compressed = g.frame(
        Frame::new("Cube root")
            .note("Compressed the way perception is (signed, for colours out of gamut)."),
        || signed_pow(g, lms, 1.0 / 3.0),
    );
    g.frame(
        Frame::new("OKLab")
            .note("Lightness, then green–red and blue–yellow, from the compressed responses."),
        || transform(g, compressed, &matrices::LMS_TO_OKLAB),
    )
}

/// From OKLab to a space whose matrix `from_lms` takes the cones' responses (LMS) to.
fn from_oklab(g: &GroupBuilder<'_>, lab: Ref, from_lms: &matrices::Mat3, space: &str) -> Ref {
    let compressed = g.frame(
        Frame::new("Compressed LMS")
            .note("The cones' compressed responses the OKLab colour is made of."),
        || transform(g, lab, &matrices::OKLAB_TO_LMS),
    );
    let lms = g.frame(
        Frame::new("Cube").note("Uncompressed (signed, for colours out of gamut)."),
        || signed_pow(g, compressed, 3.0),
    );
    g.frame(
        Frame::new(format!("To {space}"))
            .note(format!("The {space} colour giving those responses.")),
        || transform(g, lms, from_lms),
    )
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            LINEAR_RGB_TO_XYZ,
            "Linear RGB to XYZ",
            "color",
            &["rgb"],
            &["xyz"],
            |g| {
                let rgb = vec3_in(g, "rgb", "Linear RGB");
                vec3_out(
                    g,
                    "xyz",
                    transform(g, rgb, &matrices::LINEAR_RGB_TO_XYZ),
                    "XYZ",
                );
            },
        )
        .aliases(&["sRGB to XYZ", "RGB to XYZ", CONVERSION]),
        GroupSpec::new(
            XYZ_TO_LINEAR_RGB,
            "XYZ to Linear RGB",
            "color",
            &["xyz"],
            &["rgb"],
            |g| {
                let xyz = vec3_in(g, "xyz", "XYZ");
                vec3_out(
                    g,
                    "rgb",
                    transform(g, xyz, &matrices::XYZ_TO_LINEAR_RGB),
                    "Linear RGB",
                );
            },
        )
        .aliases(&["XYZ to sRGB", "XYZ to RGB", CONVERSION]),
        GroupSpec::new(
            XYZ_TO_OKLAB,
            "XYZ to OKLab",
            "color",
            &["xyz"],
            &["oklab"],
            |g| {
                let xyz = vec3_in(g, "xyz", "XYZ");
                vec3_out(
                    g,
                    "oklab",
                    to_oklab(g, xyz, &matrices::XYZ_TO_LMS, "XYZ"),
                    "OKLab",
                );
            },
        )
        .aliases(&[CONVERSION, "Perceptual Color"]),
        GroupSpec::new(
            OKLAB_TO_XYZ,
            "OKLab to XYZ",
            "color",
            &["oklab"],
            &["xyz"],
            |g| {
                let lab = vec3_in(g, "oklab", "OKLab");
                vec3_out(
                    g,
                    "xyz",
                    from_oklab(g, lab, &matrices::LMS_TO_XYZ, "XYZ"),
                    "XYZ",
                );
            },
        )
        .aliases(&[CONVERSION, "Perceptual Color"]),
        GroupSpec::new(
            LINEAR_RGB_TO_OKLAB,
            "Linear RGB to OKLab",
            "color",
            &["rgb"],
            &["oklab"],
            |g| {
                let rgb = vec3_in(g, "rgb", "Linear RGB");
                vec3_out(
                    g,
                    "oklab",
                    to_oklab(g, rgb, &matrices::LINEAR_RGB_TO_LMS, "linear RGB"),
                    "OKLab",
                );
            },
        )
        .aliases(&[
            "sRGB to OKLab",
            "RGB to OKLab",
            CONVERSION,
            "Perceptual Color",
        ]),
        GroupSpec::new(
            OKLAB_TO_LINEAR_RGB,
            "OKLab to Linear RGB",
            "color",
            &["oklab"],
            &["rgb"],
            |g| {
                let lab = vec3_in(g, "oklab", "OKLab");
                vec3_out(
                    g,
                    "rgb",
                    from_oklab(g, lab, &matrices::LMS_TO_LINEAR_RGB, "linear RGB"),
                    "Linear RGB",
                );
            },
        )
        .aliases(&[
            "OKLab to sRGB",
            "OKLab to RGB",
            CONVERSION,
            "Perceptual Color",
        ]),
        GroupSpec::new(
            OKLAB_TO_OKLCH,
            "OKLab to OKLch",
            "color",
            &["oklab"],
            &["oklch"],
            |g| {
                let lab = vec3_in(g, "oklab", "OKLab");
                let split = g.op("separate-vector", lab);
                let (lightness, a, b) = (split.out("x"), split.out("y"), split.out("z"));
                let chroma = g.frame(
                    Frame::new("Chroma").note("How far from grey: the length of (a, b)."),
                    || {
                        let ab = g.vec(WgslType::Vec2f, (a, b)).first();
                        g.op("length", ab).first()
                    },
                );
                let hue = g.frame(
                    Frame::new("Hue").note("The angle of (a, b), in degrees from 0 to 360."),
                    || {
                        let turns = g.op("atan2", (b, a));
                        let fraction = g.op("fract", g.op("mul", (turns, 1.0 / (2.0 * PI))));
                        g.op("mul", (fraction, 360.0)).first()
                    },
                );
                let oklch = g.vec(WgslType::Vec3f, (lightness, chroma, hue)).first();
                vec3_out(g, "oklch", oklch, "OKLch");
            },
        )
        .aliases(&["Lab to LCh", "Polar", CONVERSION]),
        GroupSpec::new(
            OKLCH_TO_OKLAB,
            "OKLch to OKLab",
            "color",
            &["oklch"],
            &["oklab"],
            |g| {
                let lch = vec3_in(g, "oklch", "OKLch");
                let split = g.op("separate-vector", lch);
                let (lightness, chroma, hue) = (split.out("x"), split.out("y"), split.out("z"));
                let (a, b) = g.frame(
                    Frame::new("a and b")
                        .note("Chroma along the hue's direction (hue in degrees)."),
                    || {
                        let radians = g.op("mul", (hue, PI / 180.0)).first();
                        let a = g.op("mul", (chroma, g.op("cos", radians))).first();
                        let b = g.op("mul", (chroma, g.op("sin", radians))).first();
                        (a, b)
                    },
                );
                let oklab = g.vec(WgslType::Vec3f, (lightness, a, b)).first();
                vec3_out(g, "oklab", oklab, "OKLab");
            },
        )
        .aliases(&["LCh to Lab", "Polar", CONVERSION]),
        GroupSpec::new(
            LINEAR_RGB_TO_OKLCH,
            "Linear RGB to OKLch",
            "color",
            &["rgb"],
            &["oklch"],
            |g| {
                let rgb = vec3_in(g, "rgb", "Linear RGB");
                let oklab = call_one(g, LINEAR_RGB_TO_OKLAB, "rgb", rgb, "oklab");
                let oklch = call_one(g, OKLAB_TO_OKLCH, "oklab", oklab, "oklch");
                vec3_out(g, "oklch", oklch, "OKLch");
            },
        )
        .aliases(&[
            "sRGB to OKLch",
            "RGB to OKLch",
            CONVERSION,
            "Perceptual Color",
        ]),
        GroupSpec::new(
            OKLCH_TO_LINEAR_RGB,
            "OKLch to Linear RGB",
            "color",
            &["oklch"],
            &["rgb"],
            |g| {
                let oklch = vec3_in(g, "oklch", "OKLch");
                let oklab = call_one(g, OKLCH_TO_OKLAB, "oklch", oklch, "oklab");
                let rgb = call_one(g, OKLAB_TO_LINEAR_RGB, "oklab", oklab, "rgb");
                vec3_out(g, "rgb", rgb, "Linear RGB");
            },
        )
        .aliases(&[
            "OKLch to sRGB",
            "OKLch to RGB",
            CONVERSION,
            "Perceptual Color",
        ]),
    ]
}
