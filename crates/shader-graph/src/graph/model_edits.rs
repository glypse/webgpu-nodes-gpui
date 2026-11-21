//! Edits of links and of what a group node decides about its types.

use std::collections::{HashMap, HashSet};

use super::model::GraphModel;
use super::{GraphEdge, GraphNode, Point, REROUTE, new_id};
use crate::nodes::default_data_for_type;
use crate::types::{TypeVariable, WgslType};

/// What a group node's type variable takes, and who decides.
#[derive(Clone, Debug, PartialEq)]
pub struct VariableChoice {
    pub variable: TypeVariable,
    pub ty: WgslType,
    pub inferred: WgslType,
    pub forced: Option<WgslType>,
    /// Whether links can decide it: some input socket has its type.
    pub inferable: bool,
}

impl GraphModel {
    /// Hide the links out of reroutes (see `NodeData::hide_links`), or show
    /// them again when every one of them already hides them.
    pub fn toggle_hidden_links(&mut self, node_ids: &[String]) {
        if self.is_locked() {
            return;
        }
        let reroutes: Vec<String> = node_ids
            .iter()
            .filter(|id| self.node(id).is_some_and(|n| n.is(REROUTE)))
            .cloned()
            .collect();
        let hide = reroutes.iter().any(|id| self.node(id).is_some_and(|n| n.data.hide_links != Some(true)));
        for id in &reroutes {
            self.update_node_data(id, |d| d.hide_links = hide.then_some(true));
        }
    }

    /// Route links of the active document through new reroute nodes at the
    /// positions given. Links leaving the same output share one reroute (at
    /// the middle of their positions) which then branches out to their
    /// targets. Returns the reroutes' ids.
    pub fn reroute_edges(&mut self, cuts: &[(String, Point)]) -> Vec<String> {
        if self.is_locked() {
            return Vec::new();
        }
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else { return Vec::new() };
        let mut order: Vec<(String, String)> = Vec::new();
        let mut by_source: HashMap<(String, String), (Vec<GraphEdge>, Vec<Point>)> = HashMap::new();
        for (edge_id, position) in cuts {
            let Some(edge) = doc.edge(edge_id) else { continue };
            let source = (edge.source.clone(), edge.source_handle.clone());
            let entry = by_source.entry(source.clone()).or_insert_with(|| {
                order.push(source);
                (Vec::new(), Vec::new())
            });
            if entry.0.iter().any(|e| e.id == edge.id) {
                continue;
            }
            entry.0.push(edge.clone());
            entry.1.push(*position);
        }
        let mut cut: HashSet<String> = HashSet::new();
        let (mut reroutes, mut links): (Vec<GraphNode>, Vec<GraphEdge>) = (Vec::new(), Vec::new());
        for source in &order {
            let Some((edges, positions)) = by_source.get(source) else { continue };
            let Some(first) = edges.first() else { continue };
            let count = positions.len() as f32;
            let at = Point::new(
                positions.iter().map(|p| p.x).sum::<f32>() / count,
                positions.iter().map(|p| p.y).sum::<f32>() / count,
            );
            let mut reroute = GraphNode::new(REROUTE, at);
            reroute.data = default_data_for_type(REROUTE);
            links.push(GraphEdge {
                id: new_id(),
                source: first.source.clone(),
                source_handle: first.source_handle.clone(),
                target: reroute.id.clone(),
                target_handle: "in".to_owned(),
            });
            for edge in edges {
                cut.insert(edge.id.clone());
                links.push(GraphEdge {
                    id: new_id(),
                    source: reroute.id.clone(),
                    source_handle: "out".to_owned(),
                    target: edge.target.clone(),
                    target_handle: edge.target_handle.clone(),
                });
            }
            reroutes.push(reroute);
        }
        if reroutes.is_empty() {
            return Vec::new();
        }
        let ids = reroutes.iter().map(|n| n.id.clone()).collect();
        let Some(doc) = self.doc_mut(&key) else { return Vec::new() };
        doc.nodes.extend(reroutes);
        doc.edges.retain(|e| !cut.contains(&e.id));
        doc.edges.extend(links);
        self.changed();
        ids
    }

    /// A group node's type variables: the type each takes, the one forced on
    /// the node if any, and whether links can decide it.
    pub fn variable_choices(&self, node_id: &str) -> Vec<VariableChoice> {
        let Some(node) = self.node(node_id) else { return Vec::new() };
        let Some(def) = node.definition_ref().and_then(|r| self.project.document(r)) else {
            return Vec::new();
        };
        let Some(doc_ref) = self.project.definition_ref_of(node_id) else { return Vec::new() };
        let bindings = self.view_bindings(doc_ref, &self.path);
        let resolved = self.resolver.resolve(&self.project, node, &bindings);
        let mut unforced = node.data.clone();
        unforced.types.clear();
        let free = self.resolver.resolve_with(&self.project, node, &unforced, &bindings);
        def.type_variables
            .iter()
            .map(|variable| {
                let inferable = def.inputs.iter().any(|h| h.ty.variable() == Some(variable.id.as_str()));
                let fallback = variable.types.first().copied().unwrap_or(WgslType::F32);
                VariableChoice {
                    variable: variable.clone(),
                    ty: resolved.bindings.as_ref().and_then(|b| b.get(&variable.id)).copied().unwrap_or(fallback),
                    inferred: if inferable {
                        free.bindings.as_ref().and_then(|b| b.get(&variable.id)).copied().unwrap_or(fallback)
                    } else {
                        fallback
                    },
                    forced: node.data.types.get(&variable.id).copied(),
                    inferable,
                }
            })
            .collect()
    }
}
