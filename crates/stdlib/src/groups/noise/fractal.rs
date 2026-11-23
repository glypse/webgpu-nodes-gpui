//! Fractal noise (fBm): octaves of a noise, each finer and fainter, summed.

use shader_graph::types::WgslType;

use crate::builder::{
    Frame, GroupBuilder, GroupSpec, Input, Output, Ref, RepeatBody, RepeatItem, Repeated,
};
use crate::groups::helpers::{clamp_range, compare};

pub const FRACTAL_OCTAVE: &str = "std-fractal-octave";

/// The weight under which the octaves left of a fractal noise are dropped.
const INAUDIBLE: f64 = 1.0 / 4096.0;

#[derive(Clone, Copy)]
pub struct FractalInputs {
    pub detail: Ref,
    pub roughness: Ref,
    pub lacunarity: Ref,
}

/// The fractal inputs of a noise, with Blender's ranges and defaults.
pub fn fractal_inputs(g: &GroupBuilder<'_>) -> FractalInputs {
    FractalInputs {
        detail: g.input(
            "detail",
            Input::new(WgslType::F32)
                .label("Detail")
                .default(0.0)
                .min(0.0)
                .max(15.0),
        ),
        roughness: g.input(
            "roughness",
            Input::new(WgslType::F32)
                .label("Roughness")
                .default(0.5)
                .min(0.0)
                .max(1.0),
        ),
        lacunarity: g.input(
            "lacunarity",
            Input::new(WgslType::F32)
                .label("Lacunarity")
                .default(2.0)
                .min(0.0),
        ),
    }
}

/// An octave as `fractal` hands it over: its index, and `std-fractal-octave`'s outputs.
#[derive(Clone, Copy)]
pub struct Octave {
    pub iteration: Ref,
    pub scale: Ref,
    pub amplitude: Ref,
    pub power: Ref,
}

fn octave_body(g: &GroupBuilder<'_>) {
    let iteration = g.input(
        "iteration",
        Input::new(WgslType::I32).label("Octave").default(0),
    );
    let detail = clamp_range(
        g,
        g.input(
            "detail",
            Input::new(WgslType::F32)
                .label("Detail")
                .default(0.0)
                .min(0.0)
                .max(15.0),
        ),
        0.0,
        15.0,
    );
    let roughness = g.input(
        "roughness",
        Input::new(WgslType::F32)
            .label("Roughness")
            .default(0.5)
            .min(0.0)
            .max(1.0),
    );
    let lacunarity = g.input(
        "lacunarity",
        Input::new(WgslType::F32)
            .label("Lacunarity")
            .default(2.0)
            .min(0.0),
    );
    let octave = g.op("to-float", iteration).first();
    g.frame(
        Frame::new("Octave").note(
            "Its scale and weight (Amplitude); the octave past Detail's\nwhole part counts for its fraction, which Power leaves out.",
        ),
        || {
            let weight = clamp_range(g, g.op("add", (g.op("sub", (detail, octave)), 1.0)), 0.0, 1.0);
            let power = g.op("pow", (roughness, octave)).first();
            g.output(
                "scale",
                g.op("pow", (lacunarity, octave)),
                Output::new(WgslType::F32).label("Scale"),
            );
            g.output(
                "amplitude",
                g.op("mul", (power, weight)),
                Output::new(WgslType::F32).label("Amplitude"),
            );
            g.output("power", power, Output::new(WgslType::F32).label("Power"));
        },
    );
    g.frame(
        Frame::new("Octaves").note(
            "Detail's, stopping early once the weights of all the octaves left\n(at most Roughness^n / (1 - Roughness)) add up to under 1/4096.",
        ),
        || {
            let detailed = g
                .op("add", (g.op("to-int", g.op("ceil", detail)), g.constant(WgslType::I32, 1.0)))
                .first();
            let r = g.op("max", (roughness, 0.000001)).first();
            let decay = g.op("log", g.op("mul", (g.op("sub", (1.0, r)), INAUDIBLE)));
            let ratio = g.op("log", r);
            let needed = g
                .op("to-int", g.op("max", (g.op("ceil", g.op("div", (decay, ratio))), 1.0)))
                .first();
            let fading = compare(g, roughness, 1.0, "<");
            g.output(
                "octaves",
                g.op("switch", (fading, detailed, g.op("min", (detailed, needed)))),
                Output::new(WgslType::I32).label("Octaves"),
            );
        },
    );
}

/// Sums the octaves of a noise (fBm). `octave` gets each octave's scale and
/// amplitude (the last one weighted by Detail's fraction) and the items'
/// current values, and sets their next ones, as `GroupBuilder::repeat`.
pub fn fractal<'a>(
    g: &'a GroupBuilder<'a>,
    inputs: FractalInputs,
    items: Vec<RepeatItem>,
    mut octave: impl FnMut(&RepeatBody<'_>, Octave),
) -> Repeated<'a> {
    let octaves = g
        .call(FRACTAL_OCTAVE)
        .input("detail", inputs.detail)
        .input("roughness", inputs.roughness)
        .input("lacunarity", inputs.lacunarity)
        .add()
        .out("octaves");
    g.repeat(octaves, items, |body| {
        let iteration = body.iteration();
        let node = g
            .call(FRACTAL_OCTAVE)
            .input("iteration", iteration)
            .input("detail", inputs.detail)
            .input("roughness", inputs.roughness)
            .input("lacunarity", inputs.lacunarity)
            .add();
        octave(
            body,
            Octave {
                iteration,
                scale: node.out("scale"),
                amplitude: node.out("amplitude"),
                power: node.out("power"),
            },
        );
    })
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            FRACTAL_OCTAVE,
            "Fractal Octave",
            "texture",
            &["iteration", "detail", "roughness", "lacunarity"],
            &["scale", "amplitude", "power", "octaves"],
            octave_body,
        )
        .aliases(&["fBm Octave", "Octave", "Fractal Brownian Motion"])
        .helper(&[]),
    ]
}
