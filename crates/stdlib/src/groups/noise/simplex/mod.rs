//! OpenSimplex noise: lattices of kernels summed at each point.

mod kernel;
mod lattices;
mod orientation;

pub use kernel::{LatticeCopy, copy_seed, kernel};
pub use orientation::{FAST, SMOOTH, orientation_input, variant_input};

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = kernel::all();
    groups.extend(orientation::all());
    groups.extend(lattices::all());
    groups
}
