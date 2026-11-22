//! The normalising pass on load: fills in the defaults of every node a file
//! saved without them.

use super::GraphProject;
use crate::nodes::{is_known_node_type, resolve_descriptor};

/// Fill each node's data with its type's defaults, under the values it saved.
/// Group nodes keep their data: their definitions own it.
pub fn normalise_project(project: &mut GraphProject) {
    // Descriptors of plain nodes don't depend on the rest of the project, so a
    // blank one stands in for it.
    let blank = GraphProject::default();
    for doc in project.definitions.values_mut() {
        for node in &mut doc.nodes {
            if node.group.is_some() || !is_known_node_type(&node.node_type) {
                continue;
            }
            let defaults = &resolve_descriptor(node, &blank).default_data;
            node.data = node.data.with_defaults(defaults);
        }
    }
}
