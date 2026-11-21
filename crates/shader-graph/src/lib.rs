//! Headless shader graph: document model, type inference and WGSL codegen.

pub mod catalog;
pub mod codegen;
pub mod enums;
pub mod graph;
pub mod library;
pub mod nodes;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
pub mod types;
pub mod zones;

pub use codegen::{ShaderProgram, UniformBlock, UniformField, UniformScalar, UniformValue};
