//! Groups: creating, grouping a selection, ungrouping, and editing a group's
//! interface (inputs, outputs, enums, type variables).

use std::collections::HashSet;

use super::model::{GraphModel, clone_subgraph};
use super::{
    GROUP_INPUT, GROUP_OUTPUT, GraphDocument, GraphEdge, GraphNode, GroupRef,
    GroupSide, NODE_GROUP, Point, ROOT_ID, new_id,
};
use crate::library::{self, is_library_ref};
use super::model_interface::fit_type_owners;
use crate::enums::{fit_variants, is_under, owner_chain, owner_exists, shown_apart};
use crate::types::{
    EnumVariant, GroupHandleDef, HandleType, InputOwner, InputValue, TypeVariable, WgslType,
    toggle_variants,
};
use crate::zones::paired_output_id;

const UNGROUPABLE_TYPES: [&str; 3] = ["frag-output", GROUP_INPUT, GROUP_OUTPUT];
const GROUP_OFFSET: f32 = 300.0;

/// A group definition that pruning would remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunableGroup {
    pub reference: String,
    pub label: String,
    /// Its nodes, not counting the group's input and output boundaries.
    pub node_count: usize,
}

fn reachable_from(doc: &GraphDocument, starts: &[&str]) -> HashSet<String> {
    let mut seen: HashSet<String> = starts.iter().map(|s| (*s).to_owned()).collect();
    let mut stack: Vec<String> = seen.iter().cloned().collect();
    while let Some(current) = stack.pop() {
        let mut next: Vec<&str> = doc.edges_from(&current).map(|e| e.target.as_str()).collect();
        // A zone's output depends on its input without a link saying so.
        if let Some(output) = doc.node(&current).and_then(paired_output_id) {
            next.push(output);
        }
        for n in next {
            if seen.insert(n.to_owned()) {
                stack.push(n.to_owned());
            }
        }
    }
    seen
}

impl GraphModel {
    /// "Group N", the next label no user group has.
    fn next_group_label(&self) -> String {
        let labels: HashSet<&str> = self
            .project
            .definitions
            .iter()
            .filter(|(k, _)| k.as_str() != ROOT_ID)
            .filter_map(|(_, d)| d.label.as_deref())
            .collect();
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|l| !labels.contains(l.as_str()))
            .expect("unbounded")
    }

    /// Add an empty group (one input passed to one output) to the active
    /// document. Returns the group node's id.
    pub fn create_group(&mut self, position: Point) -> Option<String> {
        if self.is_locked() {
            return None;
        }
        let (input_node, output_node) = (new_id(), new_id());
        let input = GroupHandleDef::new(new_id(), "Input 1", WgslType::F32.into());
        let output = GroupHandleDef::new(new_id(), "Output 1", WgslType::F32.into());
        let mut inner_in = GraphNode::new(GROUP_INPUT, Point::new(-200.0, 0.0));
        inner_in.id = input_node.clone();
        let mut inner_out = GraphNode::new(GROUP_OUTPUT, Point::new(200.0, 0.0));
        inner_out.id = output_node.clone();
        let definition = GraphDocument {
            nodes: vec![inner_in, inner_out],
            edges: vec![GraphEdge {
                id: new_id(),
                source: input_node,
                source_handle: input.id.clone(),
                target: output_node,
                target_handle: output.id.clone(),
            }],
            label: Some(self.next_group_label()),
            inputs: vec![input],
            outputs: vec![output],
            ..Default::default()
        };
        self.add_instance_of(definition, position)
    }

    fn add_instance_of(&mut self, definition: GraphDocument, position: Point) -> Option<String> {
        let reference = new_id();
        let mut node = GraphNode::new(NODE_GROUP, position);
        node.group = Some(GroupRef { definition_ref: reference.clone(), locked: false });
        let id = node.id.clone();
        let key = self.active_definition_ref();
        self.project.definitions.insert(reference, definition);
        self.project.definitions.get_mut(&key)?.nodes.push(node);
        self.changed();
        Some(id)
    }

    /// Add another instance of an existing definition to the active document.
    pub fn add_group_instance(&mut self, definition_ref: &str, position: Point) -> Option<String> {
        if self.is_locked()
            || definition_ref == ROOT_ID
            || !self.project.definitions.contains_key(definition_ref)
            || self.would_create_group_cycle(definition_ref)
        {
            return None;
        }
        let locked = is_library_ref(definition_ref)
            || self.project.definitions.get(definition_ref).is_some_and(|def| def.locked);
        let mut node = GraphNode::new(NODE_GROUP, position);
        node.group = Some(GroupRef { definition_ref: definition_ref.to_owned(), locked });
        let id = node.id.clone();
        let key = self.active_definition_ref();
        self.project.definitions.get_mut(&key)?.nodes.push(node);
        self.changed();
        Some(id)
    }

    /// Whether an instance of `definition_ref` in the active document would
    /// contain itself.
    pub fn would_create_group_cycle(&self, definition_ref: &str) -> bool {
        let active = self.active_definition_ref();
        if active == definition_ref {
            return true;
        }
        let mut seen = HashSet::new();
        let mut stack = vec![definition_ref.to_owned()];
        while let Some(current) = stack.pop() {
            if current == active {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(doc) = self.project.document(&current) {
                stack.extend(doc.group_refs().map(str::to_owned));
            }
        }
        false
    }

    /// Grouping collapses the selection into one node. If a path leaves the
    /// selection and comes back into it (zone dependencies included), that
    /// node would depend on itself.
    fn grouping_would_create_cycle(&self, selection: &HashSet<String>) -> bool {
        let doc = self.active_document();
        let outside: Vec<&str> = doc
            .edges
            .iter()
            .filter(|e| selection.contains(&e.source) && !selection.contains(&e.target))
            .map(|e| e.target.as_str())
            .collect();
        reachable_from(doc, &outside).iter().any(|id| selection.contains(id))
    }

    /// Collapse nodes of the active document into a new group. Zones go as a
    /// whole: selecting either boundary brings in its partner and everything
    /// inside. Returns the group node's id.
    pub fn group_selection(&mut self, selected: &[String]) -> Option<String> {
        if self.is_locked() {
            return None;
        }
        let zones = self.active_zones();
        let frames = self.active_frames();
        // A frame is grouped with what it holds.
        let mut expanded = self.with_frame_contents(selected);
        for id in selected {
            if let Some(zone) = zones.zone_of(id) {
                expanded.insert(zone.input_id.clone());
                expanded.insert(zone.output_id.clone());
                expanded.extend(zone.members.iter().cloned());
            }
        }
        let doc = self.active_document().clone();
        let nodes: Vec<GraphNode> = doc
            .nodes
            .iter()
            .filter(|n| expanded.contains(&n.id) && !UNGROUPABLE_TYPES.contains(&n.node_type.as_str()))
            .cloned()
            .collect();
        if nodes.is_empty() || nodes.iter().all(|n| n.is(NODE_GROUP)) {
            return None;
        }
        let ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        if self.grouping_would_create_cycle(&ids) {
            return None;
        }
        let (min_x, max_x) = nodes.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| {
            (lo.min(n.position.x), hi.max(n.position.x))
        });
        let (min_y, max_y) = nodes.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| {
            (lo.min(n.position.y), hi.max(n.position.y))
        });
        let center = Point::new((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);

        let internal: Vec<&GraphEdge> =
            doc.edges.iter().filter(|e| ids.contains(&e.source) && ids.contains(&e.target)).collect();
        let incoming: Vec<&GraphEdge> =
            doc.edges.iter().filter(|e| !ids.contains(&e.source) && ids.contains(&e.target)).collect();
        let outgoing: Vec<&GraphEdge> =
            doc.edges.iter().filter(|e| ids.contains(&e.source) && !ids.contains(&e.target)).collect();
        let dedupe = |edges: &[&GraphEdge]| -> Vec<Vec<GraphEdge>> {
            let mut groups: Vec<((String, String), Vec<GraphEdge>)> = Vec::new();
            for e in edges {
                let key = (e.source.clone(), e.source_handle.clone());
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, list)) => list.push((*e).clone()),
                    None => groups.push((key, vec![(*e).clone()])),
                }
            }
            groups.into_iter().map(|(_, list)| list).collect()
        };
        let (deduped_in, deduped_out) = (dedupe(&incoming), dedupe(&outgoing));

        let group_id = new_id();
        let mut input_node = GraphNode::new(GROUP_INPUT, Point::new(min_x - GROUP_OFFSET, center.y));
        let mut output_node = GraphNode::new(GROUP_OUTPUT, Point::new(max_x + GROUP_OFFSET, center.y));
        input_node.data = Default::default();
        output_node.data = Default::default();
        let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
        let (mut outer_edges, mut inner_edges): (Vec<GraphEdge>, Vec<GraphEdge>) = (Vec::new(), Vec::new());
        for (i, group) in deduped_in.iter().enumerate() {
            let first = &group[0];
            let ty = self.resolve_output_handle_type(&first.source, &first.source_handle);
            let handle = GroupHandleDef::new(new_id(), format!("Input {}", i + 1), ty.into());
            outer_edges.push(GraphEdge {
                id: new_id(),
                source: first.source.clone(),
                source_handle: first.source_handle.clone(),
                target: group_id.clone(),
                target_handle: handle.id.clone(),
            });
            inner_edges.extend(group.iter().map(|e| GraphEdge {
                id: new_id(),
                source: input_node.id.clone(),
                source_handle: handle.id.clone(),
                target: e.target.clone(),
                target_handle: e.target_handle.clone(),
            }));
            inputs.push(handle);
        }
        for (i, group) in deduped_out.iter().enumerate() {
            let first = &group[0];
            let ty = self.resolve_output_handle_type(&first.source, &first.source_handle);
            let handle = GroupHandleDef::new(new_id(), format!("Output {}", i + 1), ty.into());
            inner_edges.push(GraphEdge {
                id: new_id(),
                source: first.source.clone(),
                source_handle: first.source_handle.clone(),
                target: output_node.id.clone(),
                target_handle: handle.id.clone(),
            });
            outer_edges.extend(group.iter().map(|e| GraphEdge {
                id: new_id(),
                source: group_id.clone(),
                source_handle: handle.id.clone(),
                target: e.target.clone(),
                target_handle: e.target_handle.clone(),
            }));
            outputs.push(handle);
        }
        let mut definition_nodes: Vec<GraphNode> = nodes.clone();
        // Frames left outside don't come along.
        for node in &mut definition_nodes {
            if node.frame.as_ref().is_some_and(|f| !ids.contains(f)) {
                node.frame = None;
            }
        }
        definition_nodes.extend([input_node, output_node]);
        let definition = GraphDocument {
            nodes: definition_nodes,
            edges: internal.iter().map(|e| (*e).clone()).chain(inner_edges).collect(),
            label: Some(self.next_group_label()),
            inputs,
            outputs,
            ..Default::default()
        };
        let reference = new_id();
        let mut group_node = GraphNode::new(NODE_GROUP, center);
        group_node.id = group_id.clone();
        group_node.group = Some(GroupRef { definition_ref: reference.clone(), locked: false });
        group_node.frame = frames.common_frame(ids.iter().map(String::as_str));
        let touched: HashSet<&str> =
            internal.iter().chain(&incoming).chain(&outgoing).map(|e| e.id.as_str()).collect();
        let key = self.active_definition_ref();
        self.project.definitions.insert(reference, definition);
        let doc = self.project.definitions.get_mut(&key)?;
        doc.nodes.retain(|n| !ids.contains(&n.id));
        doc.nodes.push(group_node);
        doc.edges.retain(|e| !touched.contains(e.id.as_str()));
        doc.edges.extend(outer_edges);
        self.selected.clear();
        self.changed();
        Some(group_id)
    }

    /// Replace group nodes of the active document by what's inside them.
    /// Returns the ids of the freshly created inner nodes.
    pub fn ungroup_selection(&mut self, selected: &[String]) -> Vec<String> {
        if self.is_locked() {
            return Vec::new();
        }
        let groups: Vec<GraphNode> = self
            .active_document()
            .nodes
            .iter()
            .filter(|n| selected.contains(&n.id) && n.is(NODE_GROUP) && n.group.is_some())
            .cloned()
            .collect();
        let fresh: Vec<String> = groups.iter().flat_map(|g| self.ungroup_node(g)).collect();
        self.selected.clone_from(&fresh);
        fresh
    }

    fn ungroup_node(&mut self, group_node: &GraphNode) -> Vec<String> {
        let Some(reference) = group_node.definition_ref() else { return Vec::new() };
        let Some(def) = self.project.document(reference).cloned() else { return Vec::new() };
        let inner_nodes: Vec<GraphNode> =
            def.nodes.iter().filter(|n| n.side().is_none()).cloned().collect();
        // Re-center the inner nodes on the group node.
        let center = if inner_nodes.is_empty() {
            Point::default()
        } else {
            let n = inner_nodes.len() as f32;
            Point::new(
                inner_nodes.iter().map(|n| n.position.x).sum::<f32>() / n,
                inner_nodes.iter().map(|n| n.position.y).sum::<f32>() / n,
            )
        };
        let offset = Point::new(group_node.position.x - center.x, group_node.position.y - center.y);
        let (mut nodes, edges, id_map) = clone_subgraph(&inner_nodes, &def.edges, offset);
        // What was in no frame of the group goes in the group node's.
        if let Some(frame) = &group_node.frame {
            for node in nodes.iter_mut().filter(|n| n.frame.is_none()) {
                node.frame = Some(frame.clone());
            }
        }
        let remap = |id: &str| id_map.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else { return Vec::new() };
        let incoming: Vec<GraphEdge> = doc.edges_into(&group_node.id).cloned().collect();
        let outgoing: Vec<GraphEdge> = doc.edges_from(&group_node.id).cloned().collect();
        let mut boundary: Vec<GraphEdge> = Vec::new();
        // group-input handle -> inner target. Splice: outer source -> inner target.
        for inner in &def.edges {
            if def.node(&inner.source).and_then(GraphNode::side) != Some(GroupSide::Input) {
                continue;
            }
            let Some(feed) = incoming.iter().find(|e| e.target_handle == inner.source_handle) else {
                continue;
            };
            boundary.push(GraphEdge {
                id: new_id(),
                source: feed.source.clone(),
                source_handle: feed.source_handle.clone(),
                target: remap(&inner.target),
                target_handle: inner.target_handle.clone(),
            });
        }
        // inner source -> group-output handle. Splice: inner source -> outer target(s).
        for inner in &def.edges {
            if def.node(&inner.target).and_then(GraphNode::side) != Some(GroupSide::Output) {
                continue;
            }
            for consumer in outgoing.iter().filter(|e| e.source_handle == inner.target_handle) {
                boundary.push(GraphEdge {
                    id: new_id(),
                    source: remap(&inner.source),
                    source_handle: inner.source_handle.clone(),
                    target: consumer.target.clone(),
                    target_handle: consumer.target_handle.clone(),
                });
            }
        }
        let removed: HashSet<String> = incoming.iter().chain(&outgoing).map(|e| e.id.clone()).collect();
        let ids = nodes.iter().map(|n| n.id.clone()).collect();
        let doc = self.project.definitions.get_mut(&key).expect("active");
        doc.nodes.retain(|n| n.id != group_node.id);
        doc.nodes.extend(nodes);
        doc.edges.retain(|e| !removed.contains(&e.id));
        doc.edges.extend(edges);
        doc.edges.extend(boundary);
        self.changed();
        ids
    }

    pub fn set_group_label(&mut self, node_id: &str, label: &str) {
        let label = label.trim();
        let Some(reference) = self.node(node_id).and_then(|n| n.definition_ref()).map(str::to_owned) else {
            return;
        };
        if label.is_empty() {
            return;
        }
        if let Some(doc) = self.project.definitions.get_mut(&reference) {
            doc.label = Some(label.to_owned());
            self.changed();
        }
    }

    /// Delete a group's definition, and the nodes instancing it.
    pub fn delete_definition(&mut self, definition_ref: &str) {
        if definition_ref == ROOT_ID
            || is_library_ref(definition_ref)
            || self.project.definitions.remove(definition_ref).is_none()
        {
            return;
        }
        let mut gone: HashSet<String> = HashSet::new();
        for doc in self.project.definitions.values_mut() {
            let instances: Vec<String> = doc
                .nodes
                .iter()
                .filter(|n| n.definition_ref() == Some(definition_ref))
                .map(|n| n.id.clone())
                .collect();
            doc.nodes.retain(|n| !instances.contains(&n.id));
            doc.edges.retain(|e| !instances.contains(&e.source) && !instances.contains(&e.target));
            gone.extend(instances);
        }
        self.selected.retain(|id| !gone.contains(id));
        self.changed();
    }

    /// Definitions no group node references, directly or through others.
    pub fn prunable_refs(&self) -> Vec<String> {
        let mut used: HashSet<String> = HashSet::new();
        let mut stack = vec![ROOT_ID.to_owned()];
        while let Some(current) = stack.pop() {
            if let Some(doc) = self.project.document(&current) {
                for r in doc.group_refs() {
                    if used.insert(r.to_owned()) {
                        stack.push(r.to_owned());
                    }
                }
            }
        }
        self.project
            .definitions
            .keys()
            .filter(|k| k.as_str() != ROOT_ID && !used.contains(*k) && !is_library_ref(k))
            .cloned()
            .collect()
    }

    /// What `prune_unused_definitions` would remove, for confirming it with
    /// the user. Empty inside a locked group, where pruning isn't allowed.
    pub fn prunable_groups(&self) -> Vec<PrunableGroup> {
        if self.is_locked() {
            return Vec::new();
        }
        self.prunable_refs()
            .into_iter()
            .filter_map(|reference| {
                let def = self.project.document(&reference)?;
                Some(PrunableGroup {
                    label: def.label.clone().unwrap_or_else(|| "Group".to_owned()),
                    node_count: def.nodes.iter().filter(|n| n.side().is_none()).count(),
                    reference,
                })
            })
            .collect()
    }

    /// Removes the definitions `prunable_refs` lists, returning them. Nothing
    /// is removed inside a locked group.
    pub fn prune_unused_definitions(&mut self) -> Vec<String> {
        if self.is_locked() {
            return Vec::new();
        }
        let refs = self.prunable_refs();
        for r in &refs {
            self.project.definitions.remove(r);
        }
        if !refs.is_empty() {
            self.changed();
        }
        refs
    }

    /// The label of the standard-library group a node uses an older version
    /// of. The latest version stands in for it, so its links may not match.
    pub fn outdated_library_label(&self, node_id: &str) -> Option<String> {
        let reference = self.project.resolve_node(node_id)?.definition_ref()?;
        let latest = library::entry_for_ref(reference)?;
        (latest.reference != reference).then(|| latest.label.clone())
    }

    // ── Interface ────────────────────────────────────────────────────────────

    pub(super) fn user_definition(&mut self, definition_ref: &str) -> Option<&mut GraphDocument> {
        if definition_ref == ROOT_ID || is_library_ref(definition_ref) {
            return None;
        }
        self.project.definitions.get_mut(definition_ref)
    }

    /// Add an input or output to a group, named after the next free number.
    /// A variants list makes it an enum (or, for a bool, a checkbox).
    pub fn add_handle(
        &mut self,
        definition_ref: &str,
        side: GroupSide,
        ty: HandleType,
        variants: Option<Vec<EnumVariant>>,
        owner: Option<InputOwner>,
    ) -> Option<GroupHandleDef> {
        let def = self.user_definition(definition_ref)?;
        let handles = def.interface(side);
        let input = side == GroupSide::Input;
        let toggle = variants.is_some() && ty == HandleType::Fixed(WgslType::Bool);
        let prefix = if toggle {
            "Checkbox"
        } else if variants.is_some() {
            "Enum"
        } else if input {
            "Input"
        } else {
            "Output"
        };
        let labels: HashSet<&str> = handles.iter().map(|h| h.label.as_str()).collect();
        let n = (1..).find(|n| !labels.contains(format!("{prefix} {n}").as_str())).expect("unbounded");
        // Only an output's type can be whatever reaches it.
        let ty = if toggle {
            WgslType::Bool.into()
        } else if variants.is_some() {
            WgslType::U32.into()
        } else if input && ty.is_inferred() {
            WgslType::F32.into()
        } else {
            ty
        };
        let mut handle = GroupHandleDef::new(new_id(), format!("{prefix} {n}"), ty);
        if input {
            handle.variants = variants;
            if let Some(owner) = owner.filter(|o| owner_exists(handles, Some(o))) {
                handle.owner = Some(owner);
            }
        }
        let mut at = handles.len();
        if let Some(owner) = &handle.owner {
            let enum_id = owner.enum_id.clone();
            let last = handles
                .iter()
                .rposition(|h| h.id == enum_id || is_under(handles, h, &enum_id, None));
            at = last.map_or(at, |l| l + 1);
        }
        def.interface_mut(side).insert(at, handle.clone());
        self.changed();
        Some(handle)
    }

    /// Add an input or output to the group a boundary node is in.
    pub fn add_boundary_handle(&mut self, node_id: &str, ty: HandleType) -> Option<GroupHandleDef> {
        let side = self.node(node_id)?.side()?;
        let reference = self.project.definition_ref_of(node_id)?.to_owned();
        self.add_handle(&reference, side, ty, None, None)
    }

    pub(super) fn update_handle(
        &mut self,
        definition_ref: &str,
        side: GroupSide,
        handle_id: &str,
        update: impl FnOnce(&mut GroupHandleDef),
    ) -> bool {
        let Some(def) = self.user_definition(definition_ref) else { return false };
        let Some(handle) = def.interface_mut(side).iter_mut().find(|h| h.id == handle_id) else {
            return false;
        };
        update(handle);
        self.changed();
        true
    }

    /// Rename an input or output of a group. Rejected (returning false) when
    /// another of the same side has that label, unless the two are never
    /// shown together (inputs of different variants or types).
    pub fn set_handle_label(
        &mut self,
        definition_ref: &str,
        side: GroupSide,
        handle_id: &str,
        label: &str,
    ) -> bool {
        let label = label.trim().to_owned();
        let Some(def) = self.project.document(definition_ref) else { return false };
        let handles = def.interface(side);
        let Some(handle) = handles.iter().find(|h| h.id == handle_id) else { return false };
        let taken = handles.iter().any(|h| {
            h.id != handle_id
                && h.label == label
                && !(side == GroupSide::Input && shown_apart(handles, handle, h))
        });
        if label.is_empty() || taken {
            return false;
        }
        self.update_handle(definition_ref, side, handle_id, |h| h.label = label)
    }

    /// Retype an input or output; only a u32 stays an enum, and links that no
    /// longer fit are removed.
    pub fn set_handle_type(
        &mut self,
        definition_ref: &str,
        side: GroupSide,
        handle_id: &str,
        ty: HandleType,
    ) {
        if side == GroupSide::Input && ty.is_inferred() {
            return;
        }
        let enum_owner = self
            .project
            .document(definition_ref)
            .and_then(|d| d.interface(side).iter().find(|h| h.id == handle_id))
            .and_then(|h| h.owner.clone());
        let Some(def) = self.user_definition(definition_ref) else { return };
        for h in def.interface_mut(side) {
            if h.id == handle_id {
                h.ty = ty.clone();
                if ty != HandleType::Fixed(WgslType::U32) {
                    h.variants = None;
                    h.variants_by = None;
                }
                if !matches!(ty, HandleType::Fixed(WgslType::Vec3f | WgslType::Vec4f)) {
                    h.display = None;
                }
                if !h.can_compile() {
                    h.compiled = None;
                }
            } else if ty != HandleType::Fixed(WgslType::U32)
                && h.owner.as_ref().is_some_and(|o| o.enum_id == handle_id)
            {
                // The inputs of a former enum's variants take its place.
                h.owner = enum_owner.clone();
            }
        }
        self.changed();
        self.drop_unfit_links_to_instances(definition_ref, &[handle_id.to_owned()]);
    }

    /// Drop the links of every instance of a definition whose types no longer fit.
    fn drop_unfit_links_to_instances(&mut self, definition_ref: &str, handles: &[String]) {
        let keys: Vec<String> = self.project.definitions.keys().cloned().collect();
        let mut dropped = false;
        for key in keys {
            let Some(doc) = self.project.document(&key) else { continue };
            let instances: HashSet<&str> = doc
                .nodes
                .iter()
                .filter(|n| n.definition_ref() == Some(definition_ref))
                .map(|n| n.id.as_str())
                .collect();
            let unfit: HashSet<String> = doc
                .edges
                .iter()
                .filter(|e| {
                    (instances.contains(e.target.as_str()) && handles.contains(&e.target_handle))
                        || (instances.contains(e.source.as_str()) && handles.contains(&e.source_handle))
                })
                .filter(|e| match (doc.node(&e.source), doc.node(&e.target)) {
                    (Some(s), Some(t)) => !self.link_fits(s, &e.source_handle, t, &e.target_handle),
                    _ => true,
                })
                .map(|e| e.id.clone())
                .collect();
            if !unfit.is_empty() {
                if let Some(doc) = self.project.definitions.get_mut(&key) {
                    doc.edges.retain(|e| !unfit.contains(&e.id));
                }
                dropped = true;
            }
        }
        if dropped {
            self.changed();
        }
    }

    /// Set what a group's input holds on group nodes until edited.
    pub fn set_handle_default(&mut self, definition_ref: &str, handle_id: &str, value: Option<InputValue>) {
        if let Some(def) = self.user_definition(definition_ref) {
            match value {
                Some(v) => {
                    def.defaults.insert(handle_id.to_owned(), v);
                }
                None => {
                    def.defaults.remove(handle_id);
                }
            }
            self.changed();
        }
    }

    /// Make an input an enum choosing among `variants` (it becomes a u32),
    /// or a plain input again with `None`. Inputs under removed variants go
    /// with them, and Match nodes linked to it follow.
    pub fn set_handle_variants(
        &mut self,
        definition_ref: &str,
        handle_id: &str,
        variants: Option<Vec<EnumVariant>>,
    ) {
        let Some(def) = self.project.document(definition_ref) else { return };
        let is_toggle = def.inputs.iter().any(|h| {
            h.id == handle_id && h.ty == HandleType::Fixed(WgslType::Bool) && h.variants.is_some()
        });
        if variants.is_some() && !is_toggle {
            self.set_handle_type(definition_ref, GroupSide::Input, handle_id, WgslType::U32.into());
        }
        let Some(def) = self.user_definition(definition_ref) else { return };
        let handles = def.inputs.clone();
        let variables = def.type_variables.clone();
        let Some(handle) = def.inputs.iter_mut().find(|h| h.id == handle_id) else { return };
        handle.compiled = None;
        handle.variants = variants.clone();
        if variants.is_some() {
            *handle = fit_variants(handle, &variables);
        } else {
            handle.variants_by = None;
        }
        let kept: Vec<f64> = variants.iter().flatten().map(|v| v.value).collect();
        let orphans: Vec<String> = handles
            .iter()
            .filter(|h| owner_chain(&handles, h).iter().any(|o| o.enum_id == handle_id && !kept.contains(&o.value)))
            .map(|h| h.id.clone())
            .collect();
        self.changed();
        if !orphans.is_empty() {
            self.remove_handles(definition_ref, GroupSide::Input, &orphans);
        }
    }

    /// Make an input a checkbox: a bool compiled into the shader.
    pub fn toggle_variants() -> Vec<EnumVariant> {
        toggle_variants()
    }

    pub fn remove_handle(&mut self, definition_ref: &str, side: GroupSide, handle_id: &str) {
        let Some(def) = self.project.document(definition_ref) else { return };
        let handles = def.interface(side);
        if !handles.iter().any(|h| h.id == handle_id) {
            return;
        }
        let mut ids = vec![handle_id.to_owned()];
        ids.extend(
            handles.iter().filter(|h| is_under(handles, h, handle_id, None)).map(|h| h.id.clone()),
        );
        self.remove_handles(definition_ref, side, &ids);
    }

    pub(super) fn remove_handles(&mut self, definition_ref: &str, side: GroupSide, ids: &[String]) {
        let Some(def) = self.user_definition(definition_ref) else { return };
        def.interface_mut(side).retain(|h| !ids.contains(&h.id));
        if side == GroupSide::Input {
            def.defaults.retain(|id, _| !ids.contains(id));
        }
        // The links to them on every boundary node of that side.
        let boundary: HashSet<String> = def
            .nodes
            .iter()
            .filter(|n| n.side() == Some(side))
            .map(|n| n.id.clone())
            .collect();
        def.edges.retain(|e| {
            !((boundary.contains(&e.source) && ids.contains(&e.source_handle))
                || (boundary.contains(&e.target) && ids.contains(&e.target_handle)))
        });
        // And the links of every instance: the handles no longer exist there.
        for doc in self.project.definitions.values_mut() {
            let instances: HashSet<String> = doc
                .nodes
                .iter()
                .filter(|n| n.definition_ref() == Some(definition_ref))
                .map(|n| n.id.clone())
                .collect();
            doc.edges.retain(|e| {
                !((instances.contains(&e.source) && ids.contains(&e.source_handle))
                    || (instances.contains(&e.target) && ids.contains(&e.target_handle)))
            });
        }
        self.changed();
    }

    /// Move a handle up (-1) or down (+1) among its side's.
    pub fn move_handle(&mut self, definition_ref: &str, side: GroupSide, handle_id: &str, offset: isize) {
        let Some(def) = self.user_definition(definition_ref) else { return };
        let list = def.interface_mut(side);
        let Some(index) = list.iter().position(|h| h.id == handle_id) else { return };
        let Some(target) = index.checked_add_signed(offset).filter(|t| *t < list.len()) else {
            return;
        };
        list.swap(index, target);
        self.changed();
    }

    // ── Type variables ───────────────────────────────────────────────────────

    /// Add a type variable to a group, named after the next free letter from T.
    pub fn add_type_variable(&mut self, definition_ref: &str, types: &[WgslType]) -> Option<TypeVariable> {
        if types.is_empty() {
            return None;
        }
        let def = self.user_definition(definition_ref)?;
        let name = "TUVWXYZABCDEFGHIJKLMNOPQRS"
            .chars()
            .map(String::from)
            .find(|n| !def.type_variables.iter().any(|v| &v.name == n))
            .unwrap_or_else(|| format!("T{}", def.type_variables.len() + 1));
        let variable = TypeVariable { id: new_id(), name, types: types.to_vec() };
        def.type_variables.push(variable.clone());
        self.changed();
        Some(variable)
    }

    /// Change a variable's name and the types it may take. Group nodes
    /// forcing a type it no longer has go back to inferring it; sockets keep
    /// the variable, and links that no longer fit are dropped.
    pub fn set_type_variable(
        &mut self,
        definition_ref: &str,
        variable_id: &str,
        name: Option<&str>,
        types: Option<&[WgslType]>,
    ) {
        let types = types.filter(|t| !t.is_empty());
        let Some(def) = self.user_definition(definition_ref) else { return };
        let Some(variable) = def.type_variables.iter_mut().find(|v| v.id == variable_id) else {
            return;
        };
        if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
            variable.name = name.to_owned();
        }
        if let Some(types) = types {
            variable.types = types.to_vec();
            fit_type_owners(def);
        }
        let handles = self.handles_of_variable(definition_ref, variable_id);
        if let Some(types) = types {
            for doc in self.project.definitions.values_mut() {
                for node in doc.nodes.iter_mut().filter(|n| n.definition_ref() == Some(definition_ref)) {
                    if node.data.types.get(variable_id).is_some_and(|t| !types.contains(t)) {
                        node.data.types.remove(variable_id);
                    }
                }
            }
        }
        self.changed();
        if types.is_some() {
            self.drop_unfit_links_to_instances(definition_ref, &handles);
        }
    }

    fn handles_of_variable(&self, definition_ref: &str, variable_id: &str) -> Vec<String> {
        self.project.document(definition_ref).map_or_else(Vec::new, |d| {
            d.inputs
                .iter()
                .chain(&d.outputs)
                .filter(|h| h.ty.variable() == Some(variable_id))
                .map(|h| h.id.clone())
                .collect()
        })
    }

    /// Remove a variable; the sockets using it take its first type.
    pub fn remove_type_variable(&mut self, definition_ref: &str, variable_id: &str) {
        let Some(def) = self.user_definition(definition_ref) else { return };
        let Some(variable) = def.type_variables.iter().find(|v| v.id == variable_id).cloned() else {
            return;
        };
        let fixed = variable.types.first().copied().unwrap_or(WgslType::F32);
        for h in def.inputs.iter_mut().chain(&mut def.outputs) {
            if h.ty.variable() == Some(variable_id) {
                h.ty = HandleType::Fixed(fixed);
            }
        }
        for item in def.nodes.iter_mut().filter_map(|n| n.data.items.as_mut()).flatten() {
            if item.ty.variable() == Some(variable_id) {
                item.ty = HandleType::Fixed(fixed);
            }
        }
        def.type_variables.retain(|v| v.id != variable_id);
        fit_type_owners(def);
        for doc in self.project.definitions.values_mut() {
            for node in doc.nodes.iter_mut().filter(|n| n.definition_ref() == Some(definition_ref)) {
                node.data.types.remove(variable_id);
            }
        }
        self.changed();
    }

    /// Pick the type a group node gives one of its definition's variables, or
    /// let links infer it with `None`.
    pub fn set_variable_type(&mut self, node_id: &str, variable_id: &str, ty: Option<WgslType>) {
        self.update_node_data(node_id, |data| match ty {
            Some(t) => {
                data.types.insert(variable_id.to_owned(), t);
            }
            None => {
                data.types.remove(variable_id);
            }
        });
    }

    /// Copies the definition of the group node (anywhere in the project) into
    /// a new, unlocked definition, and points the node at the copy. Returns
    /// the copy's definition ref.
    pub fn deep_clone_group(&mut self, node_id: &str) -> Option<String> {
        let holder = self.project.definition_ref_of(node_id)?.to_owned();
        let source_ref = self.project.document(&holder)?.node(node_id)?.definition_ref()?.to_owned();
        let source = self.project.document(&source_ref)?.clone();
        let (nodes, edges, _) = clone_subgraph(&source.nodes, &source.edges, Point::default());
        let copy = GraphDocument {
            nodes,
            edges,
            label: Some(self.unique_copy_label(source.label.as_deref())),
            type_variables: source.type_variables,
            inputs: source.inputs,
            outputs: source.outputs,
            defaults: source.defaults,
            ..GraphDocument::default()
        };
        let reference = new_id();
        self.project.definitions.insert(reference.clone(), copy);
        let target = self.project.definitions.get_mut(&holder)?.node_mut(node_id)?;
        target.group = Some(GroupRef { definition_ref: reference.clone(), locked: false });
        self.changed();
        Some(reference)
    }

    /// "<label> - Copy", then "<label> - Copy 2", ... whichever no library or
    /// user group has yet.
    pub(super) fn unique_copy_label(&self, source: Option<&str>) -> String {
        let Some(source) = source.filter(|s| !s.is_empty()) else {
            return "Group".to_owned();
        };
        let used: HashSet<&str> = library::entries()
            .iter()
            .map(|e| e.label.as_str())
            .chain(self.project.definitions.values().filter_map(|d| d.label.as_deref()))
            .collect();
        let base = format!("{source} - Copy");
        if !used.contains(base.as_str()) {
            return base;
        }
        let mut n = 2;
        loop {
            let label = format!("{base} {n}");
            if !used.contains(label.as_str()) {
                return label;
            }
            n += 1;
        }
    }
}
