//! Editing Custom WGSL nodes' code and sockets.

use super::model::GraphModel;
use super::{GraphNode, new_id};
use crate::nodes::{CUSTOM_WGSL, custom_sockets, is_socket_name, new_socket_label};
use crate::types::{CustomSockets, HandleDef, WgslType};

/// Longest code a node may hold, in bytes.
pub const MAX_CODE_LENGTH: usize = 1024 * 1024;

impl GraphModel {
    fn custom_node(&self, node_id: &str) -> Option<GraphNode> {
        let node = self.active_document().node(node_id)?;
        (node.is(CUSTOM_WGSL) && !self.is_locked()).then(|| node.clone())
    }

    pub fn set_custom_code(&mut self, node_id: &str, code: &str) {
        if self.custom_node(node_id).is_none() || code.len() > MAX_CODE_LENGTH {
            return;
        }
        self.update_node_data(node_id, |data| data.code = Some(code.to_owned()));
    }

    /// Write a node's sockets, dropping links to sockets gone or no longer fitting their type.
    fn set_custom_sockets(&mut self, node_id: &str, sockets: CustomSockets) {
        let key = self.active_definition_ref();
        {
            let Some(doc) = self.project.definitions.get_mut(&key) else { return };
            if let Some(node) = doc.node_mut(node_id) {
                node.data.sockets = Some(sockets.clone());
            }
            doc.edges.retain(|e| {
                !(e.target == node_id && !sockets.inputs.iter().any(|s| s.id == e.target_handle))
                    && !(e.source == node_id && !sockets.outputs.iter().any(|s| s.id == e.source_handle))
            });
        }
        self.project.reindex();
        self.resolver.clear();
        self.drop_unfit_links_among(&key, &[node_id.to_owned()]);
        self.changed();
    }

    fn update_sockets(
        &mut self,
        node_id: &str,
        input: bool,
        update: impl FnOnce(&mut Vec<HandleDef>),
    ) {
        let Some(node) = self.custom_node(node_id) else { return };
        let mut sockets = custom_sockets(&node.data);
        update(if input { &mut sockets.inputs } else { &mut sockets.outputs });
        self.set_custom_sockets(node_id, sockets);
    }

    /// Append a socket with a free name; returns it.
    pub fn add_custom_socket(&mut self, node_id: &str, input: bool, ty: WgslType) -> Option<HandleDef> {
        let node = self.custom_node(node_id)?;
        let socket = HandleDef::new(new_id(), new_socket_label(&custom_sockets(&node.data), input), ty);
        let added = socket.clone();
        self.update_sockets(node_id, input, |s| s.push(added));
        Some(socket)
    }

    pub fn remove_custom_socket(&mut self, node_id: &str, input: bool, socket_id: &str) {
        self.update_sockets(node_id, input, |s| s.retain(|x| x.id != socket_id));
    }

    /// Rename a socket, which is also the variable the code reads or writes.
    /// Rejects names that aren't WGSL identifiers or that another socket has;
    /// returns whether it was applied.
    pub fn rename_custom_socket(&mut self, node_id: &str, input: bool, socket_id: &str, label: &str) -> bool {
        let Some(node) = self.custom_node(node_id) else { return false };
        let name = label.trim();
        let sockets = custom_sockets(&node.data);
        if !is_socket_name(name)
            || sockets.inputs.iter().chain(&sockets.outputs).any(|s| s.id != socket_id && s.label == name)
        {
            return false;
        }
        self.update_sockets(node_id, input, |list| {
            for s in list.iter_mut().filter(|s| s.id == socket_id) {
                s.label = name.to_owned();
            }
        });
        true
    }

    /// Change a socket's type; links that no longer fit are removed.
    pub fn set_custom_socket_type(&mut self, node_id: &str, input: bool, socket_id: &str, ty: WgslType) {
        self.update_sockets(node_id, input, |list| {
            for s in list.iter_mut().filter(|s| s.id == socket_id) {
                s.ty = ty;
            }
        });
    }

    /// Move a socket up (-1) or down (+1) among its side's.
    pub fn move_custom_socket(&mut self, node_id: &str, input: bool, socket_id: &str, offset: isize) {
        self.update_sockets(node_id, input, |list| {
            if let Some(index) = list.iter().position(|s| s.id == socket_id)
                && let Some(target) = index.checked_add_signed(offset).filter(|t| *t < list.len()) {
                    list.swap(index, target);
                }
        });
    }
}
