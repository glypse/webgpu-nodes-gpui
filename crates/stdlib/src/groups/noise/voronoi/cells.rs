//! Voronoi's cells: one point per lattice cell, moved at random, and the
//! searches through the cells around a position.

use shader_graph::types::WgslType;

use super::metric::{MetricInputs, VORONOI_BISECTOR, VORONOI_DISTANCE, metric_inputs};
use crate::builder::{Frame, GroupBuilder, GroupSpec, Input, Output, Ref, RepeatItem, TypeVar};
use crate::groups::helpers::compare;
use crate::groups::noise::coordinates::coordinates_type;
use crate::groups::noise::lattice::{HASH, LATTICE_NEIGHBOR};

pub const VORONOI_POINT: &str = "std-voronoi-point";
pub const VORONOI_NEAREST: &str = "std-voronoi-nearest";
pub const VORONOI_EDGES: &str = "std-voronoi-edges";
pub const VORONOI_RADIUS: &str = "std-voronoi-radius";
pub const VORONOI_SMOOTH: &str = "std-voronoi-smooth";

/// XORed into the seed for the cells' colours, so they don't follow their points.
pub const COLOR_SALT: f64 = 1540483477.0;

/// Hash's components in [0, 1] for the cell at `cell` (a float vector of whole numbers).
pub fn cell_hash(g: &GroupBuilder<'_>, cell: Ref, seed: Ref) -> Ref {
    g.call(HASH)
        .input("value", g.op("to-int", cell))
        .input("seed", seed)
        .add()
        .out("hash")
}

/// How the points of the cells around a cell are placed.
#[derive(Clone, Copy)]
pub struct PointInputs {
    pub cell: Ref,
    pub randomness: Ref,
    pub seed: Ref,
}

fn point_inputs(g: &GroupBuilder<'_>, cell: Ref) -> PointInputs {
    PointInputs {
        cell,
        randomness: g.input(
            "randomness",
            Input::new(WgslType::F32).label("Randomness").default(1.0),
        ),
        seed: g.input("seed", Input::new(WgslType::I32).label("Seed").default(0)),
    }
}

/// The inputs of the helpers searching the cells around a position: the cell
/// it's in, where it is in that cell, and how its points are placed.
fn cell_inputs(g: &GroupBuilder<'_>, t: &TypeVar) -> (Ref, PointInputs) {
    let cell = g.input("cell", Input::new(t).label("Cell"));
    let local = g.input("local", Input::new(t).label("Local Position"));
    (local, point_inputs(g, cell))
}

fn point_at(g: &GroupBuilder<'_>, p: &PointInputs, offset: Ref) -> Ref {
    g.call(VORONOI_POINT)
        .input("cell", p.cell)
        .input("offset", offset)
        .input("randomness", p.randomness)
        .input("seed", p.seed)
        .add()
        .out("point")
}

/// How many cells lie within `radius` of a cell, in the neighbours' lattice.
fn neighbor_count(g: &GroupBuilder<'_>, p: &PointInputs, radius: f64) -> Ref {
    g.call(LATTICE_NEIGHBOR)
        .input("cell", p.cell)
        .input("radius", radius)
        .add()
        .out("count")
}

/// The offset and point of the neighbour at `index` within `radius`.
fn neighbor_point(g: &GroupBuilder<'_>, p: &PointInputs, index: Ref, radius: f64) -> (Ref, Ref) {
    let offset = g
        .call(LATTICE_NEIGHBOR)
        .input("index", index)
        .input("cell", p.cell)
        .input("radius", radius)
        .add()
        .out("offset");
    (offset, point_at(g, p, offset))
}

fn voronoi_point_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let cell = g.input("cell", Input::new(&t).label("Cell"));
    let offset = g.input("offset", Input::new(&t).label("Offset"));
    let randomness = g.input(
        "randomness",
        Input::new(WgslType::F32).label("Randomness").default(1.0),
    );
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    let neighbor = g.op("add", (cell, offset)).first();
    let jitter = cell_hash(g, neighbor, seed);
    let point = g.op("add", (offset, g.op("mul", (jitter, randomness))));
    g.output("point", point, Output::new(&t).label("Point"));
    g.output("neighbor", neighbor, Output::new(&t).label("Neighbor Cell"));
}

/// The F1 and F2 points (and their offsets from the cell) nearest a position.
fn voronoi_nearest_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let (local, p) = cell_inputs(g, &t);
    let metric = metric_inputs(g);
    let closest = g.frame(
        Frame::new("Closest two points").note(
            "Every point of the 3×3(×3×3) cells around: the closest is F1,\nthe one after it F2.",
        ),
        || {
            g.repeat(
                neighbor_count(g, &p, 1.0),
                vec![
                    RepeatItem::new("F1 Distance", WgslType::F32, 1e10),
                    RepeatItem::new("F2 Distance", WgslType::F32, 1e10),
                    RepeatItem::new("F1 Point", &t, 0.0),
                    RepeatItem::new("F2 Point", &t, 0.0),
                    RepeatItem::new("F1 Offset", &t, 0.0),
                    RepeatItem::new("F2 Offset", &t, 0.0),
                ],
                |body| {
                    let (offset, point) = neighbor_point(g, &p, body.iteration(), 1.0);
                    let d = distance(g, point, local, &metric);
                    let f1_distance = body.state("F1 Distance");
                    let f2_distance = body.state("F2 Distance");
                    let closer1 = compare(g, d, f1_distance, "<");
                    let closer2 = compare(g, d, f2_distance, "<");
                    let f1_point = body.state("F1 Point");
                    let f2_point = body.state("F2 Point");
                    let f1_offset = body.state("F1 Offset");
                    let f2_offset = body.state("F2 Offset");
                    body.set("F1 Distance", g.op("min", (f1_distance, d)));
                    body.set(
                        "F2 Distance",
                        g.op(
                            "switch",
                            (closer1, g.op("min", (f2_distance, d)), f1_distance),
                        ),
                    );
                    body.set("F1 Point", g.op("switch", (closer1, f1_point, point)));
                    body.set(
                        "F2 Point",
                        g.op(
                            "switch",
                            (
                                closer1,
                                g.op("switch", (closer2, f2_point, point)),
                                f1_point,
                            ),
                        ),
                    );
                    body.set("F1 Offset", g.op("switch", (closer1, f1_offset, offset)));
                    body.set(
                        "F2 Offset",
                        g.op(
                            "switch",
                            (
                                closer1,
                                g.op("switch", (closer2, f2_offset, offset)),
                                f1_offset,
                            ),
                        ),
                    );
                },
            )
        },
    );
    g.output(
        "f1-distance",
        closest.get("F1 Distance"),
        Output::new(WgslType::F32).label("F1 Distance"),
    );
    g.output(
        "f2-distance",
        closest.get("F2 Distance"),
        Output::new(WgslType::F32).label("F2 Distance"),
    );
    g.output(
        "f1-point",
        closest.get("F1 Point"),
        Output::new(&t).label("F1 Point"),
    );
    g.output(
        "f2-point",
        closest.get("F2 Point"),
        Output::new(&t).label("F2 Point"),
    );
    g.output(
        "f1-offset",
        closest.get("F1 Offset"),
        Output::new(&t).label("F1 Offset"),
    );
    g.output(
        "f2-offset",
        closest.get("F2 Offset"),
        Output::new(&t).label("F2 Offset"),
    );
}

/// The distance between `a` and `b` in the metric.
fn distance(g: &GroupBuilder<'_>, a: Ref, b: Ref, metric: &MetricInputs) -> Ref {
    g.call(VORONOI_DISTANCE)
        .input("a", a)
        .input("b", b)
        .input("metric", metric.metric.value())
        .input("exponent", metric.exponent)
        .add()
        .out("distance")
}

fn voronoi_edges_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let (local, p) = cell_inputs(g, &t);
    let metric = metric_inputs(g);
    let f1_point = g.input("f1-point", Input::new(&t).label("F1 Point"));
    let f1_offset = g.input("f1-offset", Input::new(&t).label("F1 Offset"));
    let f2_point = g.input("f2-point", Input::new(&t).label("F2 Point"));
    let f2_offset = g.input("f2-offset", Input::new(&t).label("F2 Offset"));
    // Looking around the point's own cell rather than the position's: the
    // position's neighbourhood would see other points across each cell line,
    // and the edges would jump there.
    let closest_bisector = |point: Ref, offset: Ref| -> Ref {
        let from = g.op("sub", (point, local)).first();
        g.repeat(
            neighbor_count(g, &p, 2.0),
            vec![RepeatItem::new("Distance", WgslType::F32, 1e10)],
            |body| {
                let neighbor = g
                    .call(LATTICE_NEIGHBOR)
                    .input("index", body.iteration())
                    .input("cell", p.cell)
                    .input("radius", 2.0)
                    .add()
                    .out("offset");
                let to_point = g
                    .op(
                        "sub",
                        (
                            point_at(g, &p, g.op("add", (offset, neighbor)).first()),
                            local,
                        ),
                    )
                    .first();
                let gap = g.op("length", g.op("sub", (to_point, from)));
                let distinct = compare(g, gap, 0.01, ">");
                let d = g
                    .call(VORONOI_BISECTOR)
                    .input("a", from)
                    .input("b", to_point)
                    .input("metric", metric.metric.value())
                    .input("exponent", metric.exponent)
                    .add()
                    .out("distance");
                let distance = body.state("Distance");
                body.set(
                    "Distance",
                    g.op("switch", (distinct, distance, g.op("min", (distance, d)))),
                );
            },
        )
        .get("Distance")
    };
    g.frame(
        Frame::new("F1").note(
            "The bisectors of the F1 point and the points around it,\nin the metric: the edges of its cell.",
        ),
        || {
            let edge = closest_bisector(f1_point, f1_offset);
            g.output("edge", edge, Output::new(WgslType::F32).label("Distance to Edge"));
        },
    );
    g.frame(
        Frame::new("F2")
            .note("The same around the F2 point, F1's bisector included\nfrom its far side."),
        || {
            let f2_edge = closest_bisector(f2_point, f2_offset);
            g.output(
                "f2-edge",
                f2_edge,
                Output::new(WgslType::F32).label("F2 Distance to Edge"),
            );
        },
    );
}

fn voronoi_radius_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let cell = g.input("cell", Input::new(&t).label("Cell"));
    let p = point_inputs(g, cell);
    let metric = metric_inputs(g);
    let f1_point = g.input("f1-point", Input::new(&t).label("F1 Point"));
    let f1_offset = g.input("f1-offset", Input::new(&t).label("F1 Offset"));
    let sphere = g.frame(
        Frame::new("Closest point to F1")
            .note("The points of the cells around F1's, its own aside, in the metric."),
        || {
            g.repeat(
                neighbor_count(g, &p, 1.0),
                vec![RepeatItem::new("Distance", WgslType::F32, 1e10)],
                |body| {
                    let offset = g
                        .call(LATTICE_NEIGHBOR)
                        .input("index", body.iteration())
                        .input("cell", p.cell)
                        .add()
                        .out("offset");
                    let itself = compare(g, g.op("length", offset), 0.5, "<");
                    let point = point_at(g, &p, g.op("add", (f1_offset, offset)).first());
                    let d = distance(g, f1_point, point, &metric);
                    let distance = body.state("Distance");
                    body.set(
                        "Distance",
                        g.op("switch", (itself, g.op("min", (distance, d)), distance)),
                    );
                },
            )
        },
    );
    g.frame(
        Frame::new("Half-way").note("The largest ball of the metric around F1 inside its cell."),
        || {
            let radius = g.op("mul", (sphere.get("Distance"), 0.5)).first();
            g.output("radius", radius, Output::new(WgslType::F32).label("Radius"));
        },
    );
}

fn voronoi_smooth_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let (local, p) = cell_inputs(g, &t);
    let metric = metric_inputs(g);
    let smoothness = g.input(
        "smoothness",
        Input::new(WgslType::F32)
            .label("Smoothness")
            .default(0.5)
            .min(0.0)
            .max(0.5),
    );
    let color_seed = g
        .node("bitwise")
        .args((p.seed, COLOR_SALT))
        .setting("op", "^")
        .add()
        .first();
    let smooth = g.frame(
        Frame::new("Smooth minimum").note(
            "Every point of the 5×5(×5×5) cells around, blended in as they come:\nthe closer, the more, by the smoothness.",
        ),
        || {
            g.repeat(
                neighbor_count(g, &p, 2.0),
                vec![
                    RepeatItem::new("Smooth Distance", WgslType::F32, 0.0),
                    RepeatItem::new("Smooth Color", WgslType::Vec3f, 0.0),
                    RepeatItem::new("Smooth Point", &t, 0.0),
                ],
                |body| {
                    let (offset, point) = neighbor_point(g, &p, body.iteration(), 2.0);
                    let d = distance(g, point, local, &metric);
                    let blend = g.op(
                        "smoothstep",
                        (
                            0.0,
                            1.0,
                            g.op(
                                "add",
                                (
                                    0.5,
                                    g.op(
                                        "mul",
                                        (
                                            0.5,
                                            g.op(
                                                "div",
                                                (
                                                    g.op("sub", (body.state("Smooth Distance"), d)),
                                                    smoothness,
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    );
                    let h = g
                        .op(
                            "switch",
                            (compare(g, body.iteration(), 0.0, "=="), blend, 1.0),
                        )
                        .first();
                    let correction = g.op("mul", (smoothness, h, g.op("sub", (1.0, h))));
                    let shared = g
                        .op(
                            "div",
                            (correction, g.op("add", (1.0, g.op("mul", (3.0, smoothness))))),
                        )
                        .first();
                    let color = cell_hash(g, g.op("add", (p.cell, offset)).first(), color_seed);
                    let distance_mix = g.op("mix", (body.state("Smooth Distance"), d, h));
                    body.set("Smooth Distance", g.op("sub", (distance_mix, correction)));
                    let color_mix = g.op("mix", (body.state("Smooth Color"), color, h));
                    body.set("Smooth Color", g.op("sub", (color_mix, shared)));
                    let point_mix = g.op("mix", (body.state("Smooth Point"), point, h));
                    body.set("Smooth Point", g.op("sub", (point_mix, shared)));
                },
            )
        },
    );
    g.output(
        "distance",
        smooth.get("Smooth Distance"),
        Output::new(WgslType::F32).label("Smooth Distance"),
    );
    g.output(
        "color",
        smooth.get("Smooth Color"),
        Output::new(WgslType::Vec3f).label("Smooth Color"),
    );
    g.output(
        "point",
        smooth.get("Smooth Point"),
        Output::new(&t).label("Smooth Point"),
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            VORONOI_POINT,
            "Voronoi Point",
            "texture",
            &["cell", "offset", "randomness", "seed"],
            &["point", "neighbor"],
            voronoi_point_body,
        )
        .aliases(&["Feature Point", "Jittered Point", "Cell Point"])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            VORONOI_NEAREST,
            "Voronoi Nearest Points",
            "texture",
            &["cell", "local", "randomness", "seed", "metric", "exponent"],
            &[
                "f1-distance",
                "f2-distance",
                "f1-point",
                "f2-point",
                "f1-offset",
                "f2-offset",
            ],
            voronoi_nearest_body,
        )
        .aliases(&["F1 F2", "Closest Points", "Nearest Cells"])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            VORONOI_EDGES,
            "Voronoi Edges",
            "texture",
            &[
                "cell",
                "local",
                "randomness",
                "seed",
                "metric",
                "exponent",
                "f1-point",
                "f1-offset",
                "f2-point",
                "f2-offset",
            ],
            &["edge", "f2-edge"],
            voronoi_edges_body,
        )
        .aliases(&["Distance to Edge", "Cell Edges", "Bisectors"])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            VORONOI_RADIUS,
            "Voronoi Radius",
            "texture",
            &[
                "cell",
                "randomness",
                "seed",
                "metric",
                "exponent",
                "f1-point",
                "f1-offset",
            ],
            &["radius"],
            voronoi_radius_body,
        )
        .aliases(&["N-Sphere Radius", "Cell Radius", "Inscribed Radius"])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            VORONOI_SMOOTH,
            "Voronoi Smooth F1",
            "texture",
            &[
                "cell",
                "local",
                "randomness",
                "seed",
                "metric",
                "exponent",
                "smoothness",
            ],
            &["distance", "color", "point"],
            voronoi_smooth_body,
        )
        .aliases(&["Smooth Minimum", "Smooth Voronoi", "Smooth F1"])
        .helper(&["Voronoi"]),
    ]
}
