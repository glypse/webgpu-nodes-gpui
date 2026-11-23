//! Builders several families share: comparisons, boolean logic, single-socket
//! group calls and vec3 interfaces.

use shader_graph::types::WgslType;

use crate::builder::{GroupBuilder, Input, Operand, Output, Ref};

/// `a` compared to `b` by `op` (a compare node's setting), as a bool.
pub fn compare(
    g: &GroupBuilder<'_>,
    a: impl Into<Operand>,
    b: impl Into<Operand>,
    op: &str,
) -> Ref {
    g.node("compare")
        .args((a, b))
        .setting("op", op)
        .add()
        .first()
}

/// `a` and `b` joined by a boolean operator (a boolean math node's setting).
pub fn boolean(
    g: &GroupBuilder<'_>,
    a: impl Into<Operand>,
    b: impl Into<Operand>,
    op: &str,
) -> Ref {
    g.node("boolean-math")
        .args((a, b))
        .setting("op", op)
        .add()
        .first()
}

/// The output `output` of the group `target`, fed `value` on its input `input`.
pub fn call_one(
    g: &GroupBuilder<'_>,
    target: &str,
    input: &str,
    value: impl Into<Operand>,
    output: &str,
) -> Ref {
    g.call(target).input(input, value).add().out(output)
}

pub fn vec3_in(g: &GroupBuilder<'_>, key: &str, label: &str) -> Ref {
    g.input(key, Input::new(WgslType::Vec3f).label(label))
}

pub fn vec3_out(g: &GroupBuilder<'_>, key: &str, source: impl Into<Operand>, label: &str) {
    g.output(key, source, Output::new(WgslType::Vec3f).label(label));
}

/// A float input in `0..=1`.
pub fn unit(input: Input) -> Input {
    input.min(0.0).max(1.0)
}

/// The canvas' size in pixels, as a vec2f.
pub fn resolution(g: &GroupBuilder<'_>) -> Ref {
    g.node("resolution").add().first()
}

/// The texture coordinates of the fragment, as a vec2f.
pub fn uv(g: &GroupBuilder<'_>) -> Ref {
    g.node("uv").add().first()
}
