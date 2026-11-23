//! Colour spaces, gamut mapping and the Color group that combines them.

mod gamut;
mod spaces;

use std::collections::BTreeMap;

use shader_graph::graph::{ColorWidgetMode, GroupWidget};
use shader_graph::types::{InputValue, WgslType};

use self::gamut::OKLCH_GAMUT_MAP;
use crate::builder::{Frame, GroupBuilder, GroupSpec, Input, InputIds, Output, Ref, Variant};
use crate::groups::helpers::{compare, unit};

pub const COLOR: &str = "std-color";

pub fn all() -> Vec<GroupSpec> {
    let mut groups = spaces::all();
    groups.extend(gamut::all());
    groups.push(color());
    groups
}

/// HSV to linear sRGB: each channel's distance around the hue circle, mixed
/// in by saturation and scaled by value, then linearised.
fn hsv_to_linear(g: &GroupBuilder<'_>, hue: Ref, saturation: Ref, value: Ref) -> Ref {
    let (h, s, v) = (hue, saturation, value);
    let srgb = g.frame(
        Frame::new("To sRGB").note(
            "Each channel's distance around the hue circle,\nmixed in by saturation and scaled by value.",
        ),
        || {
            let shifted = g.op(
                "add",
                (
                    h,
                    g.constant(
                        WgslType::Vec3f,
                        InputValue::vector(&[1.0, 2.0 / 3.0, 1.0 / 3.0]),
                    ),
                ),
            );
            let offset = g.op("fract", shifted).first();
            let span = g.op("sub", (g.op("mul", (offset, 6.0)), 3.0));
            let ramp = g
                .op("clamp", (g.op("sub", (g.op("abs", span), 1.0)), 0.0, 1.0))
                .first();
            let mixed = g.op("mix", (1.0, ramp, s));
            g.op("max", (g.op("mul", (mixed, v)), 0.0)).first()
        },
    );
    g.frame(
        Frame::new("To linear").note("sRGB's transfer function undone: HSV picks display colours."),
        || {
            let curve = g.op(
                "pow",
                (g.op("div", (g.op("add", (srgb, 0.055)), 1.055)), 2.4),
            );
            let linear = g.op("div", (srgb, 12.92));
            g.op("switch", (compare(g, srgb, 0.04045, "<="), curve, linear))
                .first()
        },
    )
}

fn color() -> GroupSpec {
    GroupSpec::new(
        COLOR,
        "Color",
        "color",
        &[
            "mode",
            "red",
            "green",
            "blue",
            "hue",
            "saturation",
            "value",
            "lightness",
            "chroma",
            "oklch-hue",
            "alpha",
        ],
        &["color", "rgba"],
        |g| {
            let mode = g.enum_input(
                "mode",
                Input::enumeration(
                    [
                        Variant::new(0, "RGB"),
                        Variant::new(1, "HSV"),
                        Variant::new(2, "OKLch"),
                    ],
                    1,
                )
                .label("Mode"),
            );

            let rgb = g.frame(
                Frame::new("RGB").note("Red, green and blue, as given."),
                || {
                    let channel = |key: &str, label: &str| {
                        g.input(
                            key,
                            Input::new(WgslType::F32)
                                .label(label)
                                .default(0.8)
                                .owner(&mode, 0),
                        )
                    };
                    let (red, green, blue) = (
                        channel("red", "Red"),
                        channel("green", "Green"),
                        channel("blue", "Blue"),
                    );
                    g.vec(WgslType::Vec3f, (red, green, blue)).first()
                },
            );

            let hsv = g.frame(Frame::new("HSV"), || {
                let hue = g.input(
                    "hue",
                    unit(
                        Input::new(WgslType::F32)
                            .label("Hue")
                            .default(0.0)
                            .owner(&mode, 1),
                    ),
                );
                let saturation = g.input(
                    "saturation",
                    unit(
                        Input::new(WgslType::F32)
                            .label("Saturation")
                            .default(0.0)
                            .owner(&mode, 1),
                    ),
                );
                let value = g.input(
                    "value",
                    unit(
                        Input::new(WgslType::F32)
                            .label("Value")
                            .default(0.9063)
                            .owner(&mode, 1),
                    ),
                );
                hsv_to_linear(g, hue, saturation, value)
            });

            let oklch = g.frame(
                Frame::new("OKLch").note(
                    "Lightness, chroma and hue (in degrees), mapped into the display's gamut.",
                ),
                || {
                    let lightness = g.input(
                        "lightness",
                        unit(
                            Input::new(WgslType::F32)
                                .label("Lightness")
                                .default(0.9283)
                                .owner(&mode, 2),
                        ),
                    );
                    let chroma = g.input(
                        "chroma",
                        unit(
                            Input::new(WgslType::F32)
                                .label("Chroma")
                                .default(0.0)
                                .owner(&mode, 2),
                        ),
                    );
                    let hue = g.input(
                        "oklch-hue",
                        Input::new(WgslType::F32)
                            .label("Hue")
                            .default(0.0)
                            .min(0.0)
                            .max(360.0)
                            .owner(&mode, 2),
                    );
                    let lch = g.vec(WgslType::Vec3f, (lightness, chroma, hue)).first();
                    g.call(OKLCH_GAMUT_MAP).input("oklch", lch).add().out("rgb")
                },
            );

            let chosen = g
                .match_on(&mode)
                .case(0, rgb)
                .case(1, hsv)
                .case(2, oklch)
                .add()
                .first();
            g.output("color", chosen, Output::new(WgslType::Vec3f).label("Color"));
            g.frame(Frame::new("Alpha"), || {
                let split = g.op("separate-vector", chosen);
                let alpha = g.input(
                    "alpha",
                    unit(Input::new(WgslType::F32).label("Alpha").default(1.0)),
                );
                let rgba = g.vec(
                    WgslType::Vec4f,
                    (split.out("x"), split.out("y"), split.out("z"), alpha),
                );
                g.output("rgba", rgba, Output::new(WgslType::Vec4f).label("RGBA"));
            });
        },
    )
    .aliases(&[
        "RGB",
        "RGBA",
        "Combine Color",
        "Combine RGB",
        "Combine HSV",
        "Color Constant",
        "Solid Color",
        "OKLch",
    ])
    .widget(color_widget)
}

fn color_widget(input: &InputIds<'_>) -> GroupWidget {
    let mode = |space: &str, components: [&str; 3]| ColorWidgetMode {
        space: space.to_owned(),
        components: components.map(|key| input.id(key)),
    };
    GroupWidget::Color {
        mode: input.id("mode"),
        modes: BTreeMap::from([
            ("0".to_owned(), mode("rgb", ["red", "green", "blue"])),
            ("1".to_owned(), mode("hsv", ["hue", "saturation", "value"])),
            (
                "2".to_owned(),
                mode("oklch", ["lightness", "chroma", "oklch-hue"]),
            ),
        ]),
    }
}
