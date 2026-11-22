//! `GraphModel`: a project plus the editing state around it (the group being
//! viewed, the selection, undo history) and every rule edits keep.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::history::History;
use super::{
    Connection, FRAG_OUTPUT, GROUP_INPUT, GROUP_OUTPUT, GraphDocument, GraphEdge, GraphNode,
    GraphProject, GroupWidget, LAYOUT_FRAME, NODE_GROUP, Point, REROUTE, ROOT_ID, estimate_frame_header,
    frame_note, new_id,
};
use crate::color::color_mode_conversion;
use crate::layout::{
    InputRow, LayoutError, LayoutInput, LayoutOptions, NodeGeometry, NodeRows, RerouteBranches,
    compute_layout, estimate_node_geometry, reroute_branches,
};
use crate::codegen::{CodegenError, Program, ShaderOptions, generate_program};
use crate::enums::{EnumDocument, MATCH, MATCH_SELECTOR, group_constants, hidden_inputs_of};
use crate::library::is_library_ref;
use crate::nodes::{COLOR_RAMP, color_ramp_hidden_inputs, default_data_for_type, registered, resolve_descriptor};
use crate::types::{
    InputValue, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef, NodeWidget, ResolvedNodeTypes,
    TypeBindings, TypeResolver, WgslType, input_family_key, is_connection_compatible,
};
use crate::zones::{
    NEW_ITEM_HANDLE, ZoneAnalysis, analyze_zones, inferred_item_family, is_zone_input,
    is_zone_node_type, zone_options, would_create_cycle,
};

/// What the editor offers for a node's type picker.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeChoice {
    pub types: Vec<WgslType>,
    /// Whether links can infer the type (then `inferred` says which).
    pub inferable: bool,
    pub forced: Option<WgslType>,
    pub inferred: WgslType,
}

/// Nodes and the links among them, copied to paste elsewhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeClipboard {
    /// The definition the nodes were copied from.
    pub from: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Clone)]
pub struct GraphModel {
    pub(crate) project: GraphProject,
    pub(crate) resolver: Arc<TypeResolver>,
    pub(crate) path: Vec<String>,
    pub(crate) selected: Vec<String>,
    history: History,
    /// Nesting depth of `batched` calls; history steps wait for the outermost.
    batch_depth: u32,
    saved: String,
}

impl Default for GraphModel {
    fn default() -> Self {
        Self::new(GraphProject::default())
    }
}

impl GraphModel {
    pub fn new(mut project: GraphProject) -> Self {
        crate::library::register(&mut project);
        project.reindex();
        let snapshot = Self::snapshot_of(&project);
        let mut history = History::default();
        history.reset(snapshot.clone());
        Self {
            project,
            resolver: Arc::new(TypeResolver::new()),
            path: Vec::new(),
            selected: Vec::new(),
            history,
            batch_depth: 0,
            saved: snapshot,
        }
    }

    // ── State ────────────────────────────────────────────────────────────────

    pub fn project(&self) -> &GraphProject {
        &self.project
    }

    pub fn root(&self) -> &GraphDocument {
        self.project.root()
    }

    pub fn resolver(&self) -> &TypeResolver {
        &self.resolver
    }

    /// Replace the whole project, forgetting history and selection.
    pub fn set_project(&mut self, mut project: GraphProject) {
        crate::library::register(&mut project);
        project.reindex();
        self.project = project;
        self.path.clear();
        self.selected.clear();
        self.resolver.clear();
        let snapshot = Self::snapshot_of(&self.project);
        self.history.reset(snapshot.clone());
        self.saved = snapshot;
    }

    pub fn selected_node_ids(&self) -> &[String] {
        &self.selected
    }

    pub fn set_selected_node_ids(&mut self, ids: Vec<String>) {
        self.selected = ids;
    }

    fn snapshot_of(project: &GraphProject) -> String {
        serde_json::to_string(project).expect("a project serialises")
    }

    /// Edit the project through `f`: indexes are rebuilt, types forgotten and
    /// the result recorded as an undo step.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut GraphProject) -> R) -> R {
        let result = f(&mut self.project);
        self.changed();
        result
    }

    /// Call after changing the project: rebuilds what is derived from it and
    /// records an undo step when it differs from the last one.
    pub(crate) fn changed(&mut self) {
        crate::library::register(&mut self.project);
        self.project.reindex();
        self.drop_stale_case_links();
        self.resolver.clear();
        let valid = self.valid_path(&self.path);
        self.path = valid;
        if self.batch_depth == 0 {
            let snapshot = Self::snapshot_of(&self.project);
            self.history.record(snapshot);
        }
    }

    /// Runs `f`, whose edits make one undo step together.
    pub fn batched<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.batch_depth += 1;
        let result = f(self);
        self.batch_depth -= 1;
        if self.batch_depth == 0 {
            self.changed();
        }
        result
    }

    /// Like `edit`, but a gesture's steps (`first` false) replace the step
    /// the gesture began with instead of adding their own.
    pub fn edit_gesture(&mut self, first: bool, f: impl FnOnce(&mut GraphProject)) {
        f(&mut self.project);
        if first {
            self.changed();
            return;
        }
        self.project.reindex();
        self.resolver.clear();
        let snapshot = Self::snapshot_of(&self.project);
        self.history.replace_current(snapshot);
    }

    // ── History ──────────────────────────────────────────────────────────────

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo() {
            Some(snapshot) => self.restore(&snapshot),
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo() {
            Some(snapshot) => self.restore(&snapshot),
            None => false,
        }
    }

    fn restore(&mut self, snapshot: &str) -> bool {
        let Ok(mut project) = serde_json::from_str::<GraphProject>(snapshot) else { return false };
        crate::library::register(&mut project);
        project.reindex();
        self.project = project;
        self.resolver.clear();
        self.path = self.valid_path(&self.path);
        let alive: HashSet<&str> =
            self.project.definitions.values().flat_map(|d| d.nodes.iter().map(|n| n.id.as_str())).collect();
        self.selected.retain(|id| alive.contains(id.as_str()));
        true
    }

    /// Whether the project differs from how it was last marked saved.
    pub fn has_unsaved_changes(&self) -> bool {
        // The history holds the project's snapshot as of its last change, so no new one is needed
        // except in the middle of a batch, whose edits aren't recorded yet.
        match self.history.current() {
            Some(current) if self.batch_depth == 0 => current != self.saved,
            _ => Self::snapshot_of(&self.project) != self.saved,
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved = Self::snapshot_of(&self.project);
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    /// The breadcrumb of group node ids from the root to the viewed document.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// The longest start of `path` that still walks through group nodes.
    pub fn valid_path(&self, path: &[String]) -> Vec<String> {
        let mut doc = self.project.root();
        let mut valid = Vec::new();
        for id in path {
            let Some(next) = doc
                .node(id)
                .and_then(GraphNode::definition_ref)
                .and_then(|r| self.project.document(r))
            else {
                break;
            };
            valid.push(id.clone());
            doc = next;
        }
        valid
    }

    pub fn set_path(&mut self, path: Vec<String>) {
        self.path = self.valid_path(&path);
    }

    /// Enter a group node of the viewed document.
    pub fn enter_group(&mut self, group_node_id: &str) {
        let mut path = self.path.clone();
        path.push(group_node_id.to_owned());
        self.path = self.valid_path(&path);
    }

    pub fn exit_group(&mut self) {
        self.path.pop();
    }

    /// Jump to a depth in the breadcrumb.
    pub fn jump_to_path(&mut self, path: Vec<String>) {
        self.set_path(path);
    }

    /// The key in `definitions` of the document `path` leads to; the root when it leads nowhere.
    pub fn definition_ref_at(&self, path: &[String]) -> String {
        let mut key = ROOT_ID.to_owned();
        for id in path {
            let next = self
                .project
                .document(&key)
                .and_then(|d| d.node(id))
                .and_then(GraphNode::definition_ref);
            match next {
                Some(r) => key = r.to_owned(),
                None => break,
            }
        }
        key
    }

    pub fn document_at(&self, path: &[String]) -> &GraphDocument {
        self.project.document(&self.definition_ref_at(path)).unwrap_or_else(|| self.project.root())
    }

    pub fn active_document(&self) -> &GraphDocument {
        self.document_at(&self.path)
    }

    pub fn active_definition_ref(&self) -> String {
        self.definition_ref_at(&self.path)
    }

    /// Whether the subgraph `path` leads to (or any of its ancestors) is locked.
    pub fn is_locked_at(&self, path: &[String]) -> bool {
        let mut doc = self.project.root();
        for id in path {
            let Some(node) = doc.node(id) else { return false };
            if node.group.as_ref().is_some_and(|g| g.locked) {
                return true;
            }
            match node.definition_ref().and_then(|r| self.project.document(r)) {
                Some(next) if next.locked => return true,
                Some(next) => doc = next,
                None => return false,
            }
        }
        false
    }

    pub fn is_locked(&self) -> bool {
        self.is_locked_at(&self.path)
    }

    /// The label of each segment of a breadcrumb path.
    pub fn path_labels(&self, path: &[String]) -> Vec<(String, String, bool)> {
        let mut doc = self.project.root();
        let mut labels = Vec::new();
        for id in path {
            let Some(node) = doc.node(id) else { break };
            labels.push((
                id.clone(),
                self.group_label(node),
                node.group.as_ref().is_some_and(|g| g.locked),
            ));
            match node.definition_ref().and_then(|r| self.project.document(r)) {
                Some(next) => doc = next,
                None => break,
            }
        }
        labels
    }

    fn group_label(&self, node: &GraphNode) -> String {
        node.data.label.clone().unwrap_or_else(|| resolve_descriptor(node, &self.project).label.clone())
    }

    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.project.resolve_node(id)
    }

    pub fn resolve_descriptor(&self, node: &GraphNode) -> Arc<NodeDescriptor> {
        resolve_descriptor(node, &self.project)
    }

    // ── Types ────────────────────────────────────────────────────────────────

    /// The variables' types a document is viewed with from `path`: those of
    /// the group node each level was entered through.
    pub fn view_bindings(&self, doc_ref: &str, path: &[String]) -> TypeBindings {
        let mut key = ROOT_ID.to_owned();
        let mut bindings = TypeBindings::new();
        if key == doc_ref {
            return bindings;
        }
        for id in path {
            let Some(node) = self.project.document(&key).and_then(|d| d.node(id)) else { break };
            let Some(next) = node.definition_ref() else { break };
            let resolved = self.resolver.resolve(&self.project, node, &bindings);
            bindings = resolved.bindings.clone().unwrap_or_default();
            key = next.to_owned();
            if key == doc_ref {
                return bindings;
            }
        }
        TypeBindings::new()
    }

    /// A node's resolved types as seen from `path` (the active one by default).
    pub fn types_of(&self, node_id: &str, path: Option<&[String]>) -> Option<Arc<ResolvedNodeTypes>> {
        let node = self.node(node_id)?;
        let doc_ref = self.project.definition_ref_of(node_id)?;
        let bindings = self.view_bindings(doc_ref, path.unwrap_or(&self.path));
        Some(self.resolver.resolve(&self.project, node, &bindings))
    }

    pub fn resolve_input_type(&self, node_id: &str, input: &str) -> WgslType {
        self.types_of(node_id, None).and_then(|t| t.input_type(input)).unwrap_or(WgslType::F32)
    }

    pub fn resolve_node_output_type(&self, node_id: &str) -> WgslType {
        self.types_of(node_id, None).map_or(WgslType::F32, |t| t.output_type)
    }

    pub fn resolve_output_handle_type(&self, node_id: &str, handle: &str) -> WgslType {
        self.types_of(node_id, None).map_or(WgslType::F32, |t| t.output_handle_type(handle))
    }

    pub fn resolve_node_inputs(&self, node_id: &str) -> Vec<NodeInputDef> {
        self.types_of(node_id, None).map(|t| t.resolved_inputs.clone()).unwrap_or_default()
    }

    pub fn resolve_node_outputs(&self, node_id: &str) -> Vec<NodeOutputDef> {
        self.types_of(node_id, None).map(|t| t.resolved_outputs.clone()).unwrap_or_default()
    }

    /// The type an input would get from its links alone, ignoring the type the
    /// user forced on the node.
    pub fn resolve_inferred_input_type(&self, node_id: &str, input: &str) -> WgslType {
        let Some(node) = self.node(node_id) else { return WgslType::F32 };
        let Some(doc_ref) = self.project.definition_ref_of(node_id) else { return WgslType::F32 };
        let bindings = self.view_bindings(doc_ref, &self.path);
        let mut data = node.data.clone();
        data.types.clear();
        self.resolver
            .resolve_with(&self.project, node, &data, &bindings)
            .input_type(input)
            .unwrap_or(WgslType::F32)
    }

    /// The links of `doc_ref` to sockets their nodes don't have as seen from
    /// the active path: kept, but ignored and not drawn.
    pub fn unavailable_links(&self, doc_ref: &str) -> HashSet<String> {
        let Some(doc) = self.project.document(doc_ref) else { return HashSet::new() };
        crate::types::unavailable_edges(&doc.edges, |id| self.types_of(id, None))
    }

    /// Zone membership / invalid links of a document, derived from its links.
    pub fn zone_analysis(&self, doc: &GraphDocument) -> ZoneAnalysis {
        analyze_zones(&doc.nodes, &doc.edges, &zone_options(&self.project))
    }

    pub fn active_zones(&self) -> ZoneAnalysis {
        self.zone_analysis(self.active_document())
    }

    /// What a node's type picker offers; `None` for nodes without a choice.
    pub fn type_choice(&self, node_id: &str) -> Option<TypeChoice> {
        let node = self.node(node_id)?;
        if node.is(NODE_GROUP) || node.side().is_some() || is_zone_node_type(&node.node_type) {
            return None;
        }
        let descriptor = self.resolve_descriptor(node);
        let pickable = descriptor.pickable_types()?;
        let primary = descriptor
            .primary_input
            .as_deref()
            .and_then(|id| descriptor.input(id))
            .or(descriptor.inputs.first());
        Some(TypeChoice {
            types: pickable.types,
            inferable: pickable.inferable,
            forced: node.data.types.get(&pickable.key).copied(),
            inferred: if pickable.inferable {
                primary.map_or(WgslType::F32, |p| self.resolve_inferred_input_type(node_id, &p.id))
            } else {
                self.resolve_node_output_type(node_id)
            },
        })
    }

    /// Force a node's type, or go back to inference with `None`.
    pub fn set_node_type(&mut self, node_id: &str, ty: Option<WgslType>) {
        if self.type_choice(node_id).is_none() {
            return;
        }
        let Some(node) = self.node(node_id) else { return };
        let Some(pickable) = self.resolve_descriptor(node).pickable_types() else { return };
        self.update_node_data(node_id, |data| match ty {
            Some(t) => {
                data.types.insert(pickable.key.clone(), t);
            }
            None => {
                data.types.remove(&pickable.key);
            }
        });
    }

    /// The inputs of a group node hidden by its enums' values and its types, or a Color Ramp's unused ones.
    pub fn hidden_inputs(&self, node_id: &str) -> HashSet<String> {
        let Some(node) = self.node(node_id) else { return HashSet::new() };
        if node.is(COLOR_RAMP) {
            let linked = self
                .project
                .document_of(node_id)
                .map(|doc| doc.edges_into(node_id).map(|e| e.target_handle.clone()).collect())
                .unwrap_or_default();
            return color_ramp_hidden_inputs(&node.data, &linked);
        }
        let Some(def) = node.definition_ref().and_then(|r| self.project.document(r)) else {
            return HashSet::new();
        };
        let bindings = self
            .types_of(node_id, None)
            .and_then(|t| t.bindings.clone())
            .unwrap_or_default();
        let Some(doc) = self.project.document_of(node_id) else { return HashSet::new() };
        let values = group_constants(&EnumDocument::of(doc), node, &self.project, &bindings, &BTreeMap::new());
        hidden_inputs_of(&def.inputs, &values, &bindings).into_iter().collect()
    }

    // ── Node data ────────────────────────────────────────────────────────────

    /// Change a node's data (in whichever document holds it).
    pub fn update_node_data(&mut self, node_id: &str, f: impl FnOnce(&mut NodeData)) {
        let Some(key) = self.project.definition_ref_of(node_id).map(str::to_owned) else { return };
        let Some(node) = self.project.definitions.get_mut(&key).and_then(|d| d.node_mut(node_id))
        else {
            return;
        };
        f(&mut node.data);
        self.changed();
    }

    /// Set the value an input holds while unlinked (`None` clears it).
    pub fn set_input_value(&mut self, node_id: &str, input: &str, value: Option<InputValue>) {
        let converted = self.converted_colors(node_id, input, value.as_ref());
        self.update_node_data(node_id, |data| {
            match value {
                Some(v) => {
                    data.values.insert(input.to_owned(), v);
                }
                None => {
                    data.values.remove(input);
                }
            }
            for (component, number) in converted {
                data.values.insert(component, number.into());
            }
        });
    }

    /// The components a colour widget's mode change sets so the colour stays
    /// the same.
    fn converted_colors(&self, node_id: &str, input: &str, value: Option<&InputValue>) -> Vec<(String, f64)> {
        let Some(node) = self.node(node_id) else { return Vec::new() };
        let Some(def) = node.definition_ref().and_then(|r| self.project.document(r)) else {
            return Vec::new();
        };
        let Some(widget @ GroupWidget::Color { mode, .. }) = &def.widget else { return Vec::new() };
        if mode != input {
            return Vec::new();
        }
        let Some(variants) = def.inputs.iter().find(|h| h.id == *mode).and_then(|h| h.variants.as_deref()) else {
            return Vec::new();
        };
        let mut after = node.data.values.clone();
        match value {
            Some(v) => after.insert(input.to_owned(), *v),
            None => after.remove(input),
        };
        let linked: HashSet<String> = self
            .project
            .document_of(node_id)
            .map(|doc| doc.edges_into(node_id).map(|e| e.target_handle.clone()).collect())
            .unwrap_or_default();
        color_mode_conversion(widget, variants, &def.defaults, &node.data.values, &after, &linked)
            .unwrap_or_default()
    }

    /// Pick an option of one of the node's settings.
    pub fn set_setting(&mut self, node_id: &str, key: &str, value: &str) {
        self.update_node_data(node_id, |data| {
            data.settings.insert(key.to_owned(), value.to_owned());
        });
    }

    pub fn default_node_label(&self, node_id: &str) -> String {
        self.node(node_id).map_or_else(String::new, |n| resolve_descriptor(n, &self.project).label.clone())
    }

    pub fn node_label(&self, node_id: &str) -> String {
        self.node(node_id).map_or_else(String::new, |n| self.group_label(n))
    }

    pub fn set_node_label(&mut self, node_id: &str, label: Option<String>) {
        let label = label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
        self.update_node_data(node_id, |data| data.label = label);
    }

    /// Put nodes' input values, settings and picked types back to their
    /// defaults. What makes a node what it is (its code, sockets, zone items,
    /// label) is kept.
    pub fn reset_nodes(&mut self, node_ids: &[String]) {
        for id in node_ids {
            let Some(node) = self.node(id) else { continue };
            let defaults = self.resolve_descriptor(node).default_data.clone();
            self.update_node_data(id, |data| {
                data.values = defaults.values.clone();
                data.settings = defaults.settings.clone();
                data.types = defaults.types.clone();
            });
        }
    }

    /// Show only the linked sockets of nodes, or all of them again when every
    /// one of them already shows only those.
    pub fn toggle_unlinked_sockets(&mut self, node_ids: &[String]) {
        let all_hidden = node_ids
            .iter()
            .filter_map(|id| self.node(id))
            .all(|n| n.data.hide_unlinked == Some(true));
        for id in node_ids {
            self.update_node_data(id, |data| data.hide_unlinked = (!all_hidden).then_some(true));
        }
    }

    // ── Adding and removing ──────────────────────────────────────────────────

    pub(crate) fn doc_mut(&mut self, key: &str) -> Option<&mut GraphDocument> {
        self.project.definitions.get_mut(key)
    }

    /// Add a node of a registered type to the active document; `None` when
    /// the document is locked or the type is unknown.
    pub fn add_node(&mut self, node_type: &str, position: Point) -> Option<String> {
        self.add_node_with(node_type, position, None)
    }

    pub fn add_node_with(
        &mut self,
        node_type: &str,
        position: Point,
        data: Option<NodeData>,
    ) -> Option<String> {
        if self.is_locked() || registered(node_type).is_none() {
            return None;
        }
        let mut node = GraphNode::new(node_type, position);
        let defaults = default_data_for_type(node_type);
        node.data = match data {
            Some(d) => d.with_defaults(&defaults),
            None => defaults,
        };
        let id = node.id.clone();
        let key = self.active_definition_ref();
        self.doc_mut(&key)?.nodes.push(node);
        self.changed();
        Some(id)
    }

    /// Move nodes of the active document by `offset` from where `from` has them.
    pub fn move_nodes(&mut self, from: &BTreeMap<String, Point>, offset: Point) {
        let key = self.active_definition_ref();
        let Some(doc) = self.doc_mut(&key) else { return };
        for node in &mut doc.nodes {
            if let Some(start) = from.get(&node.id) {
                node.position = Point::new(start.x + offset.x, start.y + offset.y);
            }
        }
        self.changed();
    }

    pub fn set_node_position(&mut self, node_id: &str, position: Point) {
        let Some(key) = self.project.definition_ref_of(node_id).map(str::to_owned) else { return };
        if let Some(node) = self.doc_mut(&key).and_then(|d| d.node_mut(node_id)) {
            node.position = position;
            self.changed();
        }
    }

    /// Zone nodes always go in pairs: the ids given, plus the partner of every
    /// zone boundary among them.
    fn with_zone_partners(&self, ids: &HashSet<String>) -> HashSet<String> {
        let zones = self.active_zones();
        let mut all = ids.clone();
        for id in ids {
            if let Some(zone) = zones.zone_of(id) {
                all.insert(zone.input_id.clone());
                all.insert(zone.output_id.clone());
            }
        }
        all
    }

    /// Delete nodes (and links) of the active document; zone partners go too.
    pub fn delete_nodes(&mut self, node_ids: &[String], edge_ids: &[String]) -> bool {
        if self.is_locked() {
            return false;
        }
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else { return false };
        let wanted: HashSet<String> = node_ids
            .iter()
            .filter(|id| doc.node(id).is_some_and(GraphNode::deletable))
            .cloned()
            .collect();
        let removed: HashSet<String> = self.with_zone_partners(&wanted);
        let edge_set: HashSet<&String> = edge_ids.iter().collect();
        let edges: Vec<GraphEdge> = doc
            .edges
            .iter()
            .filter(|e| {
                !edge_set.contains(&e.id) && !removed.contains(&e.source) && !removed.contains(&e.target)
            })
            .cloned()
            .collect();
        if removed.is_empty() && edges.len() == doc.edges.len() {
            return false;
        }
        let frame_of = self.active_frames().frame_of;
        let doc = self.doc_mut(&key).expect("checked");
        doc.nodes.retain(|n| !removed.contains(&n.id));
        doc.edges = edges;
        for node in &mut doc.nodes {
            // What a deleted frame held stays, in the frame around it.
            let mut frame = node.frame.clone();
            while let Some(f) = &frame {
                if !removed.contains(f) {
                    break;
                }
                frame = frame_of.get(f).cloned();
            }
            node.frame = frame;
        }
        self.selected.retain(|id| !removed.contains(id));
        self.changed();
        true
    }

    pub fn delete_edges(&mut self, edge_ids: &[String]) -> bool {
        self.delete_nodes(&[], edge_ids)
    }

    /// Delete nodes, linking what fed them to what they fed: each link out of
    /// the deleted nodes is traced back through them, along the input of the
    /// same type as the output it leaves (or the first linked input), to a
    /// node that stays. Links that wouldn't be valid are dropped.
    pub fn dissolve_nodes(&mut self, node_ids: &[String]) -> bool {
        if self.is_locked() {
            return false;
        }
        let doc = self.active_document();
        let wanted: HashSet<String> = node_ids.iter().cloned().collect();
        let mut removed = self.with_zone_partners(&wanted);
        removed.retain(|id| doc.node(id).is_some_and(GraphNode::deletable));
        let links: Vec<Connection> = doc
            .edges
            .iter()
            .filter(|e| removed.contains(&e.source) && !removed.contains(&e.target))
            .filter_map(|edge| {
                let upstream = self.upstream_through(&removed, &edge.source, &edge.source_handle)?;
                Some(Connection::new(
                    upstream.source,
                    upstream.source_handle,
                    &edge.target,
                    &edge.target_handle,
                ))
            })
            .collect();
        let removed: Vec<String> = removed.into_iter().collect();
        if !self.delete_nodes(&removed, &[]) {
            return false;
        }
        for link in links {
            if self.is_valid_connection(&link) {
                self.connect(link);
            }
        }
        true
    }

    fn upstream_through(
        &self,
        removed: &HashSet<String>,
        node_id: &str,
        handle: &str,
    ) -> Option<GraphEdge> {
        let doc = self.active_document();
        let mut seen = HashSet::new();
        let (mut node, mut handle) = (node_id.to_owned(), handle.to_owned());
        while seen.insert(node.clone()) {
            let ty = self.resolve_output_handle_type(&node, &handle);
            let hidden = self.hidden_inputs(&node);
            let incoming: Vec<&GraphEdge> = self
                .resolve_node_inputs(&node)
                .iter()
                .filter(|i| !hidden.contains(&i.id))
                .flat_map(|input| doc.edges_into(&node).filter(move |e| e.target_handle == input.id))
                .collect();
            let fed = incoming
                .iter()
                .find(|e| self.resolve_input_type(&node, &e.target_handle) == ty)
                .or(incoming.first())?;
            if !removed.contains(&fed.source) {
                return Some((*fed).clone());
            }
            (node, handle) = (fed.source.clone(), fed.source_handle.clone());
        }
        None
    }

    // ── Clipboard ────────────────────────────────────────────────────────────

    /// Copy nodes (zone partners with them) and the links among them.
    pub fn copy_nodes(&self, node_ids: &[String]) -> Option<NodeClipboard> {
        let doc = self.active_document();
        let ids = self.with_zone_partners(&self.with_frame_contents(node_ids));
        let nodes: Vec<GraphNode> =
            doc.nodes.iter().filter(|n| ids.contains(&n.id) && n.deletable()).cloned().collect();
        if nodes.is_empty() {
            return None;
        }
        let kept: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let edges = doc
            .edges
            .iter()
            .filter(|e| kept.contains(e.source.as_str()) && kept.contains(e.target.as_str()))
            .cloned()
            .collect();
        Some(NodeClipboard { from: self.active_definition_ref(), nodes, edges })
    }

    /// Paste a clipboard into the active document, its top-left at `position`
    /// (where it was copied from by default). Group nodes of a group that's
    /// gone, or that would contain itself, are skipped, and so are boundary
    /// nodes of another group: they show their own group's inputs and outputs.
    /// Returns the new nodes' ids.
    pub fn paste_nodes(&mut self, clipboard: &NodeClipboard, position: Option<Point>) -> Vec<String> {
        if self.is_locked() {
            return Vec::new();
        }
        let home = clipboard.from == self.active_definition_ref();
        let pasted: Vec<GraphNode> = clipboard
            .nodes
            .iter()
            .filter(|n| (home || n.side().is_none()) && n.definition_ref().is_none_or(|r| {
                self.project.document(r).is_some() && !self.would_create_group_cycle(r)
            }))
            .cloned()
            .collect();
        if pasted.is_empty() {
            return Vec::new();
        }
        let clipboard = &NodeClipboard { from: clipboard.from.clone(), nodes: pasted, edges: clipboard.edges.clone() };
        let min_x = clipboard.nodes.iter().map(|n| n.position.x).fold(f32::INFINITY, f32::min);
        let min_y = clipboard.nodes.iter().map(|n| n.position.y).fold(f32::INFINITY, f32::min);
        let offset = position.map_or(Point::new(40.0, 40.0), |p| Point::new(p.x - min_x, p.y - min_y));
        let (nodes, edges, _) = clone_subgraph(&clipboard.nodes, &clipboard.edges, offset);
        let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let key = self.active_definition_ref();
        let Some(doc) = self.doc_mut(&key) else { return Vec::new() };
        doc.nodes.extend(nodes);
        doc.edges.extend(edges);
        self.selected = ids.clone();
        self.changed();
        ids
    }

    pub fn duplicate_nodes(&mut self, node_ids: &[String]) -> Vec<String> {
        match self.copy_nodes(node_ids) {
            Some(clipboard) => self.paste_nodes(&clipboard, None),
            None => Vec::new(),
        }
    }

    // ── Connections ──────────────────────────────────────────────────────────

    pub(crate) fn link_fits(&self, source: &GraphNode, source_handle: &str, target: &GraphNode, target_handle: &str) -> bool {
        let source_type = self.resolve_output_handle_type(&source.id, source_handle);
        let descriptor = self.resolve_descriptor(target);

        // For nodes with resolve_inputs, validate against active inputs only.
        if descriptor.resolve_inputs.is_some() && descriptor.type_family.is_some() {
            return self
                .resolve_node_inputs(&target.id)
                .iter()
                .find(|i| i.id == target_handle)
                .is_some_and(|i| is_connection_compatible(source_type, i.default_type()));
        }
        let Some(input) = descriptor.input(target_handle) else { return false };

        // An inferred zone item takes the type of any initial value, and its
        // next value converts to that type.
        if input.family.as_deref() == Some(&inferred_item_family(target_handle)) {
            return is_zone_input(&target.node_type)
                || is_connection_compatible(source_type, self.resolve_input_type(&target.id, target_handle));
        }
        // If the target has a node-wide override, validate against that type.
        if let Some(forced) = target.data.types.get(&input_family_key(input, &descriptor.inputs)) {
            return is_connection_compatible(source_type, *forced);
        }
        // Valid if the source can be converted to any type within the family.
        input.types.iter().any(|t| is_connection_compatible(source_type, *t))
    }

    /// Whether `connection` may be made in the active document. Rejects
    /// connections that would close a cycle (zones included), type
    /// mismatches (a higher-N value into a lower-N input), connections
    /// breaking zone rules or invalidating existing links, and anything but
    /// an enum picking a Match node's case. Dropping on a zone's virtual
    /// "new item" handle accepts any type.
    pub fn is_valid_connection(&self, c: &Connection) -> bool {
        let doc = self.active_document();
        let (Some(source), Some(target)) = (doc.node(&c.source), doc.node(&c.target)) else {
            return false;
        };
        if c.source_handle == NEW_ITEM_HANDLE && c.target_handle == NEW_ITEM_HANDLE {
            return false;
        }
        if would_create_cycle(&doc.nodes, &doc.edges, &c.source, &c.target) {
            return false;
        }
        if c.source_handle != NEW_ITEM_HANDLE
            && c.target_handle != NEW_ITEM_HANDLE
            && !self.link_fits(source, &c.source_handle, target, &c.target_handle)
        {
            return false;
        }
        // A Match node's cases come from its enum: nothing else can pick one.
        if target.is(MATCH)
            && c.target_handle == MATCH_SELECTOR
            && EnumDocument::of(doc).enum_of_output(&c.source, &c.source_handle).is_none()
        {
            return false;
        }
        self.respects_zones(doc, target, c)
    }

    /// Whether adding the link leaves every existing one as valid as it was.
    fn respects_zones(&self, doc: &GraphDocument, target: &GraphNode, c: &Connection) -> bool {
        let descriptor = self.resolve_descriptor(target);
        let variadic = descriptor.input(&c.target_handle).is_some_and(|i| i.variadic);
        let candidate = GraphEdge {
            id: "__candidate__".to_owned(),
            source: c.source.clone(),
            target: c.target.clone(),
            source_handle: c.source_handle.clone(),
            target_handle: c.target_handle.clone(),
        };
        let mut edges: Vec<GraphEdge> = doc
            .edges
            .iter()
            .filter(|e| {
                variadic
                    || c.target_handle == NEW_ITEM_HANDLE
                    || !(e.target == c.target && e.target_handle == c.target_handle)
            })
            .cloned()
            .collect();
        edges.push(candidate);
        let before = self.zone_analysis(doc).invalid_edges;
        let after = analyze_zones(&doc.nodes, &edges, &zone_options(&self.project)).invalid_edges;
        after.iter().all(|id| before.contains(id))
    }

    /// Link two handles of the active document. Non-variadic inputs keep a
    /// single incoming link (the new one replaces it); a zone's virtual handle
    /// creates a state item. Returns whether anything was linked.
    pub fn connect(&mut self, c: Connection) -> bool {
        if self.is_locked() {
            return false;
        }
        if c.source_handle == NEW_ITEM_HANDLE || c.target_handle == NEW_ITEM_HANDLE {
            return self.connect_new_zone_item(&c);
        }
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else { return false };
        let Some(target) = doc.node(&c.target) else { return false };
        let variadic = self
            .resolve_descriptor(target)
            .input(&c.target_handle)
            .is_some_and(|i| i.variadic);
        // A group's output is linked on one of its Group Output nodes at a time.
        let output = target.is(GROUP_OUTPUT);
        let replaced = |e: &GraphEdge| {
            e.target_handle == c.target_handle
                && (e.target == c.target
                    || (output && doc.node(&e.target).is_some_and(|n| n.is(GROUP_OUTPUT))))
                && (!variadic || (e.source == c.source && e.source_handle == c.source_handle))
        };
        let kept: Vec<GraphEdge> = doc.edges.iter().filter(|e| !replaced(e)).cloned().collect();
        let doc = self.doc_mut(&key).expect("checked");
        doc.edges = kept;
        doc.edges.push(c.into_edge());
        self.changed();
        true
    }

    /// Make a connection if it's valid. Returns whether it was made.
    pub fn try_connect(&mut self, c: Connection) -> bool {
        self.is_valid_connection(&c) && self.connect(c)
    }

    pub fn disconnect(&mut self, edge_id: &str) -> bool {
        self.delete_edges(&[edge_id.to_owned()])
    }

    // ── Code generation ──────────────────────────────────────────────────────

    /// Generate the program drawing the project's root document, reading
    /// unlinked input values of the root from uniforms.
    pub fn generate(&self) -> Result<Program, CodegenError> {
        self.generate_with(&ShaderOptions { uniform_scopes: vec![ROOT_ID.to_owned()], ..Default::default() })
    }

    /// The program to run: like `generate`, with the frame values computed on
    /// the CPU and fed to the passes reading them.
    pub fn generate_hoisted(&self) -> Result<Program, CodegenError> {
        self.generate_with(&ShaderOptions {
            uniform_scopes: vec![ROOT_ID.to_owned()],
            hoist_frame_values: true,
        })
    }

    pub fn generate_with(&self, options: &ShaderOptions) -> Result<Program, CodegenError> {
        generate_program(&self.project, &self.resolver, options)
    }

    /// Whether the root has an Output node.
    pub fn has_output(&self) -> bool {
        self.root().nodes.iter().any(|n| n.is(FRAG_OUTPUT))
    }

    /// The model's nodes that are group boundaries, for editors.
    pub fn is_boundary(&self, node_id: &str) -> bool {
        self.node(node_id).is_some_and(|n| n.is(GROUP_INPUT) || n.is(GROUP_OUTPUT))
    }

    // ── Layout ───────────────────────────────────────────────────────────────

    /// Lay out a document's nodes left to right, among themselves (`node_ids`
    /// only, if given), after splitting its branching outputs at reroutes.
    /// Returns whether any node moved or reroute was added.
    pub fn layout_document(
        &mut self,
        definition_ref: &str,
        node_ids: Option<&[String]>,
        options: &LayoutOptions<'_>,
    ) -> Result<bool, LayoutError> {
        if self.project.document(definition_ref).is_none() || is_library_ref(definition_ref) {
            return Ok(false);
        }
        let only: Option<HashSet<&str>> = node_ids.map(|ids| ids.iter().map(String::as_str).collect());
        let branched = self.split_branches(definition_ref, only.as_ref());
        let Some(doc) = self.project.document(definition_ref) else { return Ok(false) };
        let nodes: Vec<GraphNode> = doc
            .nodes
            .iter()
            .filter(|n| only.as_ref().is_none_or(|o| o.contains(n.id.as_str())) || branched.contains(&n.id))
            .cloned()
            .collect();
        let zones = self.zone_analysis(doc);
        let frames = self.frame_analysis(doc);
        let geometry = |node: &GraphNode| {
            options
                .measure
                .and_then(|measure| measure(definition_ref, node))
                .unwrap_or_else(|| self.estimated_geometry(doc, node))
        };
        let positions = compute_layout(&LayoutInput {
            nodes: &nodes,
            edges: &doc.edges,
            zones: &zones,
            frames: &frames,
            geometry: &geometry,
            spacing: options.spacing,
        })?;
        Ok(self.apply_positions(definition_ref, &positions) || !branched.is_empty())
    }

    /// Lay out the selected nodes of the active document, among themselves.
    pub fn layout_selection(&mut self, options: &LayoutOptions<'_>) -> Result<bool, LayoutError> {
        if self.is_locked() || self.selected.is_empty() {
            return Ok(false);
        }
        let key = self.active_definition_ref();
        let selected = self.selected.clone();
        self.layout_document(&key, Some(&selected), options)
    }

    /// Lay out every document of the project (the standard library excluded).
    pub fn layout_project(&mut self, options: &LayoutOptions<'_>) -> Result<bool, LayoutError> {
        let refs: Vec<String> =
            self.project.definitions.keys().filter(|r| !is_library_ref(r)).cloned().collect();
        let mut moved = false;
        for key in refs {
            moved |= self.layout_document(&key, None, options)?;
        }
        Ok(moved)
    }

    /// Split each output linked to several inputs at a reroute, among `only`
    /// (all nodes if absent). Returns the reroutes added.
    fn split_branches(&mut self, definition_ref: &str, only: Option<&HashSet<&str>>) -> HashSet<String> {
        let Some(doc) = self.project.document(definition_ref) else { return HashSet::new() };
        let skipped = self.zone_analysis(doc).invalid_edges;
        let among = |id: &str| only.is_none_or(|o| o.contains(id));
        let RerouteBranches { reroutes, links, moved } =
            reroute_branches(&doc.nodes, &doc.edges, among, |_| new_id(), &skipped);
        if reroutes.is_empty() {
            return HashSet::new();
        }
        let added: HashSet<String> = reroutes.iter().map(|n| n.id.clone()).collect();
        let moved: HashMap<String, GraphEdge> = moved.into_iter().map(|e| (e.id.clone(), e)).collect();
        let Some(doc) = self.doc_mut(definition_ref) else { return HashSet::new() };
        for edge in &mut doc.edges {
            if let Some(replacement) = moved.get(&edge.id) {
                *edge = replacement.clone();
            }
        }
        doc.edges.extend(links);
        doc.nodes.extend(reroutes);
        self.changed();
        added
    }

    /// Move nodes of a document to `positions`, where they differ. Returns whether any moved.
    fn apply_positions(&mut self, definition_ref: &str, positions: &HashMap<String, Point>) -> bool {
        let Some(doc) = self.doc_mut(definition_ref) else { return false };
        let mut moved = false;
        for node in &mut doc.nodes {
            if let Some(at) = positions.get(&node.id).filter(|at| **at != node.position) {
                node.position = *at;
                moved = true;
            }
        }
        if moved {
            self.changed();
        }
        moved
    }

    /// A node's geometry when no front end measured it.
    fn estimated_geometry(&self, doc: &GraphDocument, node: &GraphNode) -> NodeGeometry {
        if node.is(LAYOUT_FRAME) {
            let header = estimate_frame_header(&self.node_label(&node.id), frame_note(&node.data));
            return NodeGeometry { width: header.width, height: header.height, ..Default::default() };
        }
        if node.is(REROUTE) {
            return NodeGeometry {
                width: 18.0,
                height: 18.0,
                inputs: vec![("in".to_owned(), 9.0)],
                outputs: vec![("out".to_owned(), 9.0)],
            };
        }
        estimate_node_geometry(&self.layout_rows(doc, node))
    }

    /// What a node shows, top to bottom, for estimating its geometry.
    fn layout_rows(&self, doc: &GraphDocument, node: &GraphNode) -> NodeRows {
        let descriptor = self.resolve_descriptor(node);
        let linked: HashSet<&str> = doc.edges_into(&node.id).map(|e| e.target_handle.as_str()).collect();
        let linked_out: HashSet<&str> = doc.edges_from(&node.id).map(|e| e.source_handle.as_str()).collect();
        let hidden = self.hidden_inputs(&node.id);
        let shown = |id: &str, linked_to: &HashSet<&str>| {
            node.data.hide_unlinked != Some(true) || linked_to.contains(id)
        };
        let header = descriptor.settings.len()
            + usize::from(self.type_choice(&node.id).is_some())
            + self.variable_choices(&node.id).iter().filter(|c| c.variable.types.len() > 1).count();
        NodeRows {
            header,
            outputs: self
                .resolve_node_outputs(&node.id)
                .into_iter()
                .map(|o| o.id)
                .filter(|id| shown(id.as_str(), &linked_out))
                .collect(),
            widget: descriptor.widget.as_ref().map_or(0, widget_rows),
            inputs: self
                .resolve_node_inputs(&node.id)
                .into_iter()
                .filter(|i| !hidden.contains(&i.id) && shown(i.id.as_str(), &linked))
                .map(|i| InputRow {
                    editor: !linked.contains(i.id.as_str()) && !i.fallback && !i.variadic,
                    id: i.id,
                })
                .collect(),
        }
    }
}

/// A widget's height in rows, as the front end draws it.
fn widget_rows(widget: &NodeWidget) -> usize {
    match widget {
        NodeWidget::Color(_) => 6,
        NodeWidget::Curve => 10,
        NodeWidget::Ramp => 4,
        NodeWidget::Code | NodeWidget::Asset(_) => 5,
        NodeWidget::Cases => 1,
        NodeWidget::Items => 2,
    }
}

/// Copy nodes and the edges between them with fresh ids (keeping zone
/// pairings), shifted by `offset`. Returns the copies and the id mapping.
pub fn clone_subgraph(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    offset: Point,
) -> (Vec<GraphNode>, Vec<GraphEdge>, HashMap<String, String>) {
    let id_map: HashMap<String, String> = nodes.iter().map(|n| (n.id.clone(), new_id())).collect();
    let remap = |id: &str| id_map.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let cloned_nodes = nodes
        .iter()
        .map(|n| {
            let mut data = n.data.clone();
            if let Some(paired) = data.paired_output.as_ref().and_then(|p| id_map.get(p)) {
                data.paired_output = Some(paired.clone());
            }
            GraphNode {
                id: remap(&n.id),
                node_type: n.node_type.clone(),
                data,
                position: Point::new(n.position.x + offset.x, n.position.y + offset.y),
                group: n.group.clone(),
                frame: n.frame.as_ref().filter(|f| id_map.contains_key(*f)).map(|f| remap(f)),
            }
        })
        .collect();
    let cloned_edges = edges
        .iter()
        .filter(|e| id_map.contains_key(&e.source) && id_map.contains_key(&e.target))
        .map(|e| GraphEdge {
            id: new_id(),
            source: remap(&e.source),
            target: remap(&e.target),
            source_handle: e.source_handle.clone(),
            target_handle: e.target_handle.clone(),
        })
        .collect();
    (cloned_nodes, cloned_edges, id_map)
}
