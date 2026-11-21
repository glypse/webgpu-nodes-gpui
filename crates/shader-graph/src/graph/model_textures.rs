//! Editing Render Texture nodes: their items, and how each is kept.

use std::collections::HashSet;

use super::model::GraphModel;
use super::new_id;
use crate::codegen::CodegenGraph;
use crate::nodes::{
    DEFAULT_RENDER_FORMAT, RENDER_LIKE, RENDER_PIXELS, RENDER_SCALE, RENDER_TEXTURE,
    render_texture_items,
};
use crate::passes::{PassAnalysis, TextureFormat};
use crate::types::{HandleType, WgslType, ZoneItem};

impl GraphModel {
    /// What the project's links do with textures, as it's compiled now: the
    /// same sources codegen reads.
    pub fn pass_analysis(&self) -> PassAnalysis<'_> {
        PassAnalysis::new(CodegenGraph { project: &self.project, resolver: &self.resolver })
    }

    fn render_node_in_active(&self, node_id: &str) -> Option<&super::GraphNode> {
        let node = self.active_document().node(node_id)?;
        (node.is(RENDER_TEXTURE) && !self.is_locked()).then_some(node)
    }

    /// The items of a Render Texture.
    pub fn render_items_of(&self, node_id: &str) -> Vec<ZoneItem> {
        self.active_document()
            .node(node_id)
            .filter(|n| n.is(RENDER_TEXTURE))
            .map(|n| render_texture_items(&n.data))
            .unwrap_or_default()
    }

    /// Writes a Render Texture's items, dropping links to those gone or no longer fitting.
    fn set_render_items(&mut self, node_id: &str, items: Vec<ZoneItem>) {
        if self.render_node_in_active(node_id).is_none() {
            return;
        }
        let key = self.active_definition_ref();
        let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
        let fixed = [RENDER_SCALE, RENDER_PIXELS, RENDER_LIKE];
        if let Some(doc) = self.project.definitions.get_mut(&key) {
            if let Some(node) = doc.node_mut(node_id) {
                node.data.items = Some(items.clone());
            }
            doc.edges.retain(|e| {
                let bad_target = e.target == node_id
                    && !fixed.contains(&e.target_handle.as_str())
                    && !ids.contains(e.target_handle.as_str());
                let bad_source = e.source == node_id && !ids.contains(e.source_handle.as_str());
                !(bad_target || bad_source)
            });
        }
        self.project.reindex();
        self.resolver.clear();
        self.drop_unfit_links_among(&key, &[node_id.to_owned()]);
        self.changed();
    }

    /// Appends an item to a Render Texture, named freely; returns it.
    pub fn add_render_item(
        &mut self,
        node_id: &str,
        ty: WgslType,
        format: Option<TextureFormat>,
    ) -> Option<ZoneItem> {
        self.render_node_in_active(node_id)?;
        let items = self.render_items_of(node_id);
        let mut label = "Color".to_owned();
        let mut n = 2;
        while items.iter().any(|i| i.label == label) {
            label = format!("Color {n}");
            n += 1;
        }
        let mut item = ZoneItem::new(new_id(), label, HandleType::Fixed(ty));
        item.format = Some(format.unwrap_or(DEFAULT_RENDER_FORMAT).id().to_owned());
        let mut all = items;
        all.push(item.clone());
        self.set_render_items(node_id, all);
        Some(item)
    }

    pub fn remove_render_item(&mut self, node_id: &str, item_id: &str) {
        let items = self.render_items_of(node_id).into_iter().filter(|i| i.id != item_id).collect();
        self.set_render_items(node_id, items);
    }

    /// Renames an item; rejects empty or duplicate names, returning whether it was applied.
    pub fn rename_render_item(&mut self, node_id: &str, item_id: &str, label: &str) -> bool {
        let name = label.trim();
        let items = self.render_items_of(node_id);
        if name.is_empty() || items.iter().any(|i| i.id != item_id && i.label == name) {
            return false;
        }
        self.edit_render_item(node_id, item_id, |i| i.label = name.to_owned());
        true
    }

    /// Changes the format an item is kept in.
    pub fn set_render_item_format(&mut self, node_id: &str, item_id: &str, format: TextureFormat) {
        self.edit_render_item(node_id, item_id, |i| i.format = Some(format.id().to_owned()));
    }

    /// Changes the type of the values an item draws; links that no longer fit are removed.
    pub fn set_render_item_type(&mut self, node_id: &str, item_id: &str, ty: WgslType) {
        self.edit_render_item(node_id, item_id, |i| i.ty = HandleType::Fixed(ty));
    }

    /// Moves an item up (-1) or down (+1) among the node's.
    pub fn move_render_item(&mut self, node_id: &str, item_id: &str, offset: isize) {
        let mut items = self.render_items_of(node_id);
        let Some(index) = items.iter().position(|i| i.id == item_id) else { return };
        let Some(other) = index.checked_add_signed(offset).filter(|o| *o < items.len()) else {
            return;
        };
        items.swap(index, other);
        self.set_render_items(node_id, items);
    }

    fn edit_render_item(&mut self, node_id: &str, item_id: &str, edit: impl FnOnce(&mut ZoneItem)) {
        let mut items = self.render_items_of(node_id);
        let Some(item) = items.iter_mut().find(|i| i.id == item_id) else { return };
        edit(item);
        self.set_render_items(node_id, items);
    }
}
