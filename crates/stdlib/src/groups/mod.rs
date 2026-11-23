//! The standard library's groups, described.

use crate::builder::GroupSpec;

mod color;
mod coordinates;
mod helpers;
mod math;
mod texture;
mod vector;

/// Every described group.
pub fn all() -> Vec<GroupSpec> {
    let mut groups = vector::all();
    groups.extend(math::all());
    groups.extend(color::all());
    groups.extend(coordinates::all());
    groups.extend(texture::all());
    groups
}
