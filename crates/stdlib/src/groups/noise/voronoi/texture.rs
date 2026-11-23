//! Voronoi Texture and Fractal Voronoi, after Blender's (cycles/kernel/svm/voronoi.h).

use shader_graph::types::WgslType;

use super::cells::{
    COLOR_SALT, VORONOI_EDGES, VORONOI_NEAREST, VORONOI_RADIUS, VORONOI_SMOOTH, cell_hash,
};
use super::metric::{MetricInputs, metric_inputs};
use crate::builder::{
    Frame, GroupBuilder, GroupSpec, Input, Operand, Output, Ref, RepeatItem, TypeVar,
};
use crate::groups::helpers::{clamp_range, compare};
use crate::groups::noise::coordinates::{NOISE_COORDINATES, coordinates_input};
use crate::groups::noise::fractal::{FractalInputs, Octave, fractal, fractal_inputs};

pub const VORONOI_TEXTURE: &str = "std-voronoi-texture";
pub const FRACTAL_VORONOI: &str = "std-fractal-voronoi";

/// The inputs of the Voronoi groups, keyed as `std-voronoi-texture` takes them.
struct VoronoiInputs {
    t: TypeVar,
    vector: Ref,
    scale: Ref,
    randomness: Ref,
    smoothness: Ref,
    metric: MetricInputs,
    seed: Ref,
}

fn voronoi_inputs(g: &GroupBuilder<'_>) -> VoronoiInputs {
    let (t, vector) = coordinates_input(g);
    let scale = g.input(
        "scale",
        Input::new(WgslType::F32).label("Scale").default(5.0),
    );
    let randomness = g.input(
        "randomness",
        Input::new(WgslType::F32)
            .label("Randomness")
            .default(1.0)
            .min(0.0)
            .max(1.0),
    );
    let smoothness = g.input(
        "smoothness",
        Input::new(WgslType::F32)
            .label("Smoothness")
            .default(1.0)
            .min(0.0)
            .max(1.0),
    );
    let metric = metric_inputs(g);
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    VoronoiInputs {
        t,
        vector,
        scale,
        randomness,
        smoothness,
        metric,
        seed,
    }
}

/// The Voronoi texture's inputs as a call of it passes them, with `scale` replaced.
fn feed_voronoi<'g>(
    g: &'g GroupBuilder<'_>,
    inputs: &VoronoiInputs,
    scale: impl Into<Operand>,
) -> crate::builder::Node<'g> {
    g.call(VORONOI_TEXTURE)
        .input("vector", inputs.vector)
        .input("scale", scale)
        .input("randomness", inputs.randomness)
        .input("smoothness", inputs.smoothness)
        .input("metric", inputs.metric.metric.value())
        .input("exponent", inputs.metric.exponent)
        .input("seed", inputs.seed)
        .add()
}

/// The distance from a point to the cell's lattice point, scaled by the sampled noise weight.
fn voronoi_texture_body(g: &GroupBuilder<'_>) {
    let inputs = voronoi_inputs(g);
    let t = &inputs.t;
    let (cell, local, randomness, smoothness, color_seed) = g.frame(
        Frame::new("Lattice").note(
            "Space is cut into cells of side 1 / Scale, each holding one point\nmoved at random by up to Randomness, in as many dimensions\nas Vector has components.",
        ),
        || {
            let coordinates = g
                .call(NOISE_COORDINATES)
                .input("vector", inputs.vector)
                .add()
                .out("coordinates");
            let position = g.op("mul", (coordinates, inputs.scale)).first();
            let floor = g.op("floor", position).first();
            let cell = g.wireless("Cell", floor).first();
            let local = g
                .wireless("Local Position", g.op("sub", (position, floor)))
                .first();
            let randomness = g
                .wireless("Randomness", clamp_range(g, inputs.randomness, 0.0, 1.0))
                .first();
            let smoothness = clamp_range(g, g.op("mul", (inputs.smoothness, 0.5)), 0.0, 0.5);
            let salted = g
                .node("bitwise")
                .args((inputs.seed, COLOR_SALT))
                .setting("op", "^")
                .add()
                .first();
            let color_seed = g.wireless("Color Seed", salted).first();
            (cell, local, randomness, smoothness, color_seed)
        },
    );
    let color_of = |offset: Ref| cell_hash(g, g.op("add", (cell, offset)).first(), color_seed);
    let position_of = |point: Ref| {
        g.op("div", (g.op("add", (cell, point)), inputs.scale))
            .first()
    };

    let nearest = g.frame(
        Frame::new("F1 and F2").note("The closest point (F1) and the one after it (F2)."),
        || {
            let nearest = g
                .call(VORONOI_NEAREST)
                .input("cell", cell)
                .input("local", local)
                .input("randomness", randomness)
                .input("seed", inputs.seed)
                .input("metric", inputs.metric.metric.value())
                .input("exponent", inputs.metric.exponent)
                .add();
            let f1_point = g.wireless("F1 Point", nearest.out("f1-point")).first();
            let f1_offset = g.wireless("F1 Offset", nearest.out("f1-offset")).first();
            let f2_point = g.wireless("F2 Point", nearest.out("f2-point")).first();
            let f2_offset = g.wireless("F2 Offset", nearest.out("f2-offset")).first();
            let f1_color = g.wireless("F1 Color", color_of(f1_offset)).first();
            let f1_position = g.wireless("F1 Position", position_of(f1_point)).first();
            g.output(
                "distance",
                nearest.out("f1-distance"),
                Output::new(WgslType::F32).label("Distance"),
            );
            g.output(
                "color",
                f1_color,
                Output::new(WgslType::Vec3f).label("Color"),
            );
            g.output("position", f1_position, Output::new(t).label("Position"));
            g.output(
                "cell",
                g.op("add", (cell, f1_offset)),
                Output::new(t).label("Cell"),
            );
            g.output(
                "f2-distance",
                nearest.out("f2-distance"),
                Output::new(WgslType::F32).label("F2 Distance"),
            );
            g.output(
                "f2-color",
                color_of(f2_offset),
                Output::new(WgslType::Vec3f).label("F2 Color"),
            );
            g.output(
                "f2-position",
                position_of(f2_point),
                Output::new(t).label("F2 Position"),
            );
            (
                f1_point,
                f1_offset,
                f2_point,
                f2_offset,
                f1_color,
                f1_position,
                nearest.out("f1-distance"),
            )
        },
    );
    let (f1_point, f1_offset, f2_point, f2_offset, f1_color, f1_position, f1_distance) = nearest;

    g.frame(
        Frame::new("Smooth F1")
            .note("F1 blended with the points around it, or F1 itself at no smoothness."),
        || {
            let smooth = g
                .call(VORONOI_SMOOTH)
                .input("cell", cell)
                .input("local", local)
                .input("randomness", randomness)
                .input("seed", inputs.seed)
                .input("metric", inputs.metric.metric.value())
                .input("exponent", inputs.metric.exponent)
                .input("smoothness", smoothness)
                .add();
            let smoothed = compare(g, smoothness, 0.0, ">");
            g.output(
                "smooth-distance",
                g.op("switch", (smoothed, f1_distance, smooth.out("distance"))),
                Output::new(WgslType::F32).label("Smooth Distance"),
            );
            g.output(
                "smooth-color",
                g.op("switch", (smoothed, f1_color, smooth.out("color"))),
                Output::new(WgslType::Vec3f).label("Smooth Color"),
            );
            g.output(
                "smooth-position",
                g.op(
                    "switch",
                    (smoothed, f1_position, position_of(smooth.out("point"))),
                ),
                Output::new(t).label("Smooth Position"),
            );
        },
    );

    g.frame(
        Frame::new("Edges").note("How far the cells' edges are."),
        || {
            let edges = g
                .call(VORONOI_EDGES)
                .input("cell", cell)
                .input("local", local)
                .input("randomness", randomness)
                .input("seed", inputs.seed)
                .input("metric", inputs.metric.metric.value())
                .input("exponent", inputs.metric.exponent)
                .input("f1-point", f1_point)
                .input("f1-offset", f1_offset)
                .input("f2-point", f2_point)
                .input("f2-offset", f2_offset)
                .add();
            g.output(
                "edge",
                edges.out("edge"),
                Output::new(WgslType::F32).label("Distance to Edge"),
            );
            g.output(
                "f2-edge",
                edges.out("f2-edge"),
                Output::new(WgslType::F32).label("F2 Distance to Edge"),
            );
        },
    );

    g.frame(
        Frame::new("Radius").note("The largest ball of the metric inside F1's cell."),
        || {
            let radius = g
                .call(VORONOI_RADIUS)
                .input("metric", inputs.metric.metric.value())
                .input("exponent", inputs.metric.exponent)
                .input("cell", cell)
                .input("randomness", randomness)
                .input("seed", inputs.seed)
                .input("f1-point", f1_point)
                .input("f1-offset", f1_offset)
                .add()
                .out("radius");
            g.output("radius", radius, Output::new(WgslType::F32).label("Radius"));
        },
    );
}

/// The Voronoi texture summed over octaves, each finer and fainter, and
/// normalized by its own farthest distance.
fn fractal_voronoi_body(g: &GroupBuilder<'_>) {
    let inputs = voronoi_inputs(g);
    let t = &inputs.t;
    let octaves: FractalInputs = fractal_inputs(g);
    let normalize = g.input(
        "normalize",
        Input::new(WgslType::Bool).label("Normalize").default(false),
    );
    let (reach, farthest) = g.frame(
        Frame::new("Reach").note(
            "How far a point can be from its cell's centre, to normalize by:\nReach along every axis, in the metric.",
        ),
        || {
            let randomness = clamp_range(g, inputs.randomness, 0.0, 1.0);
            let reach = g
                .wireless("Reach", g.op("add", (0.5, g.op("mul", (0.5, randomness)))))
                .first();
            let dimensions = g.op("component-count", inputs.vector).first();
            let across = super::metric::across_in_cell(g, &inputs.metric, dimensions);
            let max_distance = g.label(g.op("mul", (reach, across)), "Max Distance");
            let farthest = g.wireless("Max Distance", max_distance).first();
            (reach, farthest)
        },
    );

    let sum = g.frame(
        Frame::new("Octaves").note(
            "Voronoi Texture again at each octave, its scale growing by\nLacunarity and its weight shrinking by Roughness, summed.",
        ),
        || {
            let items = vec![
                RepeatItem::new("Distance", WgslType::F32, 0.0),
                RepeatItem::new("Color", WgslType::Vec3f, 0.0),
                RepeatItem::new("Position", t, 0.0),
                RepeatItem::new("F2 Distance", WgslType::F32, 0.0),
                RepeatItem::new("F2 Color", WgslType::Vec3f, 0.0),
                RepeatItem::new("F2 Position", t, 0.0),
                RepeatItem::new("Smooth Distance", WgslType::F32, 0.0),
                RepeatItem::new("Smooth Color", WgslType::Vec3f, 0.0),
                RepeatItem::new("Smooth Position", t, 0.0),
                RepeatItem::new("Total Amplitude", WgslType::F32, 0.0),
                RepeatItem::new("Edge Distance", WgslType::F32, 8.0),
                RepeatItem::new("F2 Edge Distance", WgslType::F32, 8.0),
                RepeatItem::new("Edge Max", WgslType::F32, reach),
            ];
            fractal(g, octaves, items, |body, octave: Octave| {
                let v = feed_voronoi(g, &inputs, g.op("mul", (inputs.scale, octave.scale)).first());
                let amplitude = octave.amplitude;
                let accumulate = |key: &str, value: Ref| {
                    g.op("add", (body.state(key), g.op("mul", (value, amplitude))))
                };
                let blend = |key: &str, value: Ref| g.op("mix", (body.state(key), value, amplitude));
                let closer = |key: &str, value: Ref| {
                    let nearer = g.op("min", (body.state(key), g.op("div", (value, octave.scale))));
                    g.op("mix", (body.state(key), nearer, amplitude))
                };
                g.frame(Frame::new("F1"), || {
                    body.set("Distance", accumulate("Distance", v.out("distance")));
                    body.set("Color", accumulate("Color", v.out("color")));
                    body.set("Position", blend("Position", v.out("position")));
                });
                g.frame(Frame::new("F2"), || {
                    body.set("F2 Distance", accumulate("F2 Distance", v.out("f2-distance")));
                    body.set("F2 Color", accumulate("F2 Color", v.out("f2-color")));
                    body.set("F2 Position", blend("F2 Position", v.out("f2-position")));
                });
                g.frame(Frame::new("Smooth F1"), || {
                    body.set("Smooth Distance", accumulate("Smooth Distance", v.out("smooth-distance")));
                    body.set("Smooth Color", accumulate("Smooth Color", v.out("smooth-color")));
                    body.set("Smooth Position", blend("Smooth Position", v.out("smooth-position")));
                });
                g.frame(
                    Frame::new("Edges")
                        .note("The closest edge of any octave, faded in by its weight."),
                    || {
                        body.set("Edge Distance", closer("Edge Distance", v.out("edge")));
                        body.set("F2 Edge Distance", closer("F2 Edge Distance", v.out("f2-edge")));
                        body.set(
                            "Edge Max",
                            g.op("mix", (body.state("Edge Max"), g.op("div", (reach, octave.scale)), amplitude)),
                        );
                    },
                );
                body.set(
                    "Total Amplitude",
                    g.op("add", (body.state("Total Amplitude"), amplitude)),
                );
            })
        },
    );

    let total = g
        .wireless("Total Amplitude", sum.get("Total Amplitude"))
        .first();
    let distance_range = g
        .wireless("Distance Range", g.op("mul", (total, farthest)))
        .first();
    let normalized = |value: Ref, by: Ref| {
        g.op("switch", (normalize, value, g.op("div", (value, by))))
            .first()
    };
    g.frame(
        Frame::new("F1").note("Divided by their greatest, when normalized."),
        || {
            g.output(
                "distance",
                normalized(sum.get("Distance"), distance_range),
                Output::new(WgslType::F32).label("Distance"),
            );
            g.output(
                "color",
                normalized(sum.get("Color"), total),
                Output::new(WgslType::Vec3f).label("Color"),
            );
            g.output(
                "position",
                sum.get("Position"),
                Output::new(t).label("Position"),
            );
        },
    );
    g.frame(Frame::new("F2"), || {
        g.output(
            "f2-distance",
            normalized(
                sum.get("F2 Distance"),
                g.op("mul", (distance_range, 2.0)).first(),
            ),
            Output::new(WgslType::F32).label("F2 Distance"),
        );
        g.output(
            "f2-color",
            normalized(sum.get("F2 Color"), total),
            Output::new(WgslType::Vec3f).label("F2 Color"),
        );
        g.output(
            "f2-position",
            sum.get("F2 Position"),
            Output::new(t).label("F2 Position"),
        );
    });
    g.frame(Frame::new("Smooth F1"), || {
        g.output(
            "smooth-distance",
            normalized(sum.get("Smooth Distance"), distance_range),
            Output::new(WgslType::F32).label("Smooth Distance"),
        );
        g.output(
            "smooth-color",
            normalized(sum.get("Smooth Color"), total),
            Output::new(WgslType::Vec3f).label("Smooth Color"),
        );
        g.output(
            "smooth-position",
            sum.get("Smooth Position"),
            Output::new(t).label("Smooth Position"),
        );
    });
    g.frame(Frame::new("Edges"), || {
        let edge_max = sum.get("Edge Max");
        g.output(
            "edge",
            normalized(sum.get("Edge Distance"), edge_max),
            Output::new(WgslType::F32).label("Distance to Edge"),
        );
        g.output(
            "f2-edge",
            normalized(sum.get("F2 Edge Distance"), edge_max),
            Output::new(WgslType::F32).label("F2 Distance to Edge"),
        );
    });
    g.frame(
        Frame::new("First octave").note("Not fractal: the first octave's."),
        || {
            let first = g
                .call(VORONOI_TEXTURE)
                .input("vector", inputs.vector)
                .input("scale", inputs.scale)
                .input("randomness", inputs.randomness)
                .input("smoothness", inputs.smoothness)
                .input("metric", inputs.metric.metric.value())
                .input("exponent", inputs.metric.exponent)
                .input("seed", inputs.seed)
                .add();
            g.output("cell", first.out("cell"), Output::new(t).label("Cell"));
            g.output(
                "radius",
                first.out("radius"),
                Output::new(WgslType::F32).label("Radius"),
            );
        },
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            VORONOI_TEXTURE,
            "Voronoi Texture",
            "texture",
            &[
                "vector",
                "scale",
                "randomness",
                "smoothness",
                "metric",
                "exponent",
                "seed",
            ],
            &[
                "distance",
                "color",
                "position",
                "cell",
                "f2-distance",
                "f2-color",
                "f2-position",
                "smooth-distance",
                "smooth-color",
                "smooth-position",
                "edge",
                "f2-edge",
                "radius",
            ],
            voronoi_texture_body,
        )
        .aliases(&[
            "Voronoi",
            "Worley",
            "Cellular Noise",
            "Cells",
            "Voronoi Noise",
            "F1",
            "F2",
        ])
        .helper(&["Voronoi"]),
        GroupSpec::new(
            FRACTAL_VORONOI,
            "Fractal Voronoi",
            "texture",
            &[
                "vector",
                "scale",
                "randomness",
                "smoothness",
                "metric",
                "exponent",
                "seed",
                "detail",
                "roughness",
                "lacunarity",
                "normalize",
            ],
            &[
                "distance",
                "color",
                "position",
                "f2-distance",
                "f2-color",
                "f2-position",
                "smooth-distance",
                "smooth-color",
                "smooth-position",
                "edge",
                "f2-edge",
                "cell",
                "radius",
            ],
            fractal_voronoi_body,
        )
        .aliases(&[
            "Voronoi fBm",
            "Voronoi FBM",
            "Fractal Worley",
            "Voronoi Texture",
            "Cellular fBm",
        ]),
    ]
}
