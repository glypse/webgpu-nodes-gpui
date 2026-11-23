//! Texture-driven helpers.

use shader_graph::types::WgslType;

use crate::builder::{Frame, GroupSpec, Input, Output, Ref};
use crate::groups::coordinates::fit_mode;
use crate::groups::helpers::{resolution, uv};

pub const FIT_UV: &str = "std-fit-uv";

pub fn all() -> Vec<GroupSpec> {
    vec![GroupSpec::new(
        FIT_UV,
        "Fit UV",
        "texture",
        &["texture", "coordinates", "mode"],
        &["coordinates"],
        |g| {
            let texture = g.input("texture", Input::new(WgslType::Texture).label("Texture"));
            let coordinates = g.input("coordinates", Input::new(WgslType::Vec2f).label("Coordinates"));
            let mode = fit_mode(g);
            let ratio = g.frame(
                Frame::new("Aspect ratios")
                    .note("The canvas' width over its height, over the texture's."),
                || {
                    let aspect = |size: Ref| {
                        let split = g.op("separate-vector", size);
                        let height = g.op("max", (split.out("y"), 1.0)).first();
                        g.op("div", (split.out("x"), height)).first()
                    };
                    let texture_size = g.op("texture-size", (texture, 0)).out("size");
                    g.op("div", (aspect(resolution(g)), aspect(texture_size))).first()
                },
            );
            let scale = g.frame(
                Frame::new("Scale").note(
                    "Around the centre, the texture's longer side fits the canvas (Contain,\nleaving bands of its edges around it) or its shorter one does (Cover,\ncropping it), unless it's stretched to fit.",
                ),
                || {
                    let inverse = g.op("div", (1.0, ratio)).first();
                    let contain = g
                        .vec(WgslType::Vec2f, (g.op("max", (ratio, 1.0)), g.op("max", (inverse, 1.0))))
                        .first();
                    let cover = g
                        .vec(WgslType::Vec2f, (g.op("min", (ratio, 1.0)), g.op("min", (inverse, 1.0))))
                        .first();
                    g.match_on(&mode)
                        .case(0, contain)
                        .case(1, cover)
                        .case(2, 1.0)
                        .add()
                        .first()
                },
            );
            let centred = g.op("sub", (g.op("fallback", (coordinates, uv(g))), 0.5)).first();
            g.output(
                "coordinates",
                g.op("add", (g.op("mul", (centred, scale)), 0.5)),
                Output::new(WgslType::Vec2f).label("Coordinates"),
            );
        },
    )
    .aliases(&["Fit Image", "Image Aspect", "Contain", "Cover", "Letterbox", "Object Fit"])]
}
