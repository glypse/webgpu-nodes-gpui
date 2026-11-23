//! Helpers generic over a float vector's width: each instance computes only
//! its own width's case, so a vec2f pays for two components, not four.

use shader_graph::types::WgslType;

use crate::builder::{GroupBuilder, GroupSpec, Input, Output, Ref};

/// The widths the helpers take, scalars included.
const WIDTHS: [WgslType; 4] = [
    WgslType::Vec3f,
    WgslType::Vec2f,
    WgslType::Vec4f,
    WgslType::F32,
];

pub const PAD_VECTOR: &str = "std-pad-vector";
pub const COMPONENT_SUM: &str = "std-component-sum";
pub const MAX_COMPONENT: &str = "std-max-component";

/// The input every helper takes, its width a variable of the group.
fn vector_input(g: &GroupBuilder<'_>) -> Ref {
    let t = g.type_variable("t", "T", &WIDTHS);
    g.input("vector", Input::new(&t).label("Vector"))
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(PAD_VECTOR, "Pad to vec4f", "vector", &["vector"], &["padded"], |g| {
            let vector = vector_input(g);
            g.frame(
                crate::builder::Frame::new("Padding").note(
                    "Zeros past the vector's own components:\na scalar would be spread into all four.",
                ),
                || {
                    let widened = g.vec(WgslType::Vec4f, (vector, 0, 0, 0));
                    let padded = g.width_switch_as(
                        WgslType::Vec4f,
                        vector,
                        (widened, vector, vector, vector),
                    );
                    g.output("padded", padded, Output::new(WgslType::Vec4f).label("Padded"));
                },
            );
        })
        .aliases(&["Zero Pad", "Widen Vector", "Extend Vector", "Pad Vector"])
        .helper(&[]),
        GroupSpec::new(COMPONENT_SUM, "Component Sum", "vector", &["vector"], &["sum"], |g| {
            let vector = vector_input(g);
            // WGSL's dot takes no scalars.
            let dot = g.op("dot", (vector, 1));
            let sum = g.width_switch(vector, (vector, dot, dot, dot));
            g.output("sum", sum, Output::new(WgslType::F32).label("Sum"));
        })
        .aliases(&["Sum Components", "Add Components", "Horizontal Add", "Manhattan Length"])
        .helper(&[]),
        GroupSpec::new(MAX_COMPONENT, "Max Component", "vector", &["vector"], &["max"], |g| {
            let vector = vector_input(g);
            // Split padded so every width finds its components on the same node.
            let padded = g.call(PAD_VECTOR).input("vector", vector).add().out("padded");
            let split = g.op("separate-vector", padded);
            let (x, y, z, w) = (split.out("x"), split.out("y"), split.out("z"), split.out("w"));
            let max = g.width_switch(
                vector,
                (
                    vector,
                    g.op("max", (x, y)),
                    g.op("max", (x, y, z)),
                    g.op("max", (x, y, z, w)),
                ),
            );
            g.output("max", max, Output::new(WgslType::F32).label("Max"));
        })
        .aliases(&["Largest Component", "Horizontal Max", "Chebyshev Length"])
        .helper(&[]),
    ]
}
