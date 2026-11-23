//! Blurs: each runs a one-dimensional kernel horizontally, renders it to a
//! texture, then runs it vertically on that and renders the result. With a
//! direction, the kernel runs once along an angle; unseparated, a
//! two-dimensional kernel does all of it in one go.

mod kernel;

use std::f64::consts::PI;

use shader_graph::types::WgslType;

use self::kernel::{BOX_BLUR_1D, BOX_BLUR_2D, GAUSSIAN_BLUR_1D, GAUSSIAN_BLUR_2D, edges};
use crate::builder::{EnumRef, Frame, GroupBuilder, GroupSpec, Input, Output, Ref};
use crate::groups::helpers::resolution;

pub const GAUSSIAN_BLUR: &str = "std-gaussian-blur";
pub const BOX_BLUR_ONCE: &str = "std-box-blur-once";
pub const BOX_BLUR: &str = "std-box-blur";

const BLUR_INPUTS: [&str; 6] = [
    "texture",
    "radius",
    "edges",
    "separable",
    "direction",
    "angle",
];

/// Render passes can't loop, so a box blur renders each iteration of its own,
/// up to this many.
const MAX_ITERATIONS: usize = 8;

struct BlurInputs {
    texture: Ref,
    radius: Ref,
    edges: Ref,
    direction: EnumRef,
    separable: EnumRef,
    angle: Ref,
}

fn blur_inputs(g: &GroupBuilder<'_>) -> BlurInputs {
    let texture = g.input("texture", Input::new(WgslType::Texture).label("Texture"));
    let t = g.type_variable("t", "T", &[WgslType::F32, WgslType::Vec2f]);
    let radius = g.input(
        "radius",
        Input::new(&t).label("Radius").default(0.01).min(0.0),
    );
    let edges = edges(g).value();
    let direction = g.enum_input("direction", Input::toggle(false).label("Direction"));
    let separable = g.enum_input(
        "separable",
        Input::toggle(true).label("Separable").owner(&direction, 0),
    );
    let angle = g.input(
        "angle",
        Input::new(WgslType::F32)
            .label("Angle")
            .default(0.0)
            .owner(&direction, 1),
    );
    BlurInputs {
        texture,
        radius,
        edges,
        direction,
        separable,
        angle,
    }
}

/// A blur by `along`, a one-dimensional kernel, or `around`, a two-dimensional one.
fn blur_body(g: &GroupBuilder<'_>, along: &str, around: &str) {
    let inputs = blur_inputs(g);
    let (pixels, canvas) = g.frame(
        Frame::new("Pixels").note(
            "The radius is a fraction of the canvas' height, like UV,\nso any resolution blurs alike: in pixels, per axis.",
        ),
        || {
            let canvas = resolution(g);
            let scaled = g.op("mul", (inputs.radius, g.constant(WgslType::Vec2f, 1.0)));
            let height = g.op("separate-vector", canvas).out("y");
            (g.op("mul", (scaled, height)).first(), canvas)
        },
    );
    let pixels_split = g.op("separate-vector", pixels);
    let (pixels_x, pixels_y) = (pixels_split.out("x"), pixels_split.out("y"));
    let step = g.op("div", (1.0, canvas)).first();
    let step_split = g.op("separate-vector", step);
    let (step_x, step_y) = (step_split.out("x"), step_split.out("y"));
    let edges = inputs.edges;
    let horizontal = g
        .call(along)
        .input("texture", inputs.texture)
        .input("direction", g.vec(WgslType::Vec2f, (step_x, 0.0)))
        .input("radius", pixels_x)
        .input("edges", edges)
        .add()
        .out("color");
    // The horizontal blur's colour, linked to a Texture input, is rendered first.
    let vertical = g
        .call(along)
        .input("texture", horizontal)
        .input("direction", g.vec(WgslType::Vec2f, (0.0, step_y)))
        .input("radius", pixels_y)
        .input("edges", edges)
        .add()
        .out("color");
    let exact = g
        .call(around)
        .input("texture", inputs.texture)
        .input("step", step)
        .input("radius", pixels)
        .input("edges", edges)
        .add()
        .out("color");
    let along_angle = g.frame(
        Frame::new("Along the angle").note(
            "A pixel's step along the angle, in degrees from the x axis,\nand the radius that way: per axis radii make an ellipse.",
        ),
        || {
            let radians = g.op("mul", (inputs.angle, PI / 180.0)).first();
            let unit = g
                .vec(WgslType::Vec2f, (g.op("cos", radians), g.op("sin", radians)))
                .first();
            g.call(along)
                .input("texture", inputs.texture)
                .input("direction", g.op("div", (unit, canvas)))
                .input("radius", g.op("length", g.op("mul", (pixels, unit))))
                .input("edges", edges)
                .add()
                .out("color")
        },
    );
    let both_axes = g
        .match_on(&inputs.separable)
        .case(0, exact)
        .case(1, vertical)
        .add()
        .first();
    let color = g
        .match_on(&inputs.direction)
        .case(0, both_axes)
        .case(1, along_angle)
        .add()
        .first();
    g.output(
        "texture",
        color,
        Output::new(WgslType::Texture).label("Texture"),
    );
}

fn box_blur_body(g: &GroupBuilder<'_>) {
    let settings = blur_inputs(g);
    let texture = settings.texture;
    let iterations = g.input(
        "iterations",
        Input::new(WgslType::I32)
            .label("Iterations")
            .default(1)
            .min(1.0)
            .max(MAX_ITERATIONS as f64)
            .compiled(),
    );
    let mut stages = vec![texture];
    let mut previous = texture;
    for _ in 0..MAX_ITERATIONS {
        let blurred = g
            .call(BOX_BLUR_ONCE)
            .input("texture", previous)
            .input("radius", settings.radius)
            .input("edges", settings.edges)
            .input("separable", settings.separable.value())
            .input("direction", settings.direction.value())
            .input("angle", settings.angle)
            .add()
            .out("texture");
        stages.push(blurred);
        previous = blurred;
    }
    let mut switch = g.index_switch(iterations, WgslType::Texture);
    for (index, stage) in stages.into_iter().enumerate() {
        switch = switch.case(index as i64, stage);
    }
    g.output(
        "texture",
        switch.add().first(),
        Output::new(WgslType::Texture).label("Texture"),
    );
}

pub fn all() -> Vec<GroupSpec> {
    let mut groups = kernel::all();
    groups.extend([
        GroupSpec::new(
            GAUSSIAN_BLUR,
            "Gaussian Blur",
            "texture",
            &BLUR_INPUTS,
            &["texture"],
            |g| blur_body(g, GAUSSIAN_BLUR_1D, GAUSSIAN_BLUR_2D),
        )
        .aliases(&["Blur", "Soften", "Defocus", "Gaussian"])
        .section(&["Blur"]),
        GroupSpec::new(
            BOX_BLUR_ONCE,
            "Box Blur Once",
            "texture",
            &BLUR_INPUTS,
            &["texture"],
            |g| blur_body(g, BOX_BLUR_1D, BOX_BLUR_2D),
        )
        .helper(&["Blur"]),
        GroupSpec::new(
            BOX_BLUR,
            "Box Blur",
            "texture",
            &[
                "texture",
                "radius",
                "edges",
                "separable",
                "direction",
                "angle",
                "iterations",
            ],
            &["texture"],
            box_blur_body,
        )
        .aliases(&["Blur", "Mean Blur", "Average"])
        .section(&["Blur"]),
    ]);
    groups
}
