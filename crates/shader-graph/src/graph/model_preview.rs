//! Previewing a node: its output is linked straight to the fragment output,
//! replacing what was there. Inside groups, each group on the way gets a
//! viewer output carrying it up a level; viewer outputs a preview no longer
//! goes through are removed, so previews never pile up.

use super::model::GraphModel;
use super::{FRAG_OUTPUT, GROUP_OUTPUT, GraphEdge, GroupSide, ROOT_ID, new_id};
use crate::library::is_library_ref;
use crate::nodes::registered;
use crate::types::{GroupHandleDef, HandleType, is_connection_compatible};

/// A node's output, as a link's source.
struct Source {
    node: String,
    handle: String,
}

impl GraphModel {
    /// Preview a node of the active document: its first output that the
    /// fragment output takes, or the next one when it's already previewed.
    /// Returns whether it's previewed.
    pub fn preview_node(&mut self, node_id: &str) -> bool {
        let path = self.path().to_vec();
        if self.is_locked_at(&path) {
            return false;
        }
        let Some(frag) = self.root().nodes.iter().find(|n| n.is(FRAG_OUTPUT)).map(|n| n.id.clone()) else {
            return false;
        };
        let Some(frag_input) = registered(FRAG_OUTPUT).and_then(|d| d.inputs.first().cloned()) else {
            return false;
        };
        let outputs: Vec<String> = self
            .resolve_node_outputs(node_id)
            .into_iter()
            .filter(|o| frag_input.types.iter().any(|t| is_connection_compatible(o.ty, *t)))
            .map(|o| o.id)
            .collect();
        if outputs.is_empty() || self.active_document().node(node_id).is_none() {
            return false;
        }

        // From the node's level up to the root: the definition and the group
        // node leaving it at each step.
        let levels: Vec<(String, String)> = (0..path.len())
            .rev()
            .map(|i| (self.definition_ref_at(&path[..=i]), path[i].clone()))
            .collect();
        let (target_doc, target_node, target_handle) = match levels.first() {
            Some((definition, _)) => {
                let (node, handle) = self.viewer_of(definition);
                (definition.clone(), node, handle)
            }
            None => (ROOT_ID.to_owned(), Some(frag.clone()), Some(frag_input.id.clone())),
        };
        let current = self.project.document(&target_doc).and_then(|d| {
            d.edges.iter().find(|e| {
                e.source == node_id
                    && Some(&e.target) == target_node.as_ref()
                    && Some(&e.target_handle) == target_handle.as_ref()
            })
        });
        let next = current
            .and_then(|e| outputs.iter().position(|o| *o == e.source_handle))
            .map_or(0, |i| (i + 1) % outputs.len());
        let Some(output) = outputs.get(next).or(outputs.first()) else { return false };

        let mut kept = Vec::new();
        let mut source = Source { node: node_id.to_owned(), handle: output.clone() };
        for (definition, instance) in &levels {
            let Some(group_output) = self.viewer_of(definition).0 else { return false };
            let Some(viewer) = self.ensure_viewer(definition) else { return false };
            kept.push(viewer.id.clone());
            self.link_into(definition, &source, &Source { node: group_output, handle: viewer.id.clone() });
            source = Source { node: instance.clone(), handle: viewer.id };
        }
        self.link_into(ROOT_ID, &source, &Source { node: frag, handle: frag_input.id });
        self.remove_viewers_but(&kept);
        self.changed();
        true
    }

    /// A group's viewer output's Group Output node, the one it's linked on or
    /// else the first, and the viewer output's id.
    fn viewer_of(&self, definition: &str) -> (Option<String>, Option<String>) {
        let Some(def) = self.project.document(definition) else { return (None, None) };
        let handle = def.outputs.iter().find(|h| h.viewer == Some(true)).map(|h| h.id.clone());
        let outputs: Vec<_> = def.nodes.iter().filter(|n| n.is(GROUP_OUTPUT)).collect();
        let linked = outputs
            .iter()
            .find(|n| def.edges_into(&n.id).any(|e| Some(&e.target_handle) == handle.as_ref()));
        (linked.or(outputs.first()).map(|n| n.id.clone()), handle)
    }

    /// A group's viewer output, added if it has none.
    fn ensure_viewer(&mut self, definition: &str) -> Option<GroupHandleDef> {
        let def = self.project.definitions.get_mut(definition)?;
        if let Some(existing) = def.outputs.iter().find(|h| h.viewer == Some(true)) {
            return Some(existing.clone());
        }
        let mut viewer = GroupHandleDef::new(new_id(), "Viewer", HandleType::Inferred { inferred: true });
        viewer.viewer = Some(true);
        def.outputs.push(viewer.clone());
        Some(viewer)
    }

    /// Link `source` into an input of the document, replacing what fed it.
    fn link_into(&mut self, document: &str, source: &Source, target: &Source) {
        let Some(doc) = self.project.definitions.get_mut(document) else { return };
        doc.edges.retain(|e| !(e.target == target.node && e.target_handle == target.handle));
        doc.edges.push(GraphEdge {
            id: new_id(),
            source: source.node.clone(),
            source_handle: source.handle.clone(),
            target: target.node.clone(),
            target_handle: target.handle.clone(),
        });
    }

    /// Remove every group's viewer output but those in `kept`, with its links.
    fn remove_viewers_but(&mut self, kept: &[String]) {
        let stale: Vec<(String, Vec<String>)> = self
            .project
            .definitions
            .iter()
            .filter(|(key, _)| !is_library_ref(key))
            .map(|(key, def)| {
                let ids = def
                    .outputs
                    .iter()
                    .filter(|h| h.viewer == Some(true) && !kept.contains(&h.id))
                    .map(|h| h.id.clone())
                    .collect();
                (key.clone(), ids)
            })
            .filter(|(_, ids): &(String, Vec<String>)| !ids.is_empty())
            .collect();
        for (key, ids) in stale {
            self.remove_handles(&key, GroupSide::Output, &ids);
        }
    }
}
