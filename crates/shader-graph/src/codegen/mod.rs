//! WGSL code generation and the program (frame and uniform layout) around it.

mod generate;
mod helpers;
mod literals;
mod plan;
mod program;
mod target;
mod texture_read;

pub use generate::*;
pub use helpers::*;
pub use literals::*;
pub use plan::*;
pub use program::*;
pub use target::{RenderTextureTarget, ShaderTarget, SimulationStage, SimulationTarget};
pub use texture_read::*;

use crate::graph::GraphProject;
use crate::types::TypeResolver;

/// A generated frame: the program drawing it, after the passes it reads from.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// The pass drawing the frame (the plan's last).
    pub program: ShaderProgram,
    /// The uniform values of every pass.
    pub uniforms: Vec<(String, UniformValue)>,
    /// The lines of `program`'s code each node's statements are on, by node id.
    pub line_map: std::collections::BTreeMap<String, std::ops::Range<usize>>,
    pub plan: Box<RenderPlan>,
}

/// Generate the program drawing a project's root document.
pub fn generate_program(
    project: &GraphProject,
    resolver: &TypeResolver,
    options: &ShaderOptions,
) -> Result<Program, CodegenError> {
    let plan = generate_render_plan(CodegenGraph { project, resolver }, options)?;
    let target = plan.target().ok_or_else(|| CodegenError::Unsupported("the frame has no pass".to_owned()))?;
    Ok(Program {
        program: target.program.clone(),
        uniforms: plan.uniforms.clone(),
        line_map: target.line_map.clone(),
        plan: Box::new(plan),
    })
}
