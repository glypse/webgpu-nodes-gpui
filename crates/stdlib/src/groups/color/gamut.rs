//! Fitting a colour to the display: clip it when the clip is barely
//! noticeable, otherwise search for the most chroma the display shows at that
//! lightness and hue.

use shader_graph::types::WgslType;

use super::spaces::{LINEAR_RGB_TO_OKLAB, OKLAB_TO_LINEAR_RGB, OKLCH_TO_OKLAB};
use crate::builder::{Frame, GroupBuilder, GroupSpec, Output, Ref, RepeatItem};
use crate::groups::helpers::{boolean, call_one, compare, vec3_in};

pub const IN_DISPLAY_GAMUT: &str = "std-in-display-gamut";
pub const OKLCH_GAMUT_MAP: &str = "std-oklch-gamut-map";

/// In OKLab units: a clip that moves a colour less than this is not visible.
const JUST_NOTICEABLE: f64 = 0.02;

/// The OKLab and linear RGB a colour in OKLch converts to.
fn to_rgb(g: &GroupBuilder<'_>, oklch: Ref) -> (Ref, Ref) {
    let oklab = call_one(g, OKLCH_TO_OKLAB, "oklch", oklch, "oklab");
    let rgb = call_one(g, OKLAB_TO_LINEAR_RGB, "oklab", oklab, "rgb");
    (oklab, rgb)
}

struct Fit {
    clipped: Ref,
    close: Ref,
}

/// Clips `rgb` to the display, and whether that keeps it within notice of `oklab`.
fn clip_unnoticed(g: &GroupBuilder<'_>, oklab: Ref, rgb: Ref) -> Fit {
    let clipped = g.op("display-gamut-clip", rgb).first();
    let back = call_one(g, LINEAR_RGB_TO_OKLAB, "rgb", clipped, "oklab");
    let moved = g.op("distance", (back, oklab)).first();
    Fit {
        clipped,
        close: compare(g, moved, JUST_NOTICEABLE, "<"),
    }
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            IN_DISPLAY_GAMUT,
            "In Display Gamut",
            "color",
            &["rgb"],
            &["in-gamut"],
            |g| {
                let rgb = vec3_in(g, "rgb", "Linear RGB");
                let clipped = g.op("display-gamut-clip", rgb).first();
                let moved = g.op("distance", (rgb, clipped)).first();
                let in_gamut = compare(g, moved, 0.00001, "<=");
                g.output("in-gamut", in_gamut, Output::new(WgslType::Bool).label("In Gamut"));
            },
        )
        .aliases(&[
            "Gamut Check",
            "Out of Gamut",
            "In sRGB Gamut",
            "In Display P3 Gamut",
            "Color Space Conversion",
        ]),
        GroupSpec::new(
            OKLCH_GAMUT_MAP,
            "OKLch Gamut Map",
            "color",
            &["oklch"],
            &["rgb"],
            |g| {
                let oklch = vec3_in(g, "oklch", "OKLch");
                let split = g.op("separate-vector", oklch);
                let (lightness, chroma, hue) = (split.out("x"), split.out("y"), split.out("z"));

                let direct = g.frame(
                    Frame::new("Clipped").note(
                        "The colour clipped to the display, if that's barely noticeable.",
                    ),
                    || {
                        let (oklab, rgb) = to_rgb(g, oklch);
                        clip_unnoticed(g, oklab, rgb)
                    },
                );

                let search = g.frame(
                    Frame::new("Chroma search").note(
                        "Otherwise, the most chroma at that lightness and hue that the display\nshows, or nearly: halving the range 16 times (CSS Color 4's algorithm).",
                    ),
                    || {
                        g.repeat(
                            16,
                            vec![
                                RepeatItem::new("Low", WgslType::F32, 0.0),
                                RepeatItem::new("High", WgslType::F32, chroma),
                                RepeatItem::new("Low In Gamut", WgslType::Bool, true),
                            ],
                            |body| {
                                let low = body.state("Low");
                                let high = body.state("High");
                                let low_in_gamut = body.state("Low In Gamut");
                                let middle = g.op("mul", (g.op("add", (low, high)), 0.5)).first();
                                let lch = g.vec(WgslType::Vec3f, (lightness, middle, hue)).first();
                                let (oklab, rgb) = to_rgb(g, lch);
                                let in_gamut = call_one(g, IN_DISPLAY_GAMUT, "rgb", rgb, "in-gamut");
                                let close = clip_unnoticed(g, oklab, rgb).close;
                                let raise = boolean(
                                    g,
                                    boolean(g, low_in_gamut, in_gamut, "and"),
                                    close,
                                    "or",
                                );
                                let not_close = g.op("not", close).first();
                                body.set("Low", g.op("switch", (raise, low, middle)));
                                body.set("High", g.op("switch", (raise, middle, high)));
                                body.set(
                                    "Low In Gamut",
                                    boolean(
                                        g,
                                        low_in_gamut,
                                        boolean(g, in_gamut, not_close, "or"),
                                        "and",
                                    ),
                                );
                            },
                        )
                    },
                );

                let mapped = g.frame(
                    Frame::new("Mapped").note("The colour at the chroma found, clipped."),
                    || {
                        let lch = g
                            .vec(WgslType::Vec3f, (lightness, search.get("Low"), hue))
                            .first();
                        let (_, rgb) = to_rgb(g, lch);
                        g.op("display-gamut-clip", rgb).first()
                    },
                );

                g.frame(
                    Frame::new("Extremes").note("White past full lightness, black at none."),
                    || {
                        let fitted = g
                            .op("switch", (direct.close, mapped, direct.clipped))
                            .first();
                        let white = g
                            .op(
                                "switch",
                                (
                                    compare(g, lightness, 1.0, ">="),
                                    fitted,
                                    g.constant(WgslType::Vec3f, 1.0),
                                ),
                            )
                            .first();
                        let color = g
                            .op(
                                "switch",
                                (
                                    compare(g, lightness, 0.0, "<="),
                                    white,
                                    g.constant(WgslType::Vec3f, 0.0),
                                ),
                            )
                            .first();
                        g.output(
                            "rgb",
                            color,
                            Output::new(WgslType::Vec3f).label("Linear RGB"),
                        );
                    },
                );
            },
        )
        .aliases(&[
            "Gamut Mapping",
            "Gamut Clip",
            "OKLch to sRGB",
            "OKLch to Display P3",
            "Fit in Gamut",
            "Color Space Conversion",
        ]),
    ]
}
