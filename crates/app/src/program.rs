use shader_graph::codegen::Program;

/// The generated program for a shader the editor produced.
pub fn program_source(code: String) -> Program {
    shader_graph::legacy::program(code)
}
