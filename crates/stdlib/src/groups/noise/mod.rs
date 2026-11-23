//! Noise textures and the helpers they share.

mod coordinates;
mod lattice;
mod simplex;

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = coordinates::all();
    groups.extend(lattice::all());
    groups.extend(simplex::all());
    groups
}
