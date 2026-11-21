//! Editing zones and Index Switches.

use std::collections::HashSet;

use super::model::GraphModel;
use super::{Connection, GraphEdge, GraphNode, Point, new_id};
use crate::nodes::{INDEX_SWITCH, clamp_cases};
use crate::passes::{
    ItemSizeMode, TextureFormat, default_size_values, item_size_handles, item_size_input_ids,
};
use crate::types::{HandleType, NodeData, WgslType, ZoneItem};
use crate::zones::{
    DEFAULT_ITERATIONS, DEFAULT_MAX_ITERATIONS, DELTA_TIME_HANDLE, ITERATIONS_HANDLE,
    ITERATION_HANDLE, MAX_ITERATIONS, NEW_ITEM_HANDLE, REPEAT_INPUT, SKIP_HANDLE, ZoneKind,
    carries_item, inferred_item_family, is_zone_input, paired_output_id, zone_items, zone_kind_of,
};

/// Distance between a new zone's input and output nodes.
const ZONE_SPACING: f32 = 500.0;

pub struct ZoneNodes {
    pub input: GraphNode,
    pub output: GraphNode,
    pub kind: ZoneKind,
}

impl GraphModel {
    /// Add a new zone (paired input/output nodes) to the active document,
    /// with one inferred state item passed straight through. Returns its
    /// input's id, or `None` if the document is locked.
    pub fn create_zone(&mut self, kind: ZoneKind, position: Point) -> Option<String> {
        if self.is_locked() {
            return None;
        }
        let (input_id, output_id) = (new_id(), new_id());
        let item = ZoneItem::new(new_id(), "Value", HandleType::inferred());
        let (input_type, output_type) = kind.boundaries();
        let repeat = kind == ZoneKind::Repeat;
        let mut input_data = NodeData { paired_output: Some(output_id.clone()), ..Default::default() };
        if repeat {
            input_data.values.insert(ITERATIONS_HANDLE.to_owned(), DEFAULT_ITERATIONS.into());
            input_data.max_iterations = Some(DEFAULT_MAX_ITERATIONS);
        }
        input_data.values.insert(item.id.clone(), 0.0.into());
        let mut output_data = NodeData::default();
        if !repeat {
            output_data.values.insert(SKIP_HANDLE.to_owned(), false.into());
        }
        output_data.items = Some(vec![item.clone()]);
        let mut input = GraphNode::new(input_type, position);
        input.id = input_id.clone();
        input.data = input_data;
        let mut output = GraphNode::new(output_type, Point::new(position.x + ZONE_SPACING, position.y));
        output.id = output_id.clone();
        output.data = output_data;
        let passthrough = GraphEdge {
            id: new_id(),
            source: input_id.clone(),
            source_handle: item.id.clone(),
            target: output_id,
            target_handle: item.id,
        };
        let key = self.active_definition_ref();
        let doc = self.project.definitions.get_mut(&key)?;
        doc.nodes.extend([input, output]);
        doc.edges.push(passthrough);
        self.changed();
        Some(input_id)
    }

    /// The paired nodes of a zone of the active document, by its input's id.
    pub fn zone_nodes(&self, zone_id: &str) -> Option<ZoneNodes> {
        let doc = self.active_document();
        let input = doc.node(zone_id)?;
        let output = doc.node(paired_output_id(input)?)?;
        let kind = zone_kind_of(&input.node_type)?;
        (is_zone_input(&input.node_type) && output.node_type == kind.boundaries().1)
            .then(|| ZoneNodes { input: input.clone(), output: output.clone(), kind })
    }

    pub fn zone_items_of(&self, zone_id: &str) -> Vec<ZoneItem> {
        zone_items(self.zone_nodes(zone_id).as_ref().map(|z| &z.output))
    }

    /// Write a zone's item list (on its output node), dropping links to
    /// items that no longer exist or no longer fit their type.
    fn set_zone_items(&mut self, zone_id: &str, items: Vec<ZoneItem>) {
        let Some(zone) = self.zone_nodes(zone_id) else { return };
        if self.is_locked() || !items.iter().all(|i| carries_item(Some(zone.kind), i)) {
            return;
        }
        let key = self.active_definition_ref();
        let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
        let sizes = item_size_input_ids(&items);
        let boundary = [zone.input.id.clone(), zone.output.id.clone()];
        let fixed = [ITERATIONS_HANDLE, ITERATION_HANDLE, DELTA_TIME_HANDLE, SKIP_HANDLE];
        {
            let doc = self.project.definitions.get_mut(&key).expect("active");
            if let Some(node) = doc.node_mut(&zone.output.id) {
                node.data.items = Some(items.clone());
            }
            doc.edges.retain(|e| {
                let bad_target = boundary.contains(&e.target)
                    && !fixed.contains(&e.target_handle.as_str())
                    && !ids.contains(e.target_handle.as_str())
                    && !(e.target == zone.input.id && sizes.contains(&e.target_handle));
                let bad_source = boundary.contains(&e.source)
                    && !fixed.contains(&e.source_handle.as_str())
                    && !ids.contains(e.source_handle.as_str());
                !(bad_target || bad_source)
            });
        }
        self.project.reindex();
        self.resolver.clear();
        self.drop_unfit_links_among(&key, &boundary);
        self.changed();
    }

    /// Drop the links of `key` touching `nodes` whose types no longer fit.
    pub(crate) fn drop_unfit_links_among(&mut self, key: &str, nodes: &[String]) {
        let Some(doc) = self.project.document(key) else { return };
        let unfit: HashSet<String> = doc
            .edges
            .iter()
            .filter(|e| nodes.contains(&e.source) || nodes.contains(&e.target))
            .filter(|e| {
                let (Some(source), Some(target)) = (doc.node(&e.source), doc.node(&e.target)) else {
                    return true;
                };
                !self.link_fits(source, &e.source_handle, target, &e.target_handle)
            })
            .map(|e| e.id.clone())
            .collect();
        if unfit.is_empty() {
            return;
        }
        if let Some(doc) = self.project.definitions.get_mut(key) {
            doc.edges.retain(|e| !unfit.contains(&e.id));
        }
        self.project.reindex();
        self.resolver.clear();
    }

    /// Append a state item to a zone. Returns the new item.
    pub fn add_zone_item(
        &mut self,
        zone_id: &str,
        ty: HandleType,
        label: Option<&str>,
    ) -> Option<ZoneItem> {
        let zone = self.zone_nodes(zone_id)?;
        if self.is_locked() {
            return None;
        }
        let items = zone_items(Some(&zone.output));
        let base = label.unwrap_or("Value");
        let mut name = base.to_owned();
        let mut n = 2;
        while items.iter().any(|i| i.label == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        let item = ZoneItem::new(new_id(), name, ty);
        let mut all = items;
        all.push(item.clone());
        self.set_zone_items(zone_id, all);
        Some(item)
    }

    pub fn remove_zone_item(&mut self, zone_id: &str, item_id: &str) {
        let items: Vec<ZoneItem> =
            self.zone_items_of(zone_id).into_iter().filter(|i| i.id != item_id).collect();
        self.pick_item_type(zone_id, item_id, None);
        self.set_zone_items(zone_id, items);
    }

    /// Rename a state item; names stay unique. Returns whether it was applied.
    pub fn rename_zone_item(&mut self, zone_id: &str, item_id: &str, label: &str) -> bool {
        let label = label.trim();
        let items = self.zone_items_of(zone_id);
        if label.is_empty() || items.iter().any(|i| i.id != item_id && i.label == label) {
            return false;
        }
        let items = items
            .into_iter()
            .map(|mut i| {
                if i.id == item_id {
                    i.label = label.to_owned();
                }
                i
            })
            .collect();
        self.set_zone_items(zone_id, items);
        true
    }

    /// Change a state item's type; links that no longer fit are removed. An
    /// item becoming inferred keeps its type until its initial value is linked.
    pub fn set_zone_item_type(&mut self, zone_id: &str, item_id: &str, ty: HandleType) {
        let items = self.zone_items_of(zone_id);
        let current = self.resolve_output_handle_type(zone_id, item_id);
        self.pick_item_type(zone_id, item_id, ty.is_inferred().then_some(current));
        let items = items
            .into_iter()
            .map(|mut i| {
                if i.id == item_id {
                    i.ty = ty.clone();
                }
                i
            })
            .collect();
        self.set_zone_items(zone_id, items);
    }

    /// Set the format a simulation's item is kept in, or (`None`) the one its
    /// type has by default. A repeat zone's items aren't kept in textures.
    pub fn set_zone_item_format(&mut self, zone_id: &str, item_id: &str, format: Option<TextureFormat>) {
        if self.zone_nodes(zone_id).is_none_or(|z| z.kind != ZoneKind::Simulation) {
            return;
        }
        let items = self
            .zone_items_of(zone_id)
            .into_iter()
            .map(|mut i| {
                if i.id == item_id {
                    i.format = format.map(|f| f.id().to_owned());
                }
                i
            })
            .collect();
        self.set_zone_items(zone_id, items);
    }

    /// Set how a simulation's item is sized, or (`None`) to the canvas' size.
    /// Its size inputs on the zone's input node follow the mode, and links to
    /// those it no longer has are dropped.
    pub fn set_zone_item_size(&mut self, zone_id: &str, item_id: &str, size: Option<ItemSizeMode>) {
        let Some(zone) = self.zone_nodes(zone_id) else { return };
        if zone.kind != ZoneKind::Simulation || self.is_locked() {
            return;
        }
        if let Some(mode) = size {
            let handles = item_size_handles(item_id);
            let key = if mode == ItemSizeMode::Scale { &handles.scale } else { &handles.pixels };
            if !zone.input.data.values.contains_key(key) {
                let defaults = default_size_values(&handles, mode);
                self.update_node_data(&zone.input.id, |data| data.values.extend(defaults));
            }
        }
        let items = self
            .zone_items_of(zone_id)
            .into_iter()
            .map(|mut i| {
                if i.id == item_id {
                    i.size = size.map(|m| m.id().to_owned());
                }
                i
            })
            .collect();
        self.set_zone_items(zone_id, items);
    }

    pub fn move_zone_item(&mut self, zone_id: &str, item_id: &str, offset: isize) {
        let mut items = self.zone_items_of(zone_id);
        let Some(index) = items.iter().position(|i| i.id == item_id) else { return };
        let Some(target) = index.checked_add_signed(offset).filter(|t| *t < items.len()) else {
            return;
        };
        items.swap(index, target);
        self.set_zone_items(zone_id, items);
    }

    /// Pick the type an inferred item takes while its initial value is
    /// unlinked (or forget it, with `None`).
    fn pick_item_type(&mut self, zone_id: &str, item_id: &str, ty: Option<WgslType>) {
        let Some(zone) = self.zone_nodes(zone_id) else { return };
        if self.is_locked() {
            return;
        }
        let key = inferred_item_family(item_id);
        self.update_node_data(&zone.input.id, |data| match ty {
            Some(t) => {
                data.types.insert(key, t);
            }
            None => {
                data.types.remove(&key);
            }
        });
    }

    /// Set a repeat zone's iteration safety cap.
    pub fn set_zone_max_iterations(&mut self, zone_id: &str, max: u32) {
        let Some(zone) = self.zone_nodes(zone_id) else { return };
        if zone.input.node_type != REPEAT_INPUT || self.is_locked() {
            return;
        }
        self.update_node_data(&zone.input.id, |data| data.max_iterations = Some(max.min(MAX_ITERATIONS)));
    }

    /// Complete a connection dropped on a zone's virtual "new item" handle:
    /// create an inferred item, then link to it. Unless the link is its
    /// initial value, the item takes the type at the other end until one is
    /// linked.
    pub(crate) fn connect_new_zone_item(&mut self, c: &Connection) -> bool {
        let to_new = c.target_handle == NEW_ITEM_HANDLE;
        let zone_node = if to_new { &c.target } else { &c.source };
        let Some(zone_id) =
            self.active_zones().zone_of_boundary.get(zone_node).cloned()
        else {
            return false;
        };
        let (ty, label) = if to_new {
            (
                self.resolve_output_handle_type(&c.source, &c.source_handle),
                self.resolve_node_outputs(&c.source).into_iter().find(|o| o.id == c.source_handle).map(|o| o.label),
            )
        } else {
            (
                self.resolve_input_type(&c.target, &c.target_handle),
                self.resolve_node_inputs(&c.target).into_iter().find(|i| i.id == c.target_handle).map(|i| i.label),
            )
        };
        let Some(item) = self.add_zone_item(&zone_id, HandleType::inferred(), label.as_deref()) else {
            return false;
        };
        if !(to_new && c.target == zone_id) {
            self.pick_item_type(&zone_id, &item.id, Some(ty));
        }
        let edge = GraphEdge {
            id: new_id(),
            source: c.source.clone(),
            source_handle: if to_new { c.source_handle.clone() } else { item.id.clone() },
            target: c.target.clone(),
            target_handle: if to_new { item.id.clone() } else { c.target_handle.clone() },
        };
        let key = self.active_definition_ref();
        if let Some(doc) = self.project.definitions.get_mut(&key) {
            if !to_new {
                doc.edges.retain(|e| !(e.target == edge.target && e.target_handle == edge.target_handle));
            }
            doc.edges.push(edge);
        }
        self.changed();
        true
    }

    // ── Index Switch ─────────────────────────────────────────────────────────

    /// Set how many cases an Index Switch has; links to cases that go are dropped.
    pub fn set_index_switch_cases(&mut self, node_id: &str, count: i64) {
        let Some(node) = self.active_document().node(node_id).cloned() else { return };
        if !node.is(INDEX_SWITCH) || self.is_locked() {
            return;
        }
        let count = clamp_cases(count);
        let cases: HashSet<String> = (0..count).map(|i| crate::enums::case_handle(i64::from(i))).collect();
        let removed = |handle: &str| crate::enums::is_case_handle(handle) && !cases.contains(handle);
        self.update_node_data(node_id, |data| {
            data.cases = Some(count);
            data.values.retain(|handle, _| !removed(handle));
        });
        let key = self.active_definition_ref();
        if let Some(doc) = self.project.definitions.get_mut(&key) {
            doc.edges.retain(|e| {
                e.target != node_id
                    || !crate::enums::is_case_handle(&e.target_handle)
                    || cases.contains(&e.target_handle)
            });
        }
        self.changed();
    }
}
