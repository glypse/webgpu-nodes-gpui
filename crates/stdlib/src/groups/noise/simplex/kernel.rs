//! What every OpenSimplex lattice adds up: a random gradient per lattice
//! point, faded out within a radius of it.

use std::f64::consts::PI;

use shader_graph::types::{InputValue, WgslType};

use crate::builder::Operand;
use crate::builder::{Frame, GroupBuilder, GroupSpec, Input, Output, Ref};
use crate::groups::helpers::component_sum;
use crate::groups::noise::coordinates::coordinates_type;
use crate::groups::noise::lattice::HASH;

pub const LATTICE_GRADIENT: &str = "std-lattice-gradient";
pub const SIMPLEX_KERNEL: &str = "std-simplex-kernel";

/// Added to the seed per copy of a lattice (times the copy's index), so the
/// copies get gradients of their own. Small enough for 4 × it to fit an i32.
const LATTICE_SALT: f64 = 461845907.0;

fn lattice_gradient_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let vertex = g.input("vertex", Input::new(&t).label("Vertex"));
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    let h = g
        .call(HASH)
        .input("value", g.op("to-int", vertex))
        .input("seed", seed)
        .add()
        .out("hash");
    let split = g.op("separate-vector", h);
    let (x, y, z) = (split.out("x"), split.out("y"), split.out("z"));
    let turn = |n: Ref| g.op("mul", (n, 2.0 * PI)).first();
    let line = g.frame(Frame::new("1D").note("A slope in [-1, 1]."), || {
        g.op("sub", (g.op("mul", (x, 2.0)), 1.0)).first()
    });
    let plane = g.frame(
        Frame::new("2D").note("A direction at a random angle."),
        || {
            let angle = turn(x);
            g.vec(WgslType::Vec2f, (g.op("cos", angle), g.op("sin", angle)))
                .first()
        },
    );
    let space = g.frame(
        Frame::new("3D").note(
            "One of OpenSimplex2's 48 directions, an expanded rhombic dodecahedron's:\na cube corner, one of its 3 edges (zeroing an axis), and one of 2 points\non that face, all made the same length.",
        ),
        || {
            let k = g.op("min", (g.op("floor", g.op("mul", (x, 48.0))), 47.0)).first();
            let bits = g.op(
                "floor",
                g.op(
                    "div",
                    (k, g.constant(WgslType::Vec3f, InputValue::vector(&[1.0, 2.0, 4.0]))),
                ),
            );
            let cube = g
                .op("sub", (g.op("mul", (g.op("mod", (bits, 2.0)), 2.0)), 1.0))
                .first();
            let axis = g.op("floor", g.op("div", (k, 16.0))).first();
            let distance = g.op(
                "abs",
                g.op(
                    "sub",
                    (
                        g.constant(WgslType::Vec3f, InputValue::vector(&[0.0, 1.0, 2.0])),
                        axis,
                    ),
                ),
            );
            let kept = g.op("step", (0.5, distance)).first();
            let edge = g.op("mul", (cube, kept)).first();
            let face = g
                .op("mod", (g.op("floor", g.op("div", (k, 8.0))), 2.0))
                .first();
            let cross = g.op("cross", (cube, edge));
            let rhomb = g.op("mix", (cube, g.op("add", (edge, cross)), face));
            g.op(
                "mul",
                (
                    g.op("add", (g.op("mul", (edge, 1.22474487139)), rhomb)),
                    g.op("sub", (1.0, g.op("mul", (face, 0.042942436724648037)))),
                ),
            )
            .first()
        },
    );
    let hyper = g.frame(
        Frame::new("4D").note(
            "A direction spread evenly over the 4D sphere: two circles\nwhose radii share the length (r₁² + r₂² = 1) at random.",
        ),
        || {
            let phi = turn(y);
            let psi = turn(z);
            let r1 = g.op("sqrt", x).first();
            let r2 = g.op("sqrt", g.op("sub", (1.0, x))).first();
            g.vec(
                WgslType::Vec4f,
                (
                    g.op("mul", (r1, g.op("cos", phi))),
                    g.op("mul", (r1, g.op("sin", phi))),
                    g.op("mul", (r2, g.op("cos", psi))),
                    g.op("mul", (r2, g.op("sin", psi))),
                ),
            )
            .first()
        },
    );
    let gradient = g.width_switch(vertex, (line, plane, space, hyper));
    g.output("gradient", gradient, Output::new(&t).label("Gradient"));
}

fn simplex_kernel_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let vertex = g.input("vertex", Input::new(&t).label("Vertex"));
    let offset = g.input("offset", Input::new(&t).label("Offset"));
    let radius = g.input(
        "radius",
        Input::new(WgslType::F32).label("Radius²").default(0.5),
    );
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    let gradient = g
        .call(LATTICE_GRADIENT)
        .input("vertex", vertex)
        .input("seed", seed)
        .add()
        .out("gradient");
    g.frame(
        Frame::new("Kernel").note(
            "The vertex's gradient along the offset to it, faded by (r² − |offset|²)⁴:\nnothing past the radius, and smooth to its edge.",
        ),
        || {
            let offset_sq = component_sum(g, g.op("mul", (offset, offset)));
            let falloff = g.op("max", (g.op("sub", (radius, offset_sq)), 0.0)).first();
            let squared = g.op("mul", (falloff, falloff)).first();
            let along = component_sum(g, g.op("mul", (gradient, offset)));
            g.output(
                "value",
                g.op("mul", (squared, squared, along)),
                Output::new(WgslType::F32).label("Value"),
            );
        },
    );
}

/// A simplex kernel's value at `offset` from the lattice point `vertex`.
pub fn kernel(
    g: &GroupBuilder<'_>,
    vertex: impl Into<Operand>,
    offset: impl Into<Operand>,
    radius: f64,
    seed: impl Into<Operand>,
) -> Ref {
    g.call(SIMPLEX_KERNEL)
        .input("vertex", vertex)
        .input("offset", offset)
        .input("radius", radius)
        .input("seed", seed)
        .add()
        .out("value")
}

/// Which lattice copy a seed is offset for: a known index, or one computed at run time.
pub enum LatticeCopy {
    Index(i64),
    Dynamic(Ref),
}

/// `seed` offset for the lattice copy `copy`.
pub fn copy_seed(g: &GroupBuilder<'_>, seed: Ref, copy: LatticeCopy) -> Ref {
    let salt = match copy {
        // Adding zero leaves the seed as it is.
        LatticeCopy::Index(0) => return seed,
        LatticeCopy::Index(index) => g
            .constant(WgslType::I32, index as f64 * LATTICE_SALT)
            .first(),
        LatticeCopy::Dynamic(index) => g
            .op("mul", (index, g.constant(WgslType::I32, LATTICE_SALT)))
            .first(),
    };
    g.op("add", (seed, salt)).first()
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            LATTICE_GRADIENT,
            "Lattice Gradient",
            "texture",
            &["vertex", "seed"],
            &["gradient"],
            lattice_gradient_body,
        )
        .aliases(&["Random Gradient", "Gradient Vector", "Random Direction"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            SIMPLEX_KERNEL,
            "Simplex Kernel",
            "texture",
            &["vertex", "offset", "radius", "seed"],
            &["value"],
            simplex_kernel_body,
        )
        .aliases(&["Surflet", "Gradient Kernel", "Simplex Contribution"])
        .helper(&["OpenSimplex"]),
    ]
}
