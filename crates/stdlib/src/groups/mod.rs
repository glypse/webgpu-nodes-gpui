//! The standard library's groups, described.

use crate::builder::GroupSpec;

mod vector;

/// Every described group.
pub fn all() -> Vec<GroupSpec> {
    vector::all()
}
