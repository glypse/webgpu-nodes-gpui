//! Zones: a pair of nodes that bound a region of the graph. The output
//! node owns the item list (`data.items`); the input node points at its
//! partner with `data.paired_output`. Item handle ids are shared by both.
//!
//! A repeat zone loops within one pixel's computation:
//!
//! ```text
//! Repeat Input  -- inputs:  iterations, initial value of every state item
//!               \- outputs: iteration index, current value of every item
//! Repeat Output -- inputs:  next value of every item
//!               \- outputs: final value of every item
//! ```
//!
//! A simulation zone steps once per frame, its state kept per pixel between
//! frames. Which nodes are *inside* a zone is derived from the links, never
//! stored: anything downstream of the zone's input is computed in it.
//!
//! A simulation's items may be textures: WGSL can't carry one through a
//! loop, but a texture of the previous frame is kept between frames.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::graph::{GraphEdge, GraphNode, GraphProject};
use crate::passes::{ItemSizeMode, item_size_handles, size_inputs};
use crate::types::{
    HandleType, NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef, TypeVariable,
    WgslType, ZoneItem, bound_type,
};

pub const REPEAT_INPUT: &str = "repeat-input";
pub const REPEAT_OUTPUT: &str = "repeat-output";
pub const SIMULATION_INPUT: &str = "simulation-input";
pub const SIMULATION_OUTPUT: &str = "simulation-output";
pub const PASS_REPEAT_INPUT: &str = "pass-repeat-input";
pub const PASS_REPEAT_OUTPUT: &str = "pass-repeat-output";

/// Repeat Input's iteration count input.
pub const ITERATIONS_HANDLE: &str = "iterations";
/// Repeat Input's current-iteration-index output.
pub const ITERATION_HANDLE: &str = "iteration";
/// Simulation Input's output: seconds since the previous frame's step.
pub const DELTA_TIME_HANDLE: &str = "delta-time";
/// Simulation Output's input: keep the previous frame's state.
pub const SKIP_HANDLE: &str = "skip";
/// Virtual handle: dropping a link on it creates a new state item.
pub const NEW_ITEM_HANDLE: &str = "__new_item__";
pub const DEFAULT_ITERATIONS: i32 = 1;
pub const DEFAULT_MAX_ITERATIONS: u32 = 1024;
pub use crate::graph::MAX_ITERATIONS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZoneKind {
    Repeat,
    Simulation,
    /// Repeats passes, not a pixel's computation: stepped like a simulation,
    /// as many times as it's told within one frame, its items kept in
    /// textures between iterations.
    Passes,
}

impl ZoneKind {
    pub fn boundaries(self) -> (&'static str, &'static str) {
        match self {
            ZoneKind::Repeat => (REPEAT_INPUT, REPEAT_OUTPUT),
            ZoneKind::Simulation => (SIMULATION_INPUT, SIMULATION_OUTPUT),
            ZoneKind::Passes => (PASS_REPEAT_INPUT, PASS_REPEAT_OUTPUT),
        }
    }
}

pub fn is_zone_node_type(ty: &str) -> bool {
    zone_kind_of(ty).is_some()
}

/// The kind of zone a node type bounds, if it's a zone node.
pub fn zone_kind_of(ty: &str) -> Option<ZoneKind> {
    match ty {
        REPEAT_INPUT | REPEAT_OUTPUT => Some(ZoneKind::Repeat),
        SIMULATION_INPUT | SIMULATION_OUTPUT => Some(ZoneKind::Simulation),
        PASS_REPEAT_INPUT | PASS_REPEAT_OUTPUT => Some(ZoneKind::Passes),
        _ => None,
    }
}

pub fn is_zone_input(ty: &str) -> bool {
    ty == REPEAT_INPUT || ty == SIMULATION_INPUT || ty == PASS_REPEAT_INPUT
}

pub fn is_zone_output(ty: &str) -> bool {
    ty == REPEAT_OUTPUT || ty == SIMULATION_OUTPUT || ty == PASS_REPEAT_OUTPUT
}

/// The id of the zone output a zone input is paired with.
pub fn paired_output_id(node: &GraphNode) -> Option<&str> {
    if is_zone_input(&node.node_type) {
        node.data.paired_output.as_deref()
    } else {
        None
    }
}

/// The zone input paired with the zone output `output_id`, among `nodes`.
pub fn paired_input<'a>(nodes: &'a [GraphNode], output_id: &str) -> Option<&'a GraphNode> {
    nodes
        .iter()
        .find(|n| is_zone_input(&n.node_type) && n.data.paired_output.as_deref() == Some(output_id))
}

/// Whether a zone keeps its items in textures: a repeat zone's live in one pixel's computation.
pub fn keeps_textures(kind: Option<ZoneKind>) -> bool {
    matches!(kind, Some(ZoneKind::Simulation | ZoneKind::Passes))
}

/// Whether a zone of `kind` can carry `item`: only a simulation's may be a texture.
pub fn carries_item(kind: Option<ZoneKind>, item: &ZoneItem) -> bool {
    keeps_textures(kind) || item.ty.fixed().is_none_or(|t| !t.is_texture())
}

/// The state items declared on a zone output node. A repeat zone has none
/// of a texture's: those a file gives it are left out.
pub fn zone_items(output: Option<&GraphNode>) -> Vec<ZoneItem> {
    let kind = output.and_then(|n| zone_kind_of(&n.node_type));
    let items = output
        .and_then(|n| n.data.items.clone())
        .unwrap_or_default();
    items
        .into_iter()
        .filter(|i| carries_item(kind, i))
        .collect()
}

/// The safety cap applied to a zone's iteration count.
pub fn zone_max_iterations(input: &GraphNode) -> u32 {
    input
        .data
        .max_iterations
        .unwrap_or(DEFAULT_MAX_ITERATIONS)
        .min(MAX_ITERATIONS)
}

pub fn is_inferred_item(item: &ZoneItem) -> bool {
    item.ty.is_inferred()
}

/// The family of an inferred item's sockets, in the `types` of both zone
/// nodes: the zone input's takes the type of the item's initial value, the
/// zone output's is forced to it.
pub fn inferred_item_family(item_id: &str) -> String {
    format!("inferred:{item_id}")
}

/// An item's input: a variable's items share its family, which takes the type
/// it's bound to; an inferred item's is in a family of its own. A texture
/// item's values are colours, computed per pixel like any other.
fn item_input(item: &ZoneItem, variables: &[TypeVariable]) -> NodeInputDef {
    let (id, label) = (item.id.clone(), item.label.clone());
    match &item.ty {
        HandleType::Fixed(t) => NodeInputDef::new(id, label, &[t.value_type()]),
        HandleType::Inferred { .. } => {
            NodeInputDef::new(id.clone(), label, &WgslType::ALL).family(inferred_item_family(&id))
        }
        HandleType::Variable { variable } => {
            let types = variables
                .iter()
                .find(|v| &v.id == variable)
                .map_or_else(|| vec![WgslType::F32], |v| v.types.clone());
            let mut input = NodeInputDef::new(id, label, &types);
            input.family = Some(variable.clone());
            input
        }
    }
}

fn item_output(item: &ZoneItem, variables: &[TypeVariable]) -> NodeOutputDef {
    NodeOutputDef::new(
        item.id.clone(),
        item.label.clone(),
        bound_type(&item.ty, variables, &BTreeMap::new()).unwrap_or(WgslType::F32),
    )
}

fn base(ty: &str, label: &str) -> NodeDescriptor {
    let mut d = NodeDescriptor::new(ty, label, NodeCategory::Zone)
        .hidden()
        .static_outputs();
    d.wgsl = std::sync::Arc::new(|_| Vec::new());
    d
}

/// The descriptor of a zone node. `variables`: the type variables of the
/// document the zone is in.
pub fn build_zone_descriptor(
    node: &GraphNode,
    project: &GraphProject,
    variables: &[TypeVariable],
) -> NodeDescriptor {
    let output_node = || paired_output_id(node).and_then(|id| project.resolve_node(id));
    match node.node_type.as_str() {
        REPEAT_INPUT => {
            let items = zone_items(output_node());
            let mut inputs = vec![NodeInputDef::new(
                ITERATIONS_HANDLE,
                "Iterations",
                &[WgslType::I32],
            )];
            inputs.extend(items.iter().map(|i| item_input(i, variables)));
            let mut outputs = vec![NodeOutputDef::new(
                ITERATION_HANDLE,
                "Iteration",
                WgslType::I32,
            )];
            outputs.extend(items.iter().map(|i| item_output(i, variables)));
            let mut data = NodeData::with_values([(ITERATIONS_HANDLE, DEFAULT_ITERATIONS)]);
            data.max_iterations = Some(DEFAULT_MAX_ITERATIONS);
            base(REPEAT_INPUT, "Repeat Input")
                .inputs(inputs)
                .outputs(outputs)
                .defaults(data)
        }
        SIMULATION_INPUT => {
            let items = zone_items(output_node());
            let mut outputs = vec![NodeOutputDef::new(
                DELTA_TIME_HANDLE,
                "Delta Time",
                WgslType::F32,
            )];
            outputs.extend(items.iter().map(|i| item_output(i, variables)));
            let mut inputs: Vec<NodeInputDef> =
                items.iter().map(|i| item_input(i, variables)).collect();
            for item in &items {
                let mode = ItemSizeMode::parse(item.size.as_deref());
                inputs.extend(size_inputs(
                    &item_size_handles(&item.id),
                    mode,
                    Some(&item.label),
                ));
            }
            base(SIMULATION_INPUT, "Simulation Input")
                .inputs(inputs)
                .outputs(outputs)
        }
        SIMULATION_OUTPUT => {
            let items = zone_items(Some(node));
            let mut inputs = vec![NodeInputDef::new(SKIP_HANDLE, "Skip", &[WgslType::Bool])];
            inputs.extend(items.iter().map(|i| item_input(i, variables)));
            let mut data = NodeData::with_values([(SKIP_HANDLE, false)]);
            data.items = Some(Vec::new());
            base(SIMULATION_OUTPUT, "Simulation Output")
                .inputs(inputs)
                .outputs(items.iter().map(|i| item_output(i, variables)).collect())
                .defaults(data)
        }
        PASS_REPEAT_INPUT => {
            let items = zone_items(output_node());
            let mut iterations =
                NodeInputDef::new(ITERATIONS_HANDLE, "Iterations", &[WgslType::I32]);
            iterations.frame = true;
            let mut inputs = vec![iterations];
            inputs.extend(items.iter().map(|i| item_input(i, variables)));
            for item in &items {
                let mode = ItemSizeMode::parse(item.size.as_deref());
                inputs.extend(size_inputs(
                    &item_size_handles(&item.id),
                    mode,
                    Some(&item.label),
                ));
            }
            let mut outputs = vec![NodeOutputDef::new(
                ITERATION_HANDLE,
                "Iteration",
                WgslType::I32,
            )];
            outputs.extend(items.iter().map(|i| item_output(i, variables)));
            let data = NodeData::with_values([(ITERATIONS_HANDLE, DEFAULT_ITERATIONS)]);
            base(PASS_REPEAT_INPUT, "Repeat Passes Input")
                .inputs(inputs)
                .outputs(outputs)
                .defaults(data)
        }
        PASS_REPEAT_OUTPUT => {
            let items = zone_items(Some(node));
            let data = NodeData {
                items: Some(Vec::new()),
                ..Default::default()
            };
            base(PASS_REPEAT_OUTPUT, "Repeat Passes Output")
                .inputs(items.iter().map(|i| item_input(i, variables)).collect())
                .outputs(items.iter().map(|i| item_output(i, variables)).collect())
                .defaults(data)
        }
        _ => {
            let items = zone_items(Some(node));
            let data = NodeData {
                items: Some(Vec::new()),
                ..Default::default()
            };
            base(REPEAT_OUTPUT, "Repeat Output")
                .inputs(items.iter().map(|i| item_input(i, variables)).collect())
                .outputs(items.iter().map(|i| item_output(i, variables)).collect())
                .defaults(data)
        }
    }
}

// ── Analysis ─────────────────────────────────────────────────────────────────

/// Nodes that can never live inside a zone: the document's sinks, and a
/// simulation's start, which a loop or another frame's step can't run.
fn root_only(node: &GraphNode, simulating: &HashSet<String>) -> bool {
    matches!(
        node.node_type.as_str(),
        "frag-output" | "group-output" | SIMULATION_INPUT
    ) || node
        .definition_ref()
        .is_some_and(|r| simulating.contains(r))
}

const SIMULATION_REASON: &str = "A simulation steps once a frame: it can't be inside a zone.";
const REPEAT_REASON: &str =
    "Passes can't be repeated inside a repeat zone, which is one pixel's computation.";

#[derive(Clone, Debug, Default)]
pub struct ZoneOptions {
    /// Group nodes computing a simulation zone, themselves or through groups
    /// they nest: they can't be in a zone either.
    pub simulating: HashSet<String>,
    /// Group nodes computing a passes zone, themselves or through groups
    /// they nest: they can't be in a repeat zone, which is one pixel's computation.
    pub looping: HashSet<String>,
}

/// What a project's zones need to know about the groups nesting them.
pub fn zone_options(project: &GraphProject) -> ZoneOptions {
    ZoneOptions {
        simulating: definitions_holding(project, SIMULATION_INPUT),
        looping: definitions_holding(project, PASS_REPEAT_INPUT),
    }
}

/// The definitions having a node of `node_type`, themselves or through the groups they nest.
fn definitions_holding(project: &GraphProject, node_type: &str) -> HashSet<String> {
    let mut holding: HashSet<String> = project
        .definitions
        .iter()
        .filter(|(_, d)| d.nodes.iter().any(|n| n.is(node_type)))
        .map(|(k, _)| k.clone())
        .collect();
    loop {
        let before = holding.len();
        for (key, doc) in &project.definitions {
            if !holding.contains(key) && doc.group_refs().any(|r| holding.contains(r)) {
                holding.insert(key.clone());
            }
        }
        if holding.len() == before {
            return holding;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Zone {
    /// The zone's id: its input node's id.
    pub id: String,
    pub kind: ZoneKind,
    pub input_id: String,
    pub output_id: String,
    /// Id of the directly enclosing zone, or `None` at the document's top level.
    pub parent: Option<String>,
    /// 0 for top-level zones.
    pub depth: usize,
    /// Every node inside the zone (nested zones included), boundaries excluded.
    pub members: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ZoneAnalysis {
    pub zones: BTreeMap<String, Zone>,
    /// Zone id of every paired zone input / output node.
    pub zone_of_boundary: HashMap<String, String>,
    /// Innermost zone containing each node. Boundary nodes map to their own zone.
    pub node_zone: HashMap<String, Option<String>>,
    /// Links breaking zone rules: drawn red, treated as disconnected by codegen.
    pub invalid_edges: HashSet<String>,
    /// Why some of them do, by link, when it's a nesting rule.
    pub reasons: HashMap<String, &'static str>,
    /// Zone nodes missing their partner.
    pub unpaired: HashSet<String>,
}

impl ZoneAnalysis {
    pub fn zone_of(&self, node: &str) -> Option<&Zone> {
        self.zones.get(self.zone_of_boundary.get(node)?)
    }
}

/// Whether a link `source -> target` would close a cycle, accounting for the
/// implicit zone input -> zone output dependency of every zone.
pub fn would_create_cycle(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    source: &str,
    target: &str,
) -> bool {
    if source == target {
        return true;
    }
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        adjacency.entry(&e.source).or_default().push(&e.target);
    }
    let outputs: HashSet<&str> = nodes
        .iter()
        .filter(|n| is_zone_output(&n.node_type))
        .map(|n| n.id.as_str())
        .collect();
    for n in nodes {
        if let Some(out) = paired_output_id(n).filter(|o| outputs.contains(o)) {
            adjacency.entry(&n.id).or_default().push(out);
        }
    }
    let mut seen = HashSet::from([target]);
    let mut stack = vec![target];
    while let Some(current) = stack.pop() {
        if current == source {
            return true;
        }
        for next in adjacency.get(current).into_iter().flatten() {
            if seen.insert(next) {
                stack.push(next);
            }
        }
    }
    false
}

/// Topological order of `ids`; nodes caught in a cycle are appended last.
fn topo_order(ids: &[&str], edges: &[(&str, &str)]) -> Vec<String> {
    let mut in_degree: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for (s, t) in edges {
        if !in_degree.contains_key(s) || !in_degree.contains_key(t) {
            continue;
        }
        adjacency.entry(s).or_default().push(t);
        *in_degree.get_mut(t).expect("checked") += 1;
    }
    let mut queue: std::collections::VecDeque<&str> = ids
        .iter()
        .copied()
        .filter(|id| in_degree[id] == 0)
        .collect();
    let mut order: Vec<String> = Vec::new();
    while let Some(id) = queue.pop_front() {
        order.push(id.to_owned());
        for next in adjacency.get(id).into_iter().flatten() {
            let d = in_degree.get_mut(next).expect("known");
            *d -= 1;
            if *d == 0 {
                queue.push_back(next);
            }
        }
    }
    let placed: HashSet<&str> = order.iter().map(String::as_str).collect();
    let rest: Vec<String> = ids
        .iter()
        .filter(|id| !placed.contains(**id))
        .map(|id| (*id).to_owned())
        .collect();
    order.extend(rest);
    order
}

/// Work out zone membership, nesting and invalid links for one document,
/// following these rules:
///
/// - "inside" flags propagate forward along links: a zone input sets its
///   zone's flag; a zone output inherits its input's flags (implicit link),
///   clears its own zone and marks downstream nodes as "after" the zone.
/// - A node can't be both inside and after the same zone, nor inside two
///   unrelated zones, and sinks are always outside, as are simulation zones:
///   the incoming links causing such conflicts are invalid, and the analysis
///   is re-run until it is stable.
/// - Remaining links may only stay in a zone or enter a nested one, never leave.
pub fn analyze_zones(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    options: &ZoneOptions,
) -> ZoneAnalysis {
    let node_by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut zones: BTreeMap<String, Zone> = BTreeMap::new();
    let mut zone_of_boundary: HashMap<String, String> = HashMap::new();
    let mut unpaired: HashSet<String> = HashSet::new();
    let mut refused: HashMap<String, &'static str> = HashMap::new();

    // 1. Pair boundary nodes.
    let mut claimed: HashSet<String> = HashSet::new();
    for node in nodes {
        if !is_zone_input(&node.node_type) {
            continue;
        }
        let kind = zone_kind_of(&node.node_type).unwrap_or(ZoneKind::Repeat);
        let output = paired_output_id(node).and_then(|id| node_by_id.get(id).copied());
        let Some(output) = output.filter(|o| {
            is_zone_output(&o.node_type)
                && zone_kind_of(&o.node_type) == Some(kind)
                && !claimed.contains(&o.id)
        }) else {
            unpaired.insert(node.id.clone());
            continue;
        };
        claimed.insert(output.id.clone());
        zones.insert(
            node.id.clone(),
            Zone {
                id: node.id.clone(),
                kind,
                input_id: node.id.clone(),
                output_id: output.id.clone(),
                parent: None,
                depth: 0,
                members: BTreeSet::new(),
            },
        );
        zone_of_boundary.insert(node.id.clone(), node.id.clone());
        zone_of_boundary.insert(output.id.clone(), node.id.clone());
    }
    for node in nodes {
        if is_zone_output(&node.node_type) && !claimed.contains(&node.id) {
            unpaired.insert(node.id.clone());
        }
    }
    let mut invalid: HashSet<String> = edges
        .iter()
        .filter(|e| unpaired.contains(&e.source) || unpaired.contains(&e.target))
        .map(|e| e.id.clone())
        .collect();

    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let implicit: Vec<(&str, &str)> = zones
        .values()
        .map(|z| (z.input_id.as_str(), z.output_id.as_str()))
        .collect();

    type Flags = HashMap<String, BTreeSet<String>>;
    let mut inside: Flags = HashMap::new();
    let mut ancestors: HashMap<String, BTreeSet<String>> = HashMap::new();

    fn enclosing(zones: &BTreeMap<String, Zone>, inside: &Flags, zone: &str) -> BTreeSet<String> {
        zones
            .get(zone)
            .and_then(|z| inside.get(&z.output_id))
            .cloned()
            .unwrap_or_default()
    }
    fn innermost(
        zones: &BTreeMap<String, Zone>,
        inside: &Flags,
        flags: &BTreeSet<String>,
    ) -> Option<String> {
        let mut best: Option<(&String, usize)> = None;
        for z in flags {
            let depth = enclosing(zones, inside, z).len();
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((z, depth));
            }
        }
        best.map(|(z, _)| z.clone())
    }

    // 2. Propagate flags until no new link has to be invalidated.
    for _ in 0..=edges.len() {
        let valid: Vec<&GraphEdge> = edges.iter().filter(|e| !invalid.contains(&e.id)).collect();
        let mut incoming: HashMap<&str, Vec<&GraphEdge>> = HashMap::new();
        for e in &valid {
            incoming.entry(&e.target).or_default().push(e);
        }
        inside = HashMap::new();
        let mut after: Flags = HashMap::new();
        let mut newly_invalid: Vec<String> = Vec::new();
        let links: Vec<(&str, &str)> = valid
            .iter()
            .map(|e| (e.source.as_str(), e.target.as_str()))
            .chain(implicit.iter().copied())
            .collect();
        for id in topo_order(&ids, &links) {
            let node = node_by_id.get(id.as_str()).copied();
            let mut ins: BTreeSet<String> = BTreeSet::new();
            let mut aft: BTreeSet<String> = BTreeSet::new();
            for e in incoming.get(id.as_str()).into_iter().flatten() {
                ins.extend(inside.get(&e.source).into_iter().flatten().cloned());
                aft.extend(after.get(&e.source).into_iter().flatten().cloned());
            }
            let zone = zone_of_boundary.get(&id).and_then(|z| zones.get(z));
            if let Some(node) = node.filter(|n| root_only(n, &options.simulating)) {
                if !ins.is_empty()
                    && !matches!(node.node_type.as_str(), "frag-output" | "group-output")
                {
                    refused.insert(id.clone(), SIMULATION_REASON);
                }
                ins.clear();
            }
            let repeats_passes = node.is_some_and(|n| {
                n.is(PASS_REPEAT_INPUT)
                    || n.definition_ref()
                        .is_some_and(|r| options.looping.contains(r))
            });
            if repeats_passes {
                let in_repeat: Vec<String> = ins
                    .iter()
                    .filter(|z| {
                        zones
                            .get(*z)
                            .is_some_and(|zone| zone.kind == ZoneKind::Repeat)
                    })
                    .cloned()
                    .collect();
                for z in in_repeat {
                    ins.remove(&z);
                    refused.insert(id.clone(), REPEAT_REASON);
                }
            }
            if let Some(zone) = zone {
                if id == zone.input_id {
                    ins.insert(zone.id.clone());
                }
                if id == zone.output_id {
                    ins.extend(inside.get(&zone.input_id).into_iter().flatten().cloned());
                    ins.remove(&zone.id);
                    aft.insert(zone.id.clone());
                }
            }
            // Inside and after the same zone: the links bringing the inside
            // flag in are the culprits.
            for z in &ins {
                if !aft.contains(z) {
                    continue;
                }
                for e in incoming.get(id.as_str()).into_iter().flatten() {
                    if inside.get(&e.source).is_some_and(|s| s.contains(z)) {
                        newly_invalid.push(e.id.clone());
                    }
                }
            }
            inside.insert(id.clone(), ins);
            after.insert(id, aft);
        }
        // Inside two unrelated zones: keep the innermost chain, drop links
        // bringing in foreign zones.
        ancestors = zones
            .keys()
            .map(|z| (z.clone(), enclosing(&zones, &inside, z)))
            .collect();
        for id in &ids {
            let own = zone_of_boundary.get(*id);
            let mut flags = inside.get(*id).cloned().unwrap_or_default();
            if let Some(own) = own {
                flags.remove(own);
            }
            let Some(keep) = innermost(&zones, &inside, &flags) else {
                continue;
            };
            let mut chain = ancestors.get(&keep).cloned().unwrap_or_default();
            chain.insert(keep);
            let foreign: Vec<&String> = flags.iter().filter(|z| !chain.contains(*z)).collect();
            if foreign.is_empty() {
                continue;
            }
            let mut suspects: Vec<&GraphEdge> =
                incoming.get(*id).into_iter().flatten().copied().collect();
            if let Some(zone) = own
                .and_then(|z| zones.get(z))
                .filter(|z| z.output_id == *id)
            {
                suspects.extend(
                    incoming
                        .get(zone.input_id.as_str())
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
            for e in suspects {
                if foreign
                    .iter()
                    .any(|z| inside.get(&e.source).is_some_and(|s| s.contains(*z)))
                {
                    newly_invalid.push(e.id.clone());
                }
            }
        }
        if newly_invalid.is_empty() {
            break;
        }
        invalid.extend(newly_invalid);
    }

    // 3. Nesting, depth and membership.
    let parents: Vec<(String, Option<String>, usize)> = zones
        .keys()
        .map(|z| {
            let outer = ancestors.get(z).cloned().unwrap_or_default();
            (z.clone(), innermost(&zones, &inside, &outer), outer.len())
        })
        .collect();
    for (id, parent, depth) in parents {
        let zone = zones.get_mut(&id).expect("listed");
        zone.parent = parent;
        zone.depth = depth;
    }
    let mut node_zone: HashMap<String, Option<String>> = HashMap::new();
    let mut memberships: Vec<(String, String)> = Vec::new();
    for id in &ids {
        if let Some(own) = zone_of_boundary.get(*id) {
            node_zone.insert((*id).to_owned(), Some(own.clone()));
            continue;
        }
        let zone = innermost(
            &zones,
            &inside,
            &inside.get(*id).cloned().unwrap_or_default(),
        );
        node_zone.insert((*id).to_owned(), zone.clone());
        let mut z = zone;
        while let Some(current) = z {
            memberships.push((current.clone(), (*id).to_owned()));
            z = zones.get(&current).and_then(|zone| zone.parent.clone());
        }
    }
    for (zone, member) in memberships {
        zones.get_mut(&zone).expect("known").members.insert(member);
    }
    let nested: Vec<(String, String, String)> = zones
        .values()
        .map(|z| (z.id.clone(), z.input_id.clone(), z.output_id.clone()))
        .collect();
    for (id, input, output) in nested {
        let mut z = zones.get(&id).and_then(|z| z.parent.clone());
        while let Some(current) = z {
            let outer = zones.get_mut(&current).expect("known");
            outer.members.insert(input.clone());
            outer.members.insert(output.clone());
            z = outer.parent.clone();
        }
    }

    // 4. Links may stay in a zone or enter nested zones, never leave one.
    let contains = |outer: &str, inner: Option<&String>| -> bool {
        let mut z = inner.cloned();
        while let Some(current) = z {
            if current == outer {
                return true;
            }
            z = zones.get(&current).and_then(|zone| zone.parent.clone());
        }
        false
    };
    let side_zone = |id: &str, boundary_is: fn(&Zone) -> &str| -> Option<String> {
        let zone = zone_of_boundary.get(id).and_then(|z| zones.get(z));
        match zone {
            Some(zone) if id == boundary_is(zone) => zone.parent.clone(),
            _ => node_zone.get(id).cloned().flatten(),
        }
    };
    let mut leaving: Vec<String> = Vec::new();
    for e in edges {
        if invalid.contains(&e.id) {
            continue;
        }
        let from = side_zone(&e.source, |z| &z.output_id);
        let to = side_zone(&e.target, |z| &z.input_id);
        if let Some(from) = from
            && !contains(&from, to.as_ref())
        {
            leaving.push(e.id.clone());
        }
    }
    invalid.extend(leaving);

    let reasons = invalid
        .iter()
        .filter_map(|id| {
            let edge = edges.iter().find(|e| &e.id == id)?;
            Some((id.clone(), *refused.get(&edge.target)?))
        })
        .collect();
    ZoneAnalysis {
        zones,
        zone_of_boundary,
        node_zone,
        invalid_edges: invalid,
        reasons,
        unpaired,
    }
}
