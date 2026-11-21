//! The node registry: every node type's descriptor.

mod builders;
mod curve;
mod dynamic;
mod standard;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

pub use builders::*;
pub use dynamic::*;
pub use standard::{COMPONENT_COUNT, COMPONENT_COUNT_INPUT};

use crate::enums::{EnumDocument, MATCH, MATCH_SELECTOR, WIDTH_SWITCH};
use crate::graph::{GROUP_INPUT, GROUP_OUTPUT, GraphNode, GraphProject, NODE_GROUP};
use crate::types::{NodeData, NodeDescriptor};
use crate::zones::{build_zone_descriptor, is_zone_node_type};

/// Node types whose descriptor is built from their node, not registered.
pub const DYNAMIC_NODE_TYPES: [&str; 9] = [
    GROUP_INPUT,
    GROUP_OUTPUT,
    crate::zones::REPEAT_INPUT,
    crate::zones::REPEAT_OUTPUT,
    crate::zones::SIMULATION_INPUT,
    crate::zones::SIMULATION_OUTPUT,
    NODE_GROUP,
    MATCH,
    INDEX_SWITCH,
];

struct Registry {
    ordered: Vec<Arc<NodeDescriptor>>,
    by_type: HashMap<String, Arc<NodeDescriptor>>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    let mut all = Vec::new();
    all.extend(standard::input_nodes());
    all.extend(standard::math_nodes());
    all.push(curve::curve_node());
    all.extend(standard::vector_nodes());
    all.extend(standard::logic_nodes());
    all.extend(standard::integer_nodes());
    all.extend(standard::converter_nodes());
    all.extend(standard::color_nodes());
    all.push(width_switch_node());
    all.push(build_match_descriptor(&[]));
    all.push(build_index_switch_descriptor(&NodeData::default()));
    all.push(build_custom_wgsl_descriptor(&NodeData::default()));
    all.extend(standard::misc_nodes());
    let ordered: Vec<_> = all.into_iter().map(Arc::new).collect();
    let by_type = ordered.iter().map(|d| (d.type_id.clone(), d.clone())).collect();
    Registry { ordered, by_type }
});

/// The registered descriptors, in the order the add menu lists them.
pub fn registered_nodes() -> &'static [Arc<NodeDescriptor>] {
    &REGISTRY.ordered
}

/// A registered node type's descriptor.
pub fn registered(node_type: &str) -> Option<Arc<NodeDescriptor>> {
    REGISTRY.by_type.get(node_type).cloned()
}

/// Whether `node_type` is a type a saved node may have.
pub fn is_known_node_type(node_type: &str) -> bool {
    REGISTRY.by_type.contains_key(node_type) || DYNAMIC_NODE_TYPES.contains(&node_type)
}

/// Default node data for a registered type.
pub fn default_data_for_type(node_type: &str) -> NodeData {
    registered(node_type).map(|d| d.default_data.clone()).unwrap_or_default()
}

/// A node's descriptor: registered, or built from the node and its document.
pub fn resolve_descriptor(node: &GraphNode, project: &GraphProject) -> Arc<NodeDescriptor> {
    let doc = || project.document_of(&node.id);
    match node.node_type.as_str() {
        GROUP_INPUT => {
            let doc = doc();
            Arc::new(build_group_input_descriptor(
                doc.map_or(&[][..], |d| &d.inputs),
                doc.map_or(&[][..], |d| &d.type_variables),
            ))
        }
        GROUP_OUTPUT => {
            let doc = doc();
            Arc::new(build_group_output_descriptor(
                doc.map_or(&[][..], |d| &d.outputs),
                doc.map_or(&[][..], |d| &d.type_variables),
            ))
        }
        NODE_GROUP => Arc::new(build_group_descriptor(node, project)),
        t if is_zone_node_type(t) => {
            let variables = doc().map_or(&[][..], |d| d.type_variables.as_slice());
            Arc::new(build_zone_descriptor(node, project, variables))
        }
        CUSTOM_WGSL => Arc::new(build_custom_wgsl_descriptor(&node.data)),
        INDEX_SWITCH => Arc::new(build_index_switch_descriptor(&node.data)),
        MATCH => {
            let variants = doc()
                .and_then(|d| EnumDocument::of(d).enum_at(&node.id, MATCH_SELECTOR).map(<[_]>::to_vec))
                .unwrap_or_default();
            Arc::new(build_match_descriptor(&variants))
        }
        other => registered(other)
            .or_else(|| registered("declare"))
            .expect("the registry has Declare"),
    }
}

/// The type name of the node deciding cases by component count.
pub const WIDTH_SWITCH_TYPE: &str = WIDTH_SWITCH;
