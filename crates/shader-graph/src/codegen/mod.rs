//! WGSL code generation and the program (frame and uniform layout) around it.

mod generate;
mod helpers;
mod literals;
mod program;

pub use generate::*;
pub use helpers::*;
pub use literals::*;
pub use program::*;

use crate::graph::GraphProject;
use crate::types::TypeResolver;

/// A generated shader program and the uniform values it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub program: ShaderProgram,
    pub uniforms: Vec<(String, UniformValue)>,
    pub line_map: std::collections::BTreeMap<String, std::ops::Range<usize>>,
}

/// Generate the program drawing a project's root document.
pub fn generate_program(
    project: &GraphProject,
    resolver: &TypeResolver,
    options: &ShaderOptions,
) -> Result<Program, CodegenError> {
    let source = generate_shader(CodegenGraph { project, resolver }, options)?;
    let program = build_program(&source.fragment, &source.uniforms);
    Ok(Program { program, uniforms: source.uniforms, line_map: source.line_map })
}
