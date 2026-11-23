use crate::types::{InputValue, ScalarKind, WgslType, scalar_number_of};

/// WGSL identifiers can't contain characters like the dashes of a UUID.
pub fn ident_suffix(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn number(n: f64) -> String {
    if n != 0.0 && (n.abs() >= 1e15 || n.abs() < 1e-6) {
        format!("{n:e}")
    } else {
        format!("{n}")
    }
}

fn scalar_literal(kind: ScalarKind, value: Option<f64>) -> String {
    let n = scalar_number_of(kind, value);
    if kind == ScalarKind::Bool {
        return if n == 1.0 { "true" } else { "false" }.to_owned();
    }
    format!("{}({})", kind.name(), number(n))
}

/// Format a stored input value as a WGSL literal of `ty`.
pub fn format_wgsl_literal(ty: WgslType, value: Option<&InputValue>) -> String {
    let kind = ty.kind();
    let components = ty.components();
    if components == 1 {
        return scalar_literal(kind, value.map(|v| v.component(0)));
    }
    match value {
        None => ty.zero(),
        Some(InputValue::Vector(v)) => {
            let parts: Vec<String> = (0..components)
                .map(|i| scalar_literal(kind, v.component(i).map(|s| s.as_f64())))
                .collect();
            format!("{}({})", ty.wgsl_name(), parts.join(", "))
        }
        Some(scalar) => {
            format!(
                "{}({})",
                ty.wgsl_name(),
                scalar_literal(kind, Some(scalar.component(0)))
            )
        }
    }
}

/// A uniform's value: its f32 components. Uniforms are always floats,
/// converted to the input's type in the shader.
#[derive(Clone, Debug, PartialEq)]
pub struct UniformValue(pub Vec<f32>);

impl UniformValue {
    pub fn components(&self) -> usize {
        self.0.len()
    }
}

/// The uniform holding `value`, read as `target`.
pub fn uniform_value(target: WgslType, value: Option<&InputValue>) -> UniformValue {
    let kind = target.kind();
    UniformValue(
        (0..target.components())
            .map(|i| scalar_number_of(kind, value.map(|v| v.component(i))) as f32)
            .collect(),
    )
}

/// The name of the uniform holding a node's unlinked input or own number.
pub fn uniform_name(node_id: &str, key: &str) -> String {
    format!("u_{}_{}", ident_suffix(node_id), ident_suffix(key))
}
