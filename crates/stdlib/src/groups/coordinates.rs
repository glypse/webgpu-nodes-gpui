//! Coordinates reshaped to the canvas' aspect ratio.

use shader_graph::types::WgslType;

use crate::builder::{EnumRef, Frame, GroupBuilder, GroupSpec, Input, Output, Variant};
use crate::groups::helpers::{compare, resolution, uv};

pub const ASPECT_UV: &str = "std-aspect-uv";

/// The Contain, Cover and Stretch choice of the groups that fit coordinates to an aspect ratio.
pub fn fit_mode(g: &GroupBuilder<'_>) -> EnumRef {
    g.enum_input(
        "mode",
        Input::enumeration(
            [
                Variant::new(0, "Contain (Fit)"),
                Variant::new(1, "Cover (Fill)"),
                Variant::new(2, "Stretch"),
            ],
            0,
        )
        .label("Mode"),
    )
}

pub fn all() -> Vec<GroupSpec> {
    vec![GroupSpec::new(
        ASPECT_UV,
        "Aspect UV",
        "input",
        &["coordinates", "mode"],
        &["coordinates"],
        |g| {
            let coordinates = g.input("coordinates", Input::new(WgslType::Vec2f).label("Coordinates"));
            let mode = fit_mode(g);
            let (aspect, wide) = g.frame(
                Frame::new("Aspect ratio")
                    .note("The canvas' width over its height, guarded against zero."),
                || {
                    let size = g.op("separate-vector", resolution(g));
                    let height = g.op("max", (size.out("y"), 1.0)).first();
                    let ratio = g.op("div", (size.out("x"), height)).first();
                    let aspect = g.op("max", (ratio, 0.000001)).first();
                    (aspect, compare(g, aspect, 1.0, ">="))
                },
            );
            let scale = g.frame(
                Frame::new("Scale").note(
                    "Squares stay square: the longer side is stretched (Contain)\nor the shorter one shrunk (Cover), unless stretched to fit.",
                ),
                || {
                    let shrink_y = g.vec(WgslType::Vec2f, (1.0, g.op("div", (1.0, aspect)))).first();
                    let stretch_x = g.vec(WgslType::Vec2f, (aspect, 1.0)).first();
                    let contain = g.op("switch", (wide, shrink_y, stretch_x)).first();
                    let cover = g.op("switch", (wide, stretch_x, shrink_y)).first();
                    g.match_on(&mode)
                        .case(0, contain)
                        .case(1, cover)
                        .case(2, 1.0)
                        .add()
                        .first()
                },
            );
            let fallback = g.op("fallback", (coordinates, uv(g))).first();
            g.output(
                "coordinates",
                g.op("mul", (fallback, scale)),
                Output::new(WgslType::Vec2f).label("Coordinates"),
            );
        },
    )
    .aliases(&["Aspect Ratio", "Aspect Correction", "Square UV"])]
}
