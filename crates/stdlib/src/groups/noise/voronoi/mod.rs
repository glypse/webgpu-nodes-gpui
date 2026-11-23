//! Voronoi noise: the distances to the nearest cell points, and the textures
//! built from them.

mod cells;
mod metric;
mod texture;

use crate::builder::GroupSpec;

pub fn all() -> Vec<GroupSpec> {
    let mut groups = metric::all();
    groups.extend(cells::all());
    groups.extend(texture::all());
    groups
}
