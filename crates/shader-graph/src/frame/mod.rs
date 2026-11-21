//! The frame tier of values: those the same for a whole frame, computed on
//! the CPU and fed to the passes that read them.

mod analysis;
mod program;

pub use analysis::*;
pub use program::*;
