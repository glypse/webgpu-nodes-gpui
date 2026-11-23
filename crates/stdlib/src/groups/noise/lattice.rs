//! Lattice noises cut space into unit cells: the cells around a point, and
//! random values per cell.

use shader_graph::types::WgslType;

use super::coordinates::coordinates_type;
use crate::builder::{Frame, GroupBuilder, GroupSpec, Input, Output, Ref};

pub const HASH: &str = "std-hash";
pub const LATTICE_NEIGHBOR: &str = "std-lattice-neighbor";

fn round(g: &GroupBuilder<'_>, v: Ref) -> Ref {
    let split = g.op("separate-vector", v);
    let (x, y, z, w) = (
        split.out("x"),
        split.out("y"),
        split.out("z"),
        split.out("w"),
    );
    let x1 = g.op("add", (x, g.op("mul", (y, w)))).first();
    let y1 = g.op("add", (y, g.op("mul", (z, x1)))).first();
    let z1 = g.op("add", (z, g.op("mul", (x1, y1)))).first();
    let w1 = g.op("add", (w, g.op("mul", (y1, z1)))).first();
    g.vec(WgslType::Vec4i, (x1, y1, z1, w1)).first()
}

fn int(g: &GroupBuilder<'_>, n: f64) -> Ref {
    g.constant(WgslType::I32, n).first()
}

fn hash_body(g: &GroupBuilder<'_>) {
    let t = g.type_variable(
        "t",
        "T",
        &[
            WgslType::Vec4i,
            WgslType::Vec3i,
            WgslType::Vec2i,
            WgslType::I32,
        ],
    );
    let value = g.input("value", Input::new(&t).label("Value"));
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    let padded = g.frame(
        Frame::new("Padding").note(
            "Zeros past the value's own components, so a cell hashes\nthe same whatever its dimensions; a scalar would be spread.",
        ),
        || {
            let widened = g.vec(WgslType::Vec4i, (value, 0.0, 0.0, 0.0));
            g.width_switch_as(WgslType::Vec4i, value, (widened, value, value, value))
        },
    );
    let start = g.frame(
        Frame::new("Seed").note("Spread the value, as a linear congruential generator would."),
        || {
            let scaled = g.op("mul", (padded, int(g, 1664525.0)));
            g.op("add", (scaled, int(g, 1013904223.0), seed)).first()
        },
    );
    let mixed = g.frame(
        Frame::new("First round")
            .note("Each component plus the product of two others,\nthen the high bits folded in."),
        || {
            let first = round(g, start);
            let shifted = g
                .node("bit-shift")
                .args((first, 16.0))
                .setting("op", ">>")
                .add()
                .first();
            g.node("bitwise")
                .args((first, shifted))
                .setting("op", "^")
                .add()
                .first()
        },
    );
    g.frame(
        Frame::new("Second round").note("Again, then the sign bit dropped and scaled to [0, 1]."),
        || {
            let second = round(g, mixed);
            let positive = g
                .node("bitwise")
                .args((second, 2147483647.0))
                .setting("op", "&")
                .add()
                .first();
            let scaled = g.op("mul", (g.op("to-float", positive), 1.0 / 2147483647.0));
            g.output("hash", scaled, Output::new(WgslType::Vec4f).label("Hash"));
        },
    );
}

fn neighbor_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let index = g.input("index", Input::new(WgslType::I32).label("Index").default(0));
    let cell = g.input("cell", Input::new(&t).label("Cell"));
    let radius = g.input(
        "radius",
        Input::new(WgslType::I32)
            .label("Radius")
            .default(1)
            .min(0.0),
    );
    let side = g
        .op("add", (g.op("mul", (radius, int(g, 2.0))), int(g, 1.0)))
        .first();
    let side2 = g.op("mul", (side, side)).first();
    let side3 = g.op("mul", (side2, side)).first();
    let side4 = g.op("mul", (side3, side)).first();
    let count = g.width_switch(cell, (side, side2, side3, side4));
    let offset = g.frame(
        Frame::new("Offset").note(
            "The index's digits in base 2 × Radius + 1, x first,\nas many as Cell has components.",
        ),
        || {
            let bases = g.vec(WgslType::Vec4i, (1.0, side, side2, side3));
            let digits = g.op("mod", (g.op("div", (index, bases)), side));
            g.op("to-float", g.op("sub", (digits, radius))).first()
        },
    );
    g.output("offset", offset, Output::new(&t).label("Offset"));
    g.output(
        "neighbor",
        g.op("add", (cell, offset)),
        Output::new(&t).label("Neighbor"),
    );
    g.output("count", count, Output::new(WgslType::I32).label("Count"));
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            HASH,
            "Hash",
            "texture",
            &["value", "seed"],
            &["hash"],
            hash_body,
        )
        .aliases(&[
            "Random",
            "White Noise",
            "PCG",
            "Integer Hash",
            "Random Vector",
        ])
        .helper(&[]),
        GroupSpec::new(
            LATTICE_NEIGHBOR,
            "Lattice Neighbor",
            "texture",
            &["index", "cell", "radius"],
            &["offset", "neighbor", "count"],
            neighbor_body,
        )
        .aliases(&["Neighbor Cell", "Cell Offset", "Grid Neighbor"])
        .helper(&[]),
    ]
}
