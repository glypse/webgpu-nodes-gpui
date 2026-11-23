//! OpenSimplex2 in each dimension count: which lattice points are close
//! enough to a position to count, found without branching, and their kernels
//! summed. 1D is plain gradient noise. Each output is scaled to [-1, 1],
//! measured on these gradients.

use shader_graph::types::{InputValue, WgslType};

use super::kernel::{LatticeCopy, copy_seed, kernel};
use super::orientation::{FAST, SMOOTH, variant_input};
use crate::builder::{Frame, GroupBuilder, GroupSpec, Input, Operand, Output, Ref, RepeatItem};
use crate::groups::helpers::{compare, vec3_in};

pub const GRADIENT_NOISE_1D: &str = "std-gradient-noise-1d";
pub const OPENSIMPLEX_2D: &str = "std-opensimplex-2d";
pub const OPENSIMPLEX_3D_HALF: &str = "std-opensimplex-3d-half";
pub const OPENSIMPLEX_3D: &str = "std-opensimplex-3d";
pub const OPENSIMPLEX_4D: &str = "std-opensimplex-4d";

/// 1 / the largest value each one reaches, by dimension count.
fn normalizer(dimensions: u8, variant: i64) -> f64 {
    match (dimensions, variant) {
        (1, _) => 3.206,
        (2, FAST) => 99.28,
        (2, _) => 18.16,
        (3, FAST) => 32.84,
        (3, _) => 3.618,
        (_, FAST) => 108.7,
        _ => 8.724,
    }
}

/// How far from a point its kernel reaches, squared: exact for the points each looks at.
fn radius(dimensions: u8, variant: i64) -> f64 {
    match (dimensions, variant) {
        (2, FAST) | (3, FAST) | (4, FAST) => 0.5,
        (2, _) => 2.0 / 3.0,
        (3, _) => 0.75,
        _ => 0.8,
    }
}

const UNSKEW_2D: f64 = -0.21132486540518713;

fn unskew_4d(variant: i64) -> f64 {
    if variant == FAST {
        0.309016994374947
    } else {
        -0.138196601125011
    }
}

fn seed_input(g: &GroupBuilder<'_>) -> Ref {
    g.input("seed", Input::new(WgslType::I32).label("Seed").default(0))
}

fn constant_i32(g: &GroupBuilder<'_>, n: f64) -> Ref {
    g.constant(WgslType::I32, n).first()
}

/// A skewed lattice's offset `q` back in real space: q + (q · 1) × unskew.
fn unskew(g: &GroupBuilder<'_>, q: Ref, factor: f64) -> Ref {
    let scaled = g.op("mul", (g.op("dot", (q, 1.0)), factor));
    g.op("add", (q, scaled)).first()
}

fn gradient_noise_1d_body(g: &GroupBuilder<'_>) {
    let x = g.input("value", Input::new(WgslType::F32).label("Value"));
    let seed = seed_input(g);
    g.frame(
        Frame::new("Neighbors").note(
            "The kernels of the whole numbers on either side, reaching 1:\nonly those two are ever close enough.",
        ),
        || {
            let left = g.op("floor", x).first();
            let right = g.op("add", (left, 1.0)).first();
            let near = kernel(g, left, g.op("sub", (x, left)), 1.0, seed);
            let far = kernel(g, right, g.op("sub", (x, right)), 1.0, seed);
            let sum = g.op("add", (near, far));
            g.output(
                "value",
                g.op("mul", (sum, normalizer(1, SMOOTH))),
                Output::new(WgslType::F32).label("Value"),
            );
        },
    );
}

fn at2(g: &GroupBuilder<'_>, x: f64, y: f64) -> Ref {
    g.constant(WgslType::Vec2f, InputValue::vector(&[x, y]))
        .first()
}

fn opensimplex_2d_body(g: &GroupBuilder<'_>) {
    let position = g.input(
        "lattice",
        Input::new(WgslType::Vec2f).label("Lattice Position"),
    );
    let variant = variant_input(g, None);
    let seed = seed_input(g);
    let (base, local) = g.frame(
        Frame::new("Cell").note("The skewed square the position is in, and where in it."),
        || {
            let base = g.op("floor", position).first();
            let local = g.op("sub", (position, base)).first();
            (
                g.wireless("Cell", base).first(),
                g.wireless("Local", local).first(),
            )
        },
    );
    let vertex_sum = |offsets: &[Ref], radius: f64| -> Ref {
        let terms: Vec<Ref> = offsets
            .iter()
            .map(|offset| {
                let vertex = g.op("add", (base, *offset)).first();
                let delta = unskew(g, g.op("sub", (position, vertex)).first(), UNSKEW_2D);
                kernel(g, vertex, delta, radius, seed)
            })
            .collect();
        g.op("add", terms).first()
    };
    let split = g.op("separate-vector", local);
    let (x, y) = (split.out("x"), split.out("y"));
    let fast = g.frame(
        Frame::new("Fast").note(
            "The 3 corners of the triangle the position is in: the square's\ntwo ends of its diagonal, and the corner on the position's side.",
        ),
        || {
            let third = g
                .op(
                    "switch",
                    (
                        compare(g, y, x, ">"),
                        at2(g, 1.0, 0.0),
                        at2(g, 0.0, 1.0),
                    ),
                )
                .first();
            let sum = vertex_sum(
                &[at2(g, 0.0, 0.0), at2(g, 1.0, 1.0), third],
                radius(2, FAST),
            );
            g.op("mul", (sum, normalizer(2, FAST))).first()
        },
    );
    let smooth = g.frame(
        Frame::new("Smooth").note(
            "The square's diagonal ends, and the 2 points across the triangle's\nother edges that its larger kernels reach, past the square when\nthe position is near a corner.",
        ),
        || {
            let far = compare(g, g.op("add", (x, y)).first(), 1.0, ">");
            let xmy = g.op("sub", (x, y)).first();
            let third = g
                .op(
                    "switch",
                    (
                        far,
                        g.op(
                            "switch",
                            (
                                compare(g, g.op("add", (x, xmy)).first(), 0.0, "<"),
                                at2(g, 1.0, 0.0),
                                at2(g, -1.0, 0.0),
                            ),
                        )
                        .first(),
                        g.op(
                            "switch",
                            (
                                compare(g, g.op("add", (x, xmy)).first(), 1.0, ">"),
                                at2(g, 0.0, 1.0),
                                at2(g, 2.0, 1.0),
                            ),
                        )
                        .first(),
                    ),
                )
                .first();
            let y_minus_xmy = g.op("sub", (y, xmy)).first();
            let fourth = g
                .op(
                    "switch",
                    (
                        far,
                        g.op(
                            "switch",
                            (
                                compare(g, y, xmy, "<"),
                                at2(g, 0.0, 1.0),
                                at2(g, 0.0, -1.0),
                            ),
                        )
                        .first(),
                        g.op(
                            "switch",
                            (
                                compare(g, y_minus_xmy, 1.0, ">"),
                                at2(g, 1.0, 0.0),
                                at2(g, 1.0, 2.0),
                            ),
                        )
                        .first(),
                    ),
                )
                .first();
            let sum = vertex_sum(
                &[at2(g, 0.0, 0.0), at2(g, 1.0, 1.0), third, fourth],
                radius(2, SMOOTH),
            );
            g.op("mul", (sum, normalizer(2, SMOOTH))).first()
        },
    );
    let value = g
        .match_on(&variant)
        .case(FAST, fast)
        .case(SMOOTH, smooth)
        .add()
        .first();
    g.output("value", value, Output::new(WgslType::F32).label("Value"));
}

fn at3(g: &GroupBuilder<'_>, x: f64, y: f64, z: f64) -> Ref {
    g.constant(WgslType::Vec3f, InputValue::vector(&[x, y, z]))
        .first()
}

fn opensimplex_3d_half_body(g: &GroupBuilder<'_>) {
    let position = vec3_in(g, "lattice", "Lattice Position");
    let variant = variant_input(g, None);
    let seed = seed_input(g);
    let fast = g.frame(
        Frame::new("Fast").note(
            "The closest whole point, and the next one along the axis\nthe position is furthest along from it.",
        ),
        || {
            let closest = g.op("round", position).first();
            let offset = g.op("sub", (position, closest)).first();
            let distance = g.op("abs", offset).first();
            let split = g.op("separate-vector", distance);
            let (x, y, z) = (split.out("x"), split.out("y"), split.out("z"));
            let others = g
                .vec(
                    WgslType::Vec3f,
                    (
                        g.op("max", (y, z)),
                        g.op("max", (z, x)),
                        g.op("max", (x, y)),
                    ),
                )
                .first();
            let along = g.op("step", (others, distance)).first();
            let next = g
                .op("add", (closest, g.op("mul", (along, g.op("sign", offset)))))
                .first();
            let r = radius(3, FAST);
            let near = kernel(g, closest, offset, r, seed);
            let far = kernel(g, next, g.op("sub", (position, next)), r, seed);
            g.op("add", (near, far)).first()
        },
    );
    let smooth = g.frame(
        Frame::new("Smooth").note(
            "Of each pair of opposite corners of the unit cube around the position,\nthe one on its side of the plane between them.",
        ),
        || {
            let base = g.op("floor", position).first();
            let local = g.op("sub", (position, base)).first();
            let corner = |normal: [f64; 3], bias: f64, from: [f64; 3], toward: [f64; 3]| -> Ref {
                let facing = g.op("dot", (local, at3(g, normal[0], normal[1], normal[2])));
                let far = g.op("floor", g.op("add", (facing, bias))).first();
                let offset = g.op(
                    "mul",
                    (at3(g, toward[0], toward[1], toward[2]), far),
                );
                g.op(
                    "add",
                    (
                        base,
                        at3(g, from[0], from[1], from[2]),
                        offset,
                    ),
                )
                .first()
            };
            let corners = [
                corner([0.25, 0.25, 0.25], 0.625, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
                corner([-0.25, 0.25, 0.25], 0.875, [1.0, 0.0, 0.0], [-1.0, 1.0, 1.0]),
                corner([0.25, -0.25, 0.25], 0.875, [0.0, 1.0, 0.0], [1.0, -1.0, 1.0]),
                corner([0.25, 0.25, -0.25], 0.875, [0.0, 0.0, 1.0], [1.0, 1.0, -1.0]),
            ];
            let terms: Vec<Operand> = corners
                .into_iter()
                .map(|c| {
                    let offset = g.op("sub", (position, c)).first();
                    kernel(g, c, offset, radius(3, SMOOTH), seed).into()
                })
                .collect();
            g.op("add", terms).first()
        },
    );
    let value = g
        .match_on(&variant)
        .case(FAST, fast)
        .case(SMOOTH, smooth)
        .add()
        .first();
    g.output("value", value, Output::new(WgslType::F32).label("Value"));
}

fn opensimplex_3d_body(g: &GroupBuilder<'_>) {
    let position = vec3_in(g, "lattice", "Lattice Position");
    let variant = variant_input(g, None);
    let seed = seed_input(g);
    g.frame(
        Frame::new("Lattices").note(
            "A body-centred cubic lattice: whole points, and a copy offset by ½\nwith gradients of its own.",
        ),
        || {
            let half = |lattice: Ref, copy: i64| {
                let seed = copy_seed(g, seed, LatticeCopy::Index(copy));
                g.call(OPENSIMPLEX_3D_HALF)
                    .input("lattice", lattice)
                    .input("variant", variant.value())
                    .input("seed", seed)
                    .add()
                    .out("value")
            };
            let shifted = g.op("add", (position, 0.5)).first();
            let sum = g.op("add", (half(position, 0), half(shifted, 1)));
            let normalizer = g
                .match_on(&variant)
                .case(FAST, normalizer(3, FAST))
                .case(SMOOTH, normalizer(3, SMOOTH))
                .add()
                .first();
            g.output(
                "value",
                g.op("mul", (sum, normalizer)),
                Output::new(WgslType::F32).label("Value"),
            );
        },
    );
}

/// The canonical vertices Smooth 4D looks at, for a position whose cell
/// coordinates are sorted (x ≥ y ≥ z ≥ w) and add up to at most 2: every
/// lattice point within its kernel's reach of such a position.
const SORTED_VERTICES: [[f64; 4]; 23] = [
    [0.0, -1.0, -1.0, -1.0],
    [0.0, -1.0, 0.0, 0.0],
    [0.0, 0.0, -1.0, -1.0],
    [0.0, 0.0, -1.0, 0.0],
    [0.0, 0.0, 0.0, -1.0],
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 1.0, 1.0, 0.0],
    [1.0, 0.0, -1.0, 0.0],
    [1.0, 0.0, 0.0, -1.0],
    [1.0, 0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0, 0.0],
    [1.0, 0.0, 1.0, 1.0],
    [1.0, 1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 1.0],
    [1.0, 1.0, 1.0, 0.0],
    [1.0, 1.0, 1.0, 1.0],
    [2.0, 1.0, 0.0, 0.0],
    [2.0, 1.0, 1.0, 0.0],
    [2.0, 1.0, 1.0, 1.0],
];

fn vec4_const(g: &GroupBuilder<'_>, components: [f64; 4]) -> Ref {
    g.constant(WgslType::Vec4f, InputValue::vector(&components))
        .first()
}

fn opensimplex_4d_body(g: &GroupBuilder<'_>) {
    let position = g.input(
        "lattice",
        Input::new(WgslType::Vec4f).label("Lattice Position"),
    );
    let variant = variant_input(g, None);
    let seed = seed_input(g);
    let base = g.wireless("Cell", g.op("floor", position)).first();

    let fast = g.frame(
        Frame::new("Fast").note(
            "Five copies of the A4 lattice, each ⅕ further along the diagonal,\none point of each: the closest corner of the simplex the position\nis in, moving one axis further per copy.",
        ),
        || {
            let u = unskew_4d(FAST);
            let (start, local, diagonal) = g.frame(
                Frame::new("First copy").note(
                    "The copy sure to have a point in the cell's base simplex,\nfrom how far along the diagonal the position is.",
                ),
                || {
                    let cell = g.op("sub", (position, base)).first();
                    let sum = g.op("dot", (cell, 1.0)).first();
                    let start = g
                        .op("to-int", g.op("floor", g.op("mul", (sum, 1.25))))
                        .first();
                    let shift = g.op("mul", (g.op("to-float", start), -0.2)).first();
                    (
                        g.wireless("First Copy", start).first(),
                        g.op("add", (cell, shift)).first(),
                        g.op("mul", (g.op("add", (sum, g.op("mul", (shift, 4.0)))), u)).first(),
                    )
                },
            );
            let copies = g.repeat(
                constant_i32(g, 5.0),
                vec![
                    RepeatItem::new("Local", WgslType::Vec4f, local),
                    RepeatItem::new("Diagonal", WgslType::F32, diagonal),
                    RepeatItem::new("Vertex", WgslType::Vec4f, base),
                    RepeatItem::new("Value", WgslType::F32, 0.0),
                ],
                |body| {
                    let (moved, lead) = g.frame(
                        Frame::new("Step").note(
                            "The axis the position is furthest along (the first on a tie),\nstepped once it's past the simplex's far face.",
                        ),
                        || {
                            let split = g.op("separate-vector", body.state("Local"));
                            let (x, y, z, w) = (
                                split.out("x"),
                                split.out("y"),
                                split.out("z"),
                                split.out("w"),
                            );
                            let lead = g.op("max", (x, y, z, w)).first();
                            let is_lead = g.op("step", (lead, body.state("Local"))).first();
                            let e = g.op("separate-vector", is_lead);
                            let (ex, ey, ez) = (e.out("x"), e.out("y"), e.out("z"));
                            let before = g
                                .vec(
                                    WgslType::Vec4f,
                                    (
                                        0.0,
                                        ex,
                                        g.op("add", (ex, ey)),
                                        g.op("add", (ex, ey, ez)),
                                    ),
                                )
                                .first();
                            let first = g
                                .op(
                                    "mul",
                                    (
                                        is_lead,
                                        g.op("sub", (1.0, g.op("min", (before, 1.0)))),
                                    ),
                                )
                                .first();
                            let face = g
                                .op("sub", (1.0, g.op("div", (body.state("Diagonal"), u))))
                                .first();
                            let moves = g
                                .op(
                                    "switch",
                                    (compare(g, lead, face, ">="), 0.0, 1.0),
                                )
                                .first();
                            (g.op("mul", (first, moves)).first(), moves)
                        },
                    );
                    let local = g.op("sub", (body.state("Local"), moved)).first();
                    let diagonal = g
                        .op("sub", (body.state("Diagonal"), g.op("mul", (lead, u))))
                        .first();
                    let vertex = g.op("add", (body.state("Vertex"), moved)).first();
                    let copy = g
                        .op(
                            "mod",
                            (
                                g.op(
                                    "add",
                                    (g.op("sub", (start, body.iteration())), constant_i32(g, 5.0)),
                                ),
                                constant_i32(g, 5.0),
                            ),
                        )
                        .first();
                    let value = kernel(
                        g,
                        vertex,
                        g.op("add", (local, diagonal)),
                        radius(4, FAST),
                        copy_seed(g, seed, LatticeCopy::Dynamic(copy)),
                    );
                    let next = g.frame(
                        Frame::new("Next copy").note(
                            "⅕ further along the diagonal; past the first copy the\ncell's corner falls one step back.",
                        ),
                        || {
                            let stepped = g
                                .op(
                                    "switch",
                                    (
                                        compare(g, body.iteration(), start, "=="),
                                        0.0,
                                        1.0,
                                    ),
                                )
                                .first();
                            (
                                g.op("add", (local, 0.2)).first(),
                                g.op("add", (diagonal, 0.8 * u)).first(),
                                g.op("sub", (vertex, stepped)).first(),
                                g.op("add", (body.state("Value"), value)).first(),
                            )
                        },
                    );
                    body.set("Local", next.0);
                    body.set("Diagonal", next.1);
                    body.set("Vertex", next.2);
                    body.set("Value", next.3);
                },
            );
            g.op("mul", (copies.get("Value"), normalizer(4, FAST))).first()
        },
    );

    let smooth = g.frame(
        Frame::new("Smooth").note(
            "The A4* lattice's points within reach, from a table of those around\na cell whose coordinates are sorted, the position's sorted the same way.",
        ),
        || {
            let (flipped, cell) = g.frame(
                Frame::new("Symmetry").note(
                    "Past the cell's middle diagonal, mirrored through its centre,\nso the coordinates add up to at most 2.",
                ),
                || {
                    let cell = g.op("sub", (position, base)).first();
                    let flipped = g
                        .wireless("Flipped", compare(g, g.op("dot", (cell, 1.0)).first(), 2.0, ">"))
                        .first();
                    let folded = g
                        .op("switch", (flipped, cell, g.op("sub", (1.0, cell))))
                        .first();
                    (flipped, folded)
                },
            );
            let picks: Vec<Ref> = g.frame(
                Frame::new("Ranks").note(
                    "Each axis's rank among the coordinates, largest first (the first\naxis on a tie), and which axis has each rank.",
                ),
                || {
                    let split = g.op("separate-vector", cell);
                    let axes = ["x", "y", "z", "w"];
                    let rank_terms: Vec<Operand> = axes
                        .iter()
                        .enumerate()
                        .map(|(j, axis)| {
                            let after = vec4_const(
                                g,
                                std::array::from_fn(|i| f64::from(u8::from(i > j))),
                            );
                            let before = vec4_const(
                                g,
                                std::array::from_fn(|i| f64::from(u8::from(i < j))),
                            );
                            let component = split.out(axis);
                            let at_least = g
                                .op("to-float", compare(g, component, cell, ">="))
                                .first();
                            let above = g
                                .op("to-float", compare(g, component, cell, ">"))
                                .first();
                            g.op(
                                "add",
                                (
                                    g.op("mul", (at_least, after)),
                                    g.op("mul", (above, before)),
                                ),
                            )
                            .first()
                            .into()
                        })
                        .collect();
                    let rank = g.op("add", rank_terms).first();
                    (0..4)
                        .map(|r| {
                            let distance = g.op("abs", g.op("sub", (rank, f64::from(r)))).first();
                            g.op("sub", (1.0, g.op("min", (distance, 1.0)))).first()
                        })
                        .collect()
                },
            );
            let sum = g.repeat(
                constant_i32(g, SORTED_VERTICES.len() as f64),
                vec![RepeatItem::new("Value", WgslType::F32, 0.0)],
                |body| {
                    let sorted = SORTED_VERTICES
                        .iter()
                        .enumerate()
                        .fold(
                            g.index_switch(body.iteration(), WgslType::Vec4f),
                            |switch, (i, components)| {
                                switch.case(i as i64, InputValue::vector(components))
                            },
                        )
                        .add()
                        .first();
                    let split = g.op("separate-vector", sorted);
                    let offset_terms: Vec<Operand> = ["x", "y", "z", "w"]
                        .iter()
                        .zip(&picks)
                        .map(|(axis, pick)| g.op("mul", (split.out(axis), *pick)).into())
                        .collect();
                    let offset = g.op("add", offset_terms).first();
                    let folded = g
                        .op("switch", (flipped, offset, g.op("sub", (1.0, offset))))
                        .first();
                    let vertex = g.op("add", (base, folded)).first();
                    let delta = unskew(g, g.op("sub", (position, vertex)).first(), unskew_4d(SMOOTH));
                    let value = kernel(g, vertex, delta, radius(4, SMOOTH), seed);
                    body.set("Value", g.op("add", (body.state("Value"), value)));
                },
            );
            g.op("mul", (sum.get("Value"), normalizer(4, SMOOTH))).first()
        },
    );
    let value = g
        .match_on(&variant)
        .case(FAST, fast)
        .case(SMOOTH, smooth)
        .add()
        .first();
    g.output("value", value, Output::new(WgslType::F32).label("Value"));
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            GRADIENT_NOISE_1D,
            "Gradient Noise 1D",
            "texture",
            &["value", "seed"],
            &["value"],
            gradient_noise_1d_body,
        )
        .aliases(&["1D Noise", "Perlin 1D", "Simplex 1D", "Line Noise"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            OPENSIMPLEX_2D,
            "OpenSimplex 2D",
            "texture",
            &["lattice", "variant", "seed"],
            &["value"],
            opensimplex_2d_body,
        )
        .aliases(&["Simplex 2D", "OpenSimplex2 2D", "SuperSimplex 2D"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            OPENSIMPLEX_3D_HALF,
            "OpenSimplex 3D Half-Lattice",
            "texture",
            &["lattice", "variant", "seed"],
            &["value"],
            opensimplex_3d_half_body,
        )
        .aliases(&["BCC Half-Lattice", "Cubic Lattice Half"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            OPENSIMPLEX_3D,
            "OpenSimplex 3D",
            "texture",
            &["lattice", "variant", "seed"],
            &["value"],
            opensimplex_3d_body,
        )
        .aliases(&[
            "Simplex 3D",
            "OpenSimplex2 3D",
            "SuperSimplex 3D",
            "BCC Noise",
        ])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            OPENSIMPLEX_4D,
            "OpenSimplex 4D",
            "texture",
            &["lattice", "variant", "seed"],
            &["value"],
            opensimplex_4d_body,
        )
        .aliases(&["Simplex 4D", "OpenSimplex2 4D", "SuperSimplex 4D"])
        .helper(&["OpenSimplex"]),
    ]
}
