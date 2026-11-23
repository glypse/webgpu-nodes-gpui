//! The standard library's groups, described.

use crate::builder::GroupSpec;

mod math;
mod vector;

/// Every described group.
pub fn all() -> Vec<GroupSpec> {
    let mut groups = vector::all();
    groups.extend(math::all());
    groups
}
