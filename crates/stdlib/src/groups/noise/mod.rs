//! Noise textures and the helpers they share.

mod coordinates;
mod fractal;
mod lattice;
mod simplex;
mod voronoi;

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = coordinates::all();
    groups.extend(lattice::all());
    groups.extend(fractal::all());
    groups.extend(simplex::all());
    groups.extend(voronoi::all());
    groups
}
