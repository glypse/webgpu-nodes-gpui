//! How Voronoi measures: distances between points and to the bisector of
//! two, in each of its metrics.

use shader_graph::types::WgslType;

use crate::builder::{EnumRef, Frame, GroupBuilder, GroupSpec, Input, Output, Ref, RepeatItem};
use crate::groups::helpers::{compare, component_sum};
use crate::groups::noise::coordinates::{coordinates_input, coordinates_type};
use crate::groups::vector::{MAX_COMPONENT, PAD_VECTOR};

pub const VORONOI_DISTANCE: &str = "std-voronoi-distance";
pub const VORONOI_BISECTOR: &str = "std-voronoi-bisector";

const EUCLIDEAN: i64 = 0;
const MANHATTAN: i64 = 1;
const CHEBYSHEV: i64 = 2;
const MINKOWSKI: i64 = 3;

/// The metric choice and its exponent, on the groups measuring with it.
#[derive(Clone)]
pub struct MetricInputs {
    pub metric: EnumRef,
    pub exponent: Ref,
}

pub fn metric_inputs(g: &GroupBuilder<'_>) -> MetricInputs {
    let metric = g.enum_input(
        "metric",
        Input::enumeration(
            [
                crate::builder::Variant::new(EUCLIDEAN, "Euclidean"),
                crate::builder::Variant::new(MANHATTAN, "Manhattan"),
                crate::builder::Variant::new(CHEBYSHEV, "Chebyshev"),
                crate::builder::Variant::new(MINKOWSKI, "Minkowski"),
            ],
            EUCLIDEAN,
        )
        .label("Metric"),
    );
    let exponent = g.input(
        "exponent",
        Input::new(WgslType::F32)
            .label("Exponent")
            .default(0.5)
            .min(0.0)
            .max(32.0)
            .owner(&metric, MINKOWSKI),
    );
    MetricInputs { metric, exponent }
}

/// How far a point moved by up to `reach` along every axis can be, in the
/// metric's length: the factor multiplying `reach` for `dimensions` axes.
pub fn across_in_cell(g: &GroupBuilder<'_>, metric: &MetricInputs, dimensions: Ref) -> Ref {
    let n = g.op("to-float", dimensions).first();
    let power = g.op("pow", (n, g.op("div", (1.0, metric.exponent))));
    g.match_on(&metric.metric)
        .case(EUCLIDEAN, g.op("sqrt", n))
        .case(MANHATTAN, n)
        .case(CHEBYSHEV, 1.0)
        .case(MINKOWSKI, power)
        .add()
        .first()
}

/// The distance between two points in the metric.
fn voronoi_distance_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let a = g.input("a", Input::new(&t).label("A"));
    let b = g.input("b", Input::new(&t).label("B"));
    let metric = metric_inputs(g);
    let d = g.op("abs", g.op("sub", (a, b))).first();
    let distance = g
        .match_on(&metric.metric)
        .case(EUCLIDEAN, g.op("length", d))
        .case(MANHATTAN, component_sum(g, d))
        .case(
            CHEBYSHEV,
            g.call(MAX_COMPONENT).input("vector", d).add().out("max"),
        )
        .case(
            MINKOWSKI,
            g.op(
                "pow",
                (
                    component_sum(g, g.op("pow", (d, metric.exponent))),
                    g.op("div", (1.0, metric.exponent)),
                ),
            ),
        )
        .add()
        .first();
    g.output(
        "distance",
        distance,
        Output::new(WgslType::F32).label("Distance"),
    );
}

/// The distance from a point to the bisector of two points (A and B), in the metric.
fn voronoi_bisector_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let a = g.input("a", Input::new(&t).label("A"));
    let b = g.input("b", Input::new(&t).label("B"));
    let metric = metric_inputs(g);
    let dimensions = g.op("component-count", a).first();
    let int = |n: f64| g.constant(WgslType::I32, n).first();
    let distance_to = |point: Ref, other: f64| {
        g.call(VORONOI_DISTANCE)
            .input("a", point)
            .input("b", other)
            .input("metric", metric.metric.value())
            .input("exponent", metric.exponent)
            .add()
            .out("distance")
    };
    let to_a = distance_to(a, 0.0);
    let to_b = distance_to(b, 0.0);
    let (near, far) = g.frame(
        Frame::new("Sides").note(
            "A and B relative to the position: which of them it's closer to,\npadded with zeros for the axis-by-axis metrics.",
        ),
        || {
            let swapped = compare(g, to_b, to_a, "<");
            let pad = |vector: Ref| {
                g.call(PAD_VECTOR)
                    .input("vector", vector)
                    .add()
                    .out("padded")
            };
            let near = pad(g.op("switch", (swapped, a, b)).first());
            let far = pad(g.op("switch", (swapped, b, a)).first());
            (near, far)
        },
    );
    let euclidean = g.frame(
        Frame::new("Euclidean").note("Straight: the distance to the plane half-way between them."),
        || {
            let across = g.op("sub", (b, a)).first();
            let midpoint = g.op("mul", (g.op("add", (a, b)), across, 0.5));
            let numerator = g.op("abs", component_sum(g, midpoint));
            g.op("div", (numerator, g.op("length", across))).first()
        },
    );
    let manhattan = g.frame(
        Frame::new("Manhattan").note(
            "For each sign pattern σ the far point is ahead of the near one along,\n(|far|₁ + σ·near) / 2: the least is the distance.",
        ),
        || {
            let patterns = g
                .width_switch(a, (int(2.0), int(4.0), int(8.0), int(16.0)))
                .first();
            let far_length = g.op("dot", (g.op("abs", far), 1.0)).first();
            g.repeat(
                patterns,
                vec![RepeatItem::new("Distance", WgslType::F32, 1e10)],
                |body| {
                    let bits = g
                        .op(
                            "mod",
                            (
                                g.op(
                                    "div",
                                    (
                                        body.iteration(),
                                        g.vec(WgslType::Vec4i, (1.0, 2.0, 4.0, 8.0)),
                                    ),
                                ),
                                int(2.0),
                            ),
                        )
                        .first();
                    let sigma = g
                        .op("sub", (g.op("mul", (g.op("to-float", bits), 2.0)), 1.0))
                        .first();
                    let ahead = compare(g, g.op("dot", (sigma, g.op("sub", (far, near)))), 0.0, ">=");
                    let d = g
                        .op("mul", (g.op("add", (far_length, g.op("dot", (sigma, near)))), 0.5))
                        .first();
                    let closest = g.op("min", (body.state("Distance"), d));
                    body.set(
                        "Distance",
                        g.op("switch", (ahead, body.state("Distance"), closest)),
                    );
                },
            )
            .get("Distance")
        },
    );
    let chebyshev = g.frame(
        Frame::new("Chebyshev").note(
            "For each axis and direction the far point isn't behind on, how far\nto half-way along it, the other axes keeping up: the least is the distance.",
        ),
        || {
            let sides = g.op("mul", (g.op("to-int", dimensions), int(2.0))).first();
            g.repeat(
                sides,
                vec![RepeatItem::new("Distance", WgslType::F32, 1e10)],
                |body| {
                    let axis = g.op("div", (body.iteration(), int(2.0))).first();
                    let sign = g
                        .op(
                            "sub",
                            (
                                1.0,
                                g.op(
                                    "mul",
                                    (g.op("to-float", g.op("mod", (body.iteration(), int(2.0)))), 2.0),
                                ),
                            ),
                        )
                        .first();
                    let offsets = g.vec(WgslType::Vec4i, (0.0, 1.0, 2.0, 3.0));
                    let along = g
                        .op(
                            "sub",
                            (
                                1.0,
                                g.op(
                                    "min",
                                    (
                                        g.op("abs", g.op("to-float", g.op("sub", (offsets, axis)))),
                                        1.0,
                                    ),
                                ),
                            ),
                        )
                        .first();
                    let near_k = g.op("dot", (near, along)).first();
                    let far_k = g.op("dot", (far, along)).first();
                    let ahead = compare(
                        g,
                        g.op("mul", (sign, g.op("sub", (far_k, near_k)))),
                        0.0,
                        ">=",
                    );
                    let split = g.op(
                        "separate-vector",
                        g.op("mul", (g.op("abs", far), g.op("sub", (1.0, along)))),
                    );
                    let largest = g
                        .op(
                            "max",
                            (split.out("x"), split.out("y"), split.out("z"), split.out("w")),
                        )
                        .first();
                    let others = g
                        .op(
                            "mul",
                            (g.op("add", (largest, g.op("mul", (sign, near_k)))), 0.5),
                        )
                        .first();
                    let d = g
                        .op(
                            "max",
                            (
                                g.op("mul", (sign, g.op("add", (near_k, far_k)), 0.5)),
                                others,
                                0.0,
                            ),
                        )
                        .first();
                    let closest = g.op("min", (body.state("Distance"), d));
                    body.set(
                        "Distance",
                        g.op("switch", (ahead, body.state("Distance"), closest)),
                    );
                },
            )
            .get("Distance")
        },
    );
    let minkowski = g.frame(
        Frame::new("Minkowski").note(
            "No closed form: half the gap between the two distances,\nexact on the edge itself, approximate away from it.",
        ),
        || g.op("mul", (g.op("abs", g.op("sub", (to_b, to_a))), 0.5)).first(),
    );
    let distance = g
        .match_on(&metric.metric)
        .case(EUCLIDEAN, euclidean)
        .case(MANHATTAN, manhattan)
        .case(CHEBYSHEV, chebyshev)
        .case(MINKOWSKI, minkowski)
        .add()
        .first();
    g.output(
        "distance",
        distance,
        Output::new(WgslType::F32).label("Distance"),
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            VORONOI_DISTANCE,
            "Voronoi Distance",
            "texture",
            &["a", "b", "metric", "exponent"],
            &["distance"],
            voronoi_distance_body,
        )
        .aliases(&[
            "Distance Metric",
            "Minkowski Distance",
            "Manhattan Distance",
            "Chebyshev Distance",
        ])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            VORONOI_BISECTOR,
            "Voronoi Bisector Distance",
            "texture",
            &["a", "b", "metric", "exponent"],
            &["distance"],
            voronoi_bisector_body,
        )
        .aliases(&["Bisector", "Distance to Bisector", "Cell Border Distance"])
        .helper(&["Voronoi"]),
    ]
}
