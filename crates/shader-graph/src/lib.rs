//! Headless shader graph: document model, type inference and WGSL codegen.

pub mod catalog;
pub mod codegen;
pub mod color;
pub mod curve;
pub mod enums;
pub mod graph;
pub mod library;
pub mod nodes;
pub mod passes;
pub mod ramp;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
pub mod types;
pub mod zones;

pub use codegen::{ShaderProgram, UniformBlock, UniformField, UniformScalar, UniformValue};
