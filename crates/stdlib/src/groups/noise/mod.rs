//! Noise textures and the helpers they share.

mod coordinates;
mod lattice;

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = coordinates::all();
    groups.extend(lattice::all());
    groups
}
