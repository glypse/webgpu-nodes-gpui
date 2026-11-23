//! Building projects tersely from nodes and links, and reading the shaders they generate.

use std::collections::BTreeMap;

use crate::codegen::ShaderOptions;
use crate::graph::{
    FRAG_OUTPUT, GraphDocument, GraphEdge, GraphModel, GraphNode, GraphProject, GroupRef, Point,
    ROOT_ID,
};
use crate::testing::validate_wgsl;
use crate::types::{InputValue, NodeData, WgslType};

/// A node at the origin.
pub fn node(id: &str, node_type: &str) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        ..GraphNode::new(node_type, Point::default())
    }
}

pub fn with_data(mut n: GraphNode, data: NodeData) -> GraphNode {
    n.data = data;
    n
}

/// A node holding `values` for its inputs.
pub fn with_values(n: GraphNode, values: &[(&str, InputValue)]) -> GraphNode {
    with_data(
        n,
        NodeData {
            values: values.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            ..Default::default()
        },
    )
}

/// A node of `definition_ref`'s group.
pub fn instance(id: &str, definition_ref: &str, data: NodeData) -> GraphNode {
    GraphNode {
        group: Some(GroupRef {
            definition_ref: definition_ref.to_owned(),
            locked: false,
        }),
        ..with_data(node(id, "node-group"), data)
    }
}

/// A Declare of `ty` holding `values`.
pub fn declare(id: &str, ty: WgslType, values: &[(&str, f64)]) -> GraphNode {
    let mut data = NodeData::default();
    data.types.insert("declare".to_owned(), ty);
    data.values = values
        .iter()
        .map(|(k, v)| ((*k).to_owned(), InputValue::from(*v)))
        .collect();
    with_data(node(id, "declare"), data)
}

pub fn types(entries: &[(&str, WgslType)]) -> BTreeMap<String, WgslType> {
    entries.iter().map(|(k, t)| ((*k).to_owned(), *t)).collect()
}

pub fn values(entries: &[(&str, f64)]) -> BTreeMap<String, InputValue> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), InputValue::from(*v)))
        .collect()
}

/// `(source, source handle, target, target handle)`.
pub type Link = (&'static str, &'static str, &'static str, &'static str);

pub fn edge(link: Link) -> GraphEdge {
    let (source, source_handle, target, target_handle) = link;
    GraphEdge {
        id: format!("{source}.{source_handle}->{target}.{target_handle}"),
        source: source.to_owned(),
        target: target.to_owned(),
        source_handle: source_handle.to_owned(),
        target_handle: target_handle.to_owned(),
    }
}

pub fn edges(links: &[Link]) -> Vec<GraphEdge> {
    links.iter().copied().map(edge).collect()
}

/// A one-document project with `nodes` and `links`, plus a fragment output with id "out".
pub fn root_project(nodes: Vec<GraphNode>, links: &[Link]) -> GraphProject {
    let root = GraphDocument {
        nodes: std::iter::once(node("out", FRAG_OUTPUT))
            .chain(nodes)
            .collect(),
        edges: edges(links),
        ..Default::default()
    };
    GraphProject::from_definitions(BTreeMap::from([(ROOT_ID.to_owned(), root)]))
}

pub fn model_for(nodes: Vec<GraphNode>, links: &[Link]) -> GraphModel {
    GraphModel::new(root_project(nodes, links))
}

/// The shader of the root document with every value compiled in.
pub fn shader(model: &GraphModel) -> String {
    model
        .generate_with(&ShaderOptions::default())
        .expect("generates")
        .program
        .code
}

/// The shader, which must be valid WGSL.
pub fn valid_shader(model: &GraphModel) -> String {
    let code = shader(model);
    validate_wgsl(&code);
    code
}
