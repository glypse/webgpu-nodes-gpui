//! Blur kernels: a weighted sum of a texture's pixels, along a direction for
//! the one-dimensional ones a separable blur runs once per axis, or over a
//! rectangle for the two-dimensional ones.

use shader_graph::nodes::extensions;
use shader_graph::types::{InputValue, WgslType};
use shader_graph::zones::DEFAULT_MAX_ITERATIONS;

use crate::builder::{
    EnumRef, Frame, GroupBuilder, GroupSpec, Input, Output, Ref, RepeatItem, Variant,
};
use crate::groups::helpers::uv;

pub const GAUSSIAN_BLUR_1D: &str = "std-gaussian-blur-1d";
pub const BOX_BLUR_1D: &str = "std-box-blur-1d";
pub const GAUSSIAN_BLUR_2D: &str = "std-gaussian-blur-2d";
pub const BOX_BLUR_2D: &str = "std-box-blur-2d";

/// The most taps on either side of the centre a repeat zone's iterations can run.
const MAX_REACH: f64 = ((DEFAULT_MAX_ITERATIONS - 1) / 2) as f64;

/// The most taps on either side of the centre, per axis, of a two-dimensional
/// kernel: their square is what each pixel reads, which a GPU must get through
/// before the browser takes it for hung.
const MAX_REACH_2D: f64 = 64.0;

/// The edge modes a blur reads its taps with.
pub fn edges(g: &GroupBuilder<'_>) -> EnumRef {
    g.enum_input(
        "edges",
        Input::enumeration(
            extensions()
                .into_iter()
                .map(|mode| Variant::new(mode.value, mode.label)),
            0,
        )
        .label("Edges"),
    )
}

#[derive(Clone, Copy)]
pub enum Profile {
    Gaussian,
    Box,
}

/// The weight of each tap of a kernel, its constants computed once per kernel.
#[derive(Clone, Copy)]
enum Weights {
    Gaussian { two_variance: Ref, floor: Ref },
    Box { taps: Ref },
}

impl Weights {
    /// Three standard deviations reach the radius, where weights fall to about
    /// 1%. The floor lowers them to reach 0 one tap past it, or each tap the
    /// kernel gains would show as a step where the radius varies.
    fn new(g: &GroupBuilder<'_>, profile: Profile, taps: Ref) -> Self {
        match profile {
            Profile::Gaussian => g.frame(
                Frame::new("Spread")
                    .note("σ = radius / 3, as 2σ², and the weight one tap past the radius."),
                || {
                    let sigma = g.op("max", (g.op("div", (taps, 3.0)), 0.0001)).first();
                    let variance = g.op("mul", (sigma, sigma)).first();
                    let two_variance = g.op("mul", (variance, 2.0)).first();
                    let past = g.op("add", (taps, 1.0)).first();
                    let squared = g.op("mul", (past, past)).first();
                    let falloff = g.op("negate", g.op("div", (squared, two_variance))).first();
                    let floor = g.op("exp", falloff).first();
                    Self::Gaussian {
                        two_variance,
                        floor,
                    }
                },
            ),
            Profile::Box => Self::Box { taps },
        }
    }

    /// The weight of tap `k`, a signed tap index.
    fn at(self, g: &GroupBuilder<'_>, k: Ref) -> Ref {
        match self {
            Self::Gaussian {
                two_variance,
                floor,
            } => {
                let squared = g.op("mul", (k, k)).first();
                let falloff = g.op("negate", g.op("div", (squared, two_variance))).first();
                let weight = g.op("exp", falloff).first();
                g.op("max", (g.op("sub", (weight, floor)), 0.0)).first()
            }
            // Equal weights out to the radius, the outermost taps weighted by its
            // fraction, so a radius growing smoothly blurs smoothly.
            Self::Box { taps } => {
                let reach = g.op("add", (taps, 1.0)).first();
                g.op("clamp", (g.op("sub", (reach, g.op("abs", k))), 0.0, 1.0))
                    .first()
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Spacing {
    stride: Ref,
    taps: Ref,
}

/// The spacing of a kernel's taps, in pixels, and its radius counted in taps.
/// Taps are a pixel apart until the radius outgrows `reach`, then spread out
/// evenly, so the kernel stays whole and changes smoothly with the radius.
fn tap_spacing(g: &GroupBuilder<'_>, radius: Ref, reach: f64) -> Spacing {
    g.frame(
        Frame::new("Spacing")
            .note("One pixel apart, or further once the radius needs more taps than it's given."),
        || {
            let stride = g.op("max", (g.op("div", (radius, reach)), 1.0)).first();
            let taps = g.op("div", (g.op("max", (radius, 0.0)), stride)).first();
            Spacing { stride, taps }
        },
    )
}

/// `texture`'s colour at `at`, premultiplied by its alpha: else transparent
/// pixels would darken their neighbours.
fn premultiplied_tap(g: &GroupBuilder<'_>, texture: Ref, at: Ref, edges: Ref) -> Ref {
    let color = g
        .op("sample-texture", (texture, at, 0, edges, 0))
        .out("color");
    let split = g.op("separate-vector", color);
    let alpha = split.out("w");
    g.vec(
        WgslType::Vec4f,
        (
            g.op("mul", (split.out("x"), alpha)),
            g.op("mul", (split.out("y"), alpha)),
            g.op("mul", (split.out("z"), alpha)),
            alpha,
        ),
    )
    .first()
}

/// The straight colour of a premultiplied sum of taps weighing `total`.
fn normalised(g: &GroupBuilder<'_>, sum: Ref, total: Ref) -> Ref {
    g.frame(
        Frame::new("Normalised")
            .note("The sum over the weights' sum, its colour divided by its alpha again."),
        || {
            let split = g.op("separate-vector", g.op("div", (sum, total)));
            let alpha = split.out("w");
            let straight = |c: Ref| g.op("div", (c, alpha)).first();
            g.vec(
                WgslType::Vec4f,
                (
                    straight(split.out("x")),
                    straight(split.out("y")),
                    straight(split.out("z")),
                    alpha,
                ),
            )
            .first()
        },
    )
}

/// Taps `-n..=n` along `direction` (one pixel in UV apart, scaled by the
/// stride), `n` the taps' reach rounded up.
fn build_1d(g: &GroupBuilder<'_>, profile: Profile) {
    let texture = g.input("texture", Input::new(WgslType::Texture).label("Texture"));
    let direction = g.input(
        "direction",
        Input::new(WgslType::Vec2f)
            .label("Direction")
            .default(InputValue::vector(&[0.0, 0.0])),
    );
    let radius = g.input(
        "radius",
        Input::new(WgslType::F32)
            .label("Radius")
            .default(0.0)
            .min(0.0),
    );
    let edges = edges(g).value();
    let spacing = tap_spacing(g, radius, MAX_REACH);
    let weight = Weights::new(g, profile, spacing.taps);
    let reach = g.op("ceil", spacing.taps).first();
    let taps = g.frame(
        Frame::new("Taps")
            .note("One sample per tap along the direction, weighted,\nits colour premultiplied by its alpha."),
        || {
            g.repeat(
                g.op("add", (g.op("mul", (reach, 2.0)), 1.0)).first(),
                vec![
                    RepeatItem::new("sum", WgslType::Vec4f, g.constant(WgslType::Vec4f, 0.0)),
                    RepeatItem::new("total", WgslType::F32, 0.0),
                ],
                |body| {
                    let k = g.op("sub", (g.op("to-float", body.iteration()), reach)).first();
                    let step = g.op("mul", (k, spacing.stride));
                    let at = g.op("add", (uv(g), g.op("mul", (direction, step)))).first();
                    let w = weight.at(g, k);
                    let tap = g.op("mul", (premultiplied_tap(g, texture, at, edges), w));
                    body.set("sum", g.op("add", (body.state("sum"), tap)));
                    body.set("total", g.op("add", (body.state("total"), w)));
                },
            )
        },
    );
    g.output(
        "color",
        normalised(g, taps.get("sum"), taps.get("total")),
        Output::new(WgslType::Vec4f).label("Color"),
    );
}

/// The rectangle of taps around each pixel, every tap read at once, so it takes
/// that pixel's radius wherever it reads, where a separable blur's second pass
/// reads what its first computed with other pixels' radii. `step` is a pixel's
/// size in UV.
fn build_2d(g: &GroupBuilder<'_>, profile: Profile) {
    let texture = g.input("texture", Input::new(WgslType::Texture).label("Texture"));
    let step = g.input(
        "step",
        Input::new(WgslType::Vec2f)
            .label("Step")
            .default(InputValue::vector(&[0.0, 0.0])),
    );
    let radius = g.input(
        "radius",
        Input::new(WgslType::Vec2f)
            .label("Radius")
            .default(InputValue::vector(&[0.0, 0.0]))
            .min(0.0),
    );
    let edges = edges(g).value();
    let radius_split = g.op("separate-vector", radius);
    let spacing_x = tap_spacing(g, radius_split.out("x"), MAX_REACH_2D);
    let spacing_y = tap_spacing(g, radius_split.out("y"), MAX_REACH_2D);
    let weight_x = Weights::new(g, profile, spacing_x.taps);
    let weight_y = Weights::new(g, profile, spacing_y.taps);
    let reach_x = g.op("ceil", spacing_x.taps).first();
    let reach_y = g.op("ceil", spacing_y.taps).first();
    let taps = g.frame(
        Frame::new("Taps").note(
            "A row of taps per tap along y, each weighted by its weights\nalong both axes, its colour premultiplied by its alpha.",
        ),
        || {
            g.repeat(
                g.op("add", (g.op("mul", (reach_y, 2.0)), 1.0)).first(),
                vec![
                    RepeatItem::new("sum", WgslType::Vec4f, g.constant(WgslType::Vec4f, 0.0)),
                    RepeatItem::new("total", WgslType::F32, 0.0),
                ],
                |rows| {
                    let ky = g.op("sub", (g.op("to-float", rows.iteration()), reach_y)).first();
                    let wy = weight_y.at(g, ky);
                    let offset_y = g.op("mul", (ky, spacing_y.stride)).first();
                    let row = g.repeat(
                        g.op("add", (g.op("mul", (reach_x, 2.0)), 1.0)).first(),
                        vec![
                            RepeatItem::new("sum", WgslType::Vec4f, rows.state("sum")),
                            RepeatItem::new("total", WgslType::F32, rows.state("total")),
                        ],
                        |columns| {
                            let kx = g
                                .op("sub", (g.op("to-float", columns.iteration()), reach_x))
                                .first();
                            let offset = g.vec(
                                WgslType::Vec2f,
                                (g.op("mul", (kx, spacing_x.stride)), offset_y),
                            );
                            let at = g.op("add", (uv(g), g.op("mul", (step, offset)))).first();
                            let w = g.op("mul", (weight_x.at(g, kx), wy)).first();
                            let tap = g.op("mul", (premultiplied_tap(g, texture, at, edges), w));
                            columns.set("sum", g.op("add", (columns.state("sum"), tap)));
                            columns.set("total", g.op("add", (columns.state("total"), w)));
                        },
                    );
                    rows.set("sum", row.get("sum"));
                    rows.set("total", row.get("total"));
                },
            )
        },
    );
    g.output(
        "color",
        normalised(g, taps.get("sum"), taps.get("total")),
        Output::new(WgslType::Vec4f).label("Color"),
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            GAUSSIAN_BLUR_1D,
            "Gaussian Blur 1D",
            "texture",
            &["texture", "direction", "radius", "edges"],
            &["color"],
            |g| build_1d(g, Profile::Gaussian),
        )
        .aliases(&["Gaussian Kernel", "Directional Gaussian Blur"])
        .helper(&["Blur"]),
        GroupSpec::new(
            BOX_BLUR_1D,
            "Box Blur 1D",
            "texture",
            &["texture", "direction", "radius", "edges"],
            &["color"],
            |g| build_1d(g, Profile::Box),
        )
        .aliases(&["Box Kernel", "Mean Filter"])
        .helper(&["Blur"]),
        GroupSpec::new(
            GAUSSIAN_BLUR_2D,
            "Gaussian Blur 2D",
            "texture",
            &["texture", "step", "radius", "edges"],
            &["color"],
            |g| build_2d(g, Profile::Gaussian),
        )
        .aliases(&["Gaussian Kernel", "Exact Gaussian Blur"])
        .helper(&["Blur"]),
        GroupSpec::new(
            BOX_BLUR_2D,
            "Box Blur 2D",
            "texture",
            &["texture", "step", "radius", "edges"],
            &["color"],
            |g| build_2d(g, Profile::Box),
        )
        .aliases(&["Box Kernel", "Exact Box Blur"])
        .helper(&["Blur"]),
    ]
}
