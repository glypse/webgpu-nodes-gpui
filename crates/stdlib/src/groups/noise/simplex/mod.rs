//! OpenSimplex noise: lattices of kernels summed at each point.

mod kernel;
mod lattices;
mod orientation;
mod texture;

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = kernel::all();
    groups.extend(orientation::all());
    groups.extend(lattices::all());
    groups.extend(texture::all());
    groups
}
