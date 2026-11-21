//! WGSL generation: the root document becomes `frag`, each group a function
//! returning a struct of its outputs, each repeat zone a `for` loop.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use thiserror::Error;

use super::helpers::HelperRegistry;
use super::literals::{
    UniformValue, format_wgsl_literal, ident_suffix, uniform_name, uniform_value,
};
use super::program::{
    FRAGMENT_VAR, FRAME_VALUES_VAR, FRAME_VAR, FrameValueUse, UniformScalar, SIMULATION_DELTA_FIELD, SamplerBinding, TextureBinding, UNIFORMS_VAR,
};
use super::target::{Probe, ProbeKind, ShaderTarget, SimulationStage};
use super::texture_read::{ReadEnv, ReadOptions, read_texture, texel_as_color};
use crate::enums::{EnumDocument, group_constants};
use crate::frame::{FrameCompiler, frame_value_field};
use crate::graph::{
    FRAG_OUTPUT, GROUP_INPUT, GROUP_OUTPUT, GraphDocument, GraphEdge, GraphNode, GraphProject,
    NODE_GROUP, REROUTE, ROOT_ID,
};
use crate::nodes::{RENDER_TEXTURE, SAMPLE_TEXTURE, TEXTURE_SIZE, resolve_descriptor, texture_coordinates};
use crate::passes::{
    CANVAS_SIZE, SimulationRef, SimulationState, StateItem, StateRead, TextureFormat,
    TextureSize, TextureSource, TextureSources, pack_state, same_size, state_source, unpack_state,
};
use crate::types::{
    CtxEnv, GroupHandleDef, HelperName, InputValue, NodeDescriptor, ResolvedNodeTypes,
    SamplerAddress, SamplerFilter, ScalarKind, TypeBindings, TypeResolver, WgslType, ZoneItem, bound_type,
    complete_bindings, convert_var, input_control, make_type, unavailable_edges,
};
use crate::zones::{
    DEFAULT_ITERATIONS, DELTA_TIME_HANDLE, ITERATIONS_HANDLE, ITERATION_HANDLE, REPEAT_INPUT,
    REPEAT_OUTPUT, SIMULATION_INPUT, SIMULATION_OUTPUT, SKIP_HANDLE, Zone, ZoneAnalysis, ZoneKind,
    analyze_zones, paired_output_id, zone_items, zone_max_iterations, zone_options,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodegenError {
    #[error("Cycle detected in the node graph - cannot generate shader")]
    Cycle,
    #[error("{0}")]
    Unsupported(String),
}

/// What generation reads: the project and its type resolver.
#[derive(Clone, Copy)]
pub struct CodegenGraph<'a> {
    pub project: &'a GraphProject,
    pub resolver: &'a TypeResolver,
}

#[derive(Clone, Debug, Default)]
pub struct ShaderOptions {
    /// The definitions (`ROOT_ID` for the root) whose unlinked input values
    /// are read from uniforms rather than written into the shader: editing
    /// them then only changes the uniforms. Values of other documents stay literals.
    pub uniform_scopes: Vec<String>,
    /// Read the root document's frame values from a block the runtime writes
    /// (computed on the CPU, once a frame) instead of computing them per pixel.
    pub hoist_frame_values: bool,
}

/// A generated fragment (defining `frag`) and the uniform values it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderSource {
    pub fragment: String,
    /// In the order the shader declares them.
    pub uniforms: Vec<(String, UniformValue)>,
    /// The lines of `fragment` each node's statements are on, by node id.
    pub line_map: BTreeMap<String, Range<usize>>,
    /// The textures the shader reads, in binding order.
    pub textures: Vec<TextureBinding>,
    pub samplers: Vec<SamplerBinding>,
    /// The types of the colour attachments it draws into; a colour without.
    pub targets: Option<Vec<WgslType>>,
    /// The simulation it computes something of (by `SimulationRef::id`), if any.
    pub simulation: Option<String>,
    /// The frame values it reads from the frame values block.
    pub frame_values: Vec<FrameValueUse>,
}

// ── Graph walking ────────────────────────────────────────────────────────────

/// Walk backwards from the `seed_type` nodes (and `seeds`) through edges to
/// find every node contributing to them. A zone output implicitly depends on
/// its input, but a simulation's that `computes` says is read rather than
/// computed (from the textures keeping its state).
fn find_reachable(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    seed_type: &str,
    seeds: &HashSet<String>,
    computes: &dyn Fn(&GraphNode) -> bool,
) -> HashSet<String> {
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        reverse.entry(&e.target).or_default().push(&e.source);
    }
    for n in nodes {
        if let Some(output) = paired_output_id(n).filter(|_| computes(n)) {
            reverse.entry(output).or_default().push(&n.id);
        }
    }
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for n in nodes.iter().filter(|n| n.is(seed_type) || seeds.contains(&n.id)) {
        reachable.insert(n.id.clone());
        queue.push_back(&n.id);
    }
    while let Some(current) = queue.pop_front() {
        for source in reverse.get(current).into_iter().flatten() {
            if reachable.insert((*source).to_owned()) {
                queue.push_back(source);
            }
        }
    }
    reachable
}

/// Topological sort of node ids; fails on a cycle.
fn topological_sort(ids: &[String], edges: &[(String, String)]) -> Result<Vec<String>, CodegenError> {
    let mut adjacency: HashMap<&str, Vec<&str>> = ids.iter().map(|id| (id.as_str(), vec![])).collect();
    let mut in_degree: HashMap<&str, usize> = ids.iter().map(|id| (id.as_str(), 0)).collect();
    for (s, t) in edges {
        if !adjacency.contains_key(s.as_str()) || !in_degree.contains_key(t.as_str()) {
            continue;
        }
        adjacency.get_mut(s.as_str()).expect("checked").push(t);
        *in_degree.get_mut(t.as_str()).expect("checked") += 1;
    }
    let mut queue: VecDeque<&str> =
        ids.iter().map(String::as_str).filter(|id| in_degree[id] == 0).collect();
    let mut sorted = Vec::with_capacity(ids.len());
    while let Some(id) = queue.pop_front() {
        sorted.push(id.to_owned());
        for next in adjacency[id].clone() {
            let d = in_degree.get_mut(next).expect("known");
            *d -= 1;
            if *d == 0 {
                queue.push_back(next);
            }
        }
    }
    if sorted.len() == ids.len() { Ok(sorted) } else { Err(CodegenError::Cycle) }
}

type LiveItems = BTreeMap<String, BTreeSet<String>>;

struct Needed {
    edges: Vec<GraphEdge>,
    reachable: HashSet<String>,
    live_items: LiveItems,
}

/// The edges needed to compute the `seed_type` nodes, and the state items of
/// each repeat zone (by Repeat Output id) they need. An item is needed when
/// its final value is read, or when its current value is read inside the loop
/// by something needed: others are left out, with what only they used. A
/// simulation's items are all computed by the passes computing its state.
/// `read` holds zone items read without a link, by Repeat Output id.
fn needed_edges(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    seed_type: &str,
    seeds: &HashSet<String>,
    read: &LiveItems,
    computes: &dyn Fn(&GraphNode) -> bool,
) -> Needed {
    let by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut live_items: LiveItems = read.clone();
    let item_owner = |node_id: &str, handle: &str| -> Option<String> {
        let node = by_id.get(node_id)?;
        if node.is(REPEAT_OUTPUT) {
            return Some(node_id.to_owned());
        }
        if node.is(REPEAT_INPUT) && handle != ITERATIONS_HANDLE && handle != ITERATION_HANDLE {
            return paired_output_id(node).map(str::to_owned);
        }
        None
    };
    loop {
        let is_live = |live: &LiveItems, node: &str, handle: &str| match item_owner(node, handle) {
            None => true,
            Some(owner) => live.get(&owner).is_some_and(|items| items.contains(handle)),
        };
        let active: Vec<GraphEdge> = edges
            .iter()
            .filter(|e| is_live(&live_items, &e.target, &e.target_handle))
            .cloned()
            .collect();
        let reachable = find_reachable(nodes, &active, seed_type, seeds, computes);
        let mut grew = false;
        for e in &active {
            if !reachable.contains(&e.target) || is_live(&live_items, &e.source, &e.source_handle) {
                continue;
            }
            let Some(owner) = item_owner(&e.source, &e.source_handle) else { continue };
            live_items.entry(owner).or_default().insert(e.source_handle.clone());
            grew = true;
        }
        if !grew {
            let needed = active
                .into_iter()
                .filter(|e| reachable.contains(&e.source) && reachable.contains(&e.target))
                .collect();
            return Needed { edges: needed, reachable, live_items };
        }
    }
}

fn var_name(node_id: &str) -> String {
    format!("v_{}", ident_suffix(node_id))
}

fn sanitize_ident(id: &str) -> String {
    format!("h_{}", ident_suffix(id))
}

// ── Code generation ──────────────────────────────────────────────────────────

/// A group's generated function: its source, name and parameters' types by handle id.
#[derive(Clone)]
struct GroupDecl {
    decl: String,
    fn_name: String,
    param_types: BTreeMap<String, WgslType>,
}

/// An output of a node read from a field of the frame values block.
struct HoistedValue {
    field: String,
    storage: WgslType,
    ty: WgslType,
}

struct ShaderState<'s, 'g> {
    graph: CodegenGraph<'g>,
    /// Compiles the frame values hoisted out of the shader, when the plan evaluates them.
    frame: Option<&'s FrameCompiler<'g>>,
    /// The frame values the shader reads, by field name.
    frame_values: Vec<FrameValueUse>,
    helpers: HelperRegistry,
    sources: &'s TextureSources<'g>,
    target_size: TextureSize,
    /// The textures the shader reads, by `TextureSource::id`, in binding order.
    textures: Vec<TextureBinding>,
    /// The samplers the shader uses, by filter and address.
    samplers: Vec<SamplerBinding>,
    /// How many variants of each group differ by the textures they read.
    texture_variants: HashMap<String, usize>,
    /// Whether each definition reads textures, itself or through groups it nests.
    reads_textures: HashMap<String, bool>,
    /// The simulation the shader computes something of (by `SimulationRef::id`), if any.
    simulation: Option<String>,
    /// Every group's generated function (and, for the first one, its output
    /// struct), keyed by definition plus the absent fallback inputs, the
    /// known enum values, the types and the outputs read, so each variant is
    /// emitted once however many instances use it. In emission order.
    group_decls: Vec<(String, GroupDecl)>,
    group_structs: HashSet<String>,
    uniform_scopes: HashSet<String>,
    uniforms: Vec<(String, UniformValue)>,
    error: Option<CodegenError>,
}

impl ShaderState<'_, '_> {
    fn fail(&mut self, error: CodegenError) {
        self.error.get_or_insert(error);
    }

    /// The module-scope texture reading `source`.
    fn texture_var(&mut self, source: &TextureSource) -> String {
        let id = source.id();
        if let Some(binding) = self.textures.iter().find(|t| t.source.id() == id) {
            return binding.name.clone();
        }
        let name = format!("runtime_texture_{}", self.textures.len());
        self.textures.push(TextureBinding { name: name.clone(), source: source.clone() });
        name
    }

    /// The module-scope sampler filtering and addressing as asked.
    fn sampler_var(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String {
        if let Some(binding) = self.samplers.iter().find(|s| s.filter == filter && s.address == address) {
            return binding.name.clone();
        }
        let name = format!("runtime_sampler_{}_{}", filter.id(), address.id());
        self.samplers.push(SamplerBinding { name: name.clone(), filter, address });
        name
    }

    fn set_uniform(&mut self, name: &str, value: UniformValue) {
        match self.uniforms.iter_mut().find(|(n, _)| n == name) {
            Some((_, existing)) => *existing = value,
            None => self.uniforms.push((name.to_owned(), value)),
        }
    }
}

impl ReadEnv for ShaderState<'_, '_> {
    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String {
        self.sampler_var(filter, address)
    }

    fn define(&mut self, name: &str, source: &str) -> String {
        self.helpers.define(name, source)
    }
}

/// How a document's shader computes a simulation zone in it: reading its
/// state (`Opaque`), or computing its initial values (`Start`), its next
/// state (`Step`), or something in it from its previous state (`Inline`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SimulationMode {
    Opaque,
    Start,
    Step,
    Inline,
}

/// The names of the variables a simulation's previous and current state are in.
struct SimulationNames {
    previous: Vec<String>,
    current: Vec<String>,
    next: Vec<String>,
    skip: String,
}

fn simulation_var_names(zone: &Zone, items: usize) -> SimulationNames {
    let ident = ident_suffix(&zone.id);
    let names = |what: &str| (0..items).map(|k| format!("z_{ident}_{what}{k}")).collect();
    SimulationNames {
        previous: names("p"),
        current: names("c"),
        next: names("next"),
        skip: format!("z_{ident}_skip"),
    }
}

/// A value drawn as a colour: a vec3f opaque, as the fragment output takes
/// it, a scalar grey, a vec2f in red and green.
pub fn field_color(expr: &str, ty: WgslType) -> String {
    let components = ty.components();
    let float = convert_var(expr, ty, make_type(crate::types::ScalarKind::F32, components));
    match components {
        1 => format!("vec4f(vec3f({float}), 1.0)"),
        2 => format!("vec4f({float}, 0.0, 1.0)"),
        3 => format!("vec4f({float}, 1.0)"),
        _ => float,
    }
}

struct BodyEmitter {
    lines: Vec<String>,
    line_map: BTreeMap<String, Range<usize>>,
    doc_nodes: Vec<GraphNode>,
    node_index: HashMap<String, usize>,
    /// The needed, valid links.
    edges: Vec<GraphEdge>,
    incoming: HashMap<String, Vec<GraphEdge>>,
    /// WGSL variables standing for zone boundary outputs, by node id then handle id.
    zone_vars: HashMap<String, HashMap<String, String>>,
    inputs: Vec<GroupHandleDef>,
    absent_inputs: BTreeSet<String>,
    constants: BTreeMap<String, i64>,
    uniforms: bool,
    bindings: TypeBindings,
    zones: ZoneAnalysis,
    reachable: HashSet<String>,
    live_items: LiveItems,
    /// The group nodes the document is computed through, from the root.
    path: Vec<String>,
    probe: Option<Probe>,
    /// How each simulation zone is computed, by zone id.
    simulations: HashMap<String, SimulationMode>,
    /// The output handles of each node something reads.
    read_handles: HashMap<String, BTreeSet<String>>,
    /// The values a simulation target gives, once its zone is emitted.
    simulation_fields: Vec<String>,
    /// The frame values read from the frame block rather than computed, by node id.
    hoisted: HashMap<String, BTreeMap<String, HoistedValue>>,
}

struct ZoneNames {
    index: String,
    count: String,
    state: Vec<String>,
    next: Vec<String>,
}

fn zone_var_names(zone: &Zone, items: usize) -> ZoneNames {
    let ident = ident_suffix(&zone.id);
    ZoneNames {
        index: format!("z_{ident}_i"),
        count: format!("z_{ident}_n"),
        state: (0..items).map(|k| format!("z_{ident}_s{k}")).collect(),
        next: (0..items).map(|k| format!("z_{ident}_next{k}")).collect(),
    }
}

impl BodyEmitter {
    fn node(&self, id: &str) -> Option<&GraphNode> {
        self.node_index.get(id).map(|i| &self.doc_nodes[*i])
    }

    fn incoming(&self, id: &str) -> &[GraphEdge] {
        self.incoming.get(id).map_or(&[], Vec::as_slice)
    }

    fn enum_document(&self) -> EnumDocument<'_> {
        EnumDocument {
            nodes: &self.doc_nodes,
            edges: &self.edges,
            inputs: &self.inputs,
            input_type: None,
        }
    }

    fn constant_of(&self, node: &str, input: &str) -> Option<i64> {
        self.enum_document().constant_at(node, input, &self.constants)
    }

    fn zone_item_types(&self, state: &ShaderState, zone: &Zone) -> HashMap<String, WgslType> {
        let Some(input) = self.node(&zone.input_id) else { return HashMap::new() };
        let resolved = state.graph.resolver.resolve(state.graph.project, input, &self.bindings);
        resolved.resolved_outputs.iter().map(|o| (o.id.clone(), o.ty.value_type())).collect()
    }

    fn is_present(&self, node: &str, input: &str) -> bool {
        let mut edge = self.incoming(node).iter().find(|e| e.target_handle == input);
        // A reroute's value is present where the one it passes on is.
        while let Some(e) = edge {
            if self.node(&e.source).is_some_and(|n| n.is(REROUTE)) {
                edge = self.incoming(&e.source).iter().find(|x| x.target_handle == "in");
            } else {
                break;
            }
        }
        let Some(edge) = edge else { return false };
        !(self.node(&edge.source).is_some_and(|n| n.is(GROUP_INPUT))
            && self.absent_inputs.contains(&edge.source_handle))
    }

    /// The type an edge's source has, a texture's included.
    fn source_socket_type(&self, state: &ShaderState, edge: &GraphEdge) -> WgslType {
        state.graph.resolver.infer_handle_type(
            state.graph.project,
            &edge.source,
            &edge.source_handle,
            &self.bindings,
        )
    }

    /// The type of an edge's source as a value; a texture's is its colour's, sampled at the pixel.
    fn source_type(&self, state: &ShaderState, edge: &GraphEdge) -> WgslType {
        self.source_socket_type(state, edge).value_type()
    }

    /// A texture linked to a value: its colour at the pixel, stretched over the target.
    fn sample_at_pixel(&self, state: &mut ShaderState, source: &TextureSource) -> String {
        let texture = state.texture_var(source);
        let format = source.format();
        // Made for this pass' size, proved while compiling: its pixel is read exactly.
        if source.size().is_some_and(|size| same_size(size, &state.target_size)) {
            return texel_as_color(
                &format!("textureLoad({texture}, vec2i({FRAGMENT_VAR}.position), 0)"),
                format,
            );
        }
        let coords = texture_coordinates(&format!("{FRAGMENT_VAR}.uv"));
        read_texture(state, &texture, format, &coords, ReadOptions::default())
    }

    fn source_expr(&self, state: &mut ShaderState, edge: &GraphEdge) -> String {
        if self.source_socket_type(state, edge).is_texture() {
            let source =
                state.sources.output(&self.path, &edge.source, &edge.source_handle).source();
            return self.sample_at_pixel(state, &source);
        }
        let Some(source) = self.node(&edge.source) else { return var_name(&edge.source) };
        if source.is(GROUP_INPUT) {
            return match self.constants.get(&edge.source_handle) {
                Some(constant) => format_wgsl_literal(
                    self.source_type(state, edge),
                    Some(&InputValue::from(*constant as f64)),
                ),
                None => sanitize_ident(&edge.source_handle),
            };
        }
        if let Some(var) = self.zone_vars.get(&edge.source).and_then(|m| m.get(&edge.source_handle)) {
            return var.clone();
        }
        let base = var_name(&edge.source);
        if source.is(NODE_GROUP) {
            return format!("{base}.{}", sanitize_ident(&edge.source_handle));
        }
        if resolve_descriptor(source, state.graph.project).outputs.len() > 1 {
            return format!("{base}_{}", ident_suffix(&edge.source_handle));
        }
        base
    }

    fn converted_source(&self, state: &mut ShaderState, edge: &GraphEdge, target: WgslType) -> String {
        let expr = self.source_expr(state, edge);
        convert_var(&expr, self.source_type(state, edge), target)
    }

    /// The outputs of a node something reads, the probe's included.
    fn read_outputs(&self, node: &str) -> BTreeSet<String> {
        let mut read = self.read_handles.get(node).cloned().unwrap_or_default();
        if let Some(Probe { path, kind: ProbeKind::Socket { node: n, handle } }) = &self.probe
            && path.is_empty()
            && n == node
        {
            read.insert(handle.clone());
        }
        read
    }

    /// What a Texture socket of a node reads (an input) or gives (an output) here.
    fn texture_source_of(
        &self,
        state: &ShaderState,
        node: &GraphNode,
        resolved: &ResolvedNodeTypes,
        id: &str,
    ) -> TextureSource {
        let is_input = resolved.resolved_inputs.iter().any(|i| i.id == id);
        if is_input {
            state.sources.input(&self.path, &node.id, id).source()
        } else {
            state.sources.output(&self.path, &node.id, id).source()
        }
    }

    /// The values of the probe (see `Probe`), from where they're computed.
    fn probe_fields(&self, state: &mut ShaderState) -> Vec<String> {
        let Some(probe) = &self.probe else { return Vec::new() };
        if let Some(inner) = probe.path.first() {
            return (0..probe.types().len())
                .map(|k| format!("{}.{}", var_name(inner), Probe::field(k)))
                .collect();
        }
        match &probe.kind {
            ProbeKind::Simulation { .. } => self.simulation_fields.clone(),
            ProbeKind::Textures { node, items } => {
                let Some(node) = self.node(node).cloned() else { return Vec::new() };
                items
                    .iter()
                    .map(|i| {
                        let value = self.input_expr(state, &node, &i.item, i.ty, None);
                        // Floats are colours, as any value rendered to a texture is; integers are kept as they are.
                        if i.format.scalar() == crate::types::ScalarKind::F32 {
                            field_color(&value, i.ty)
                        } else {
                            pack_state(&value, i.ty, i.format)
                        }
                    })
                    .collect()
            }
            ProbeKind::Socket { node, handle } => {
                let edge = GraphEdge {
                    id: String::new(),
                    source: node.clone(),
                    source_handle: handle.clone(),
                    target: String::new(),
                    target_handle: String::new(),
                };
                let expr = self.source_expr(state, &edge);
                vec![field_color(&expr, self.source_type(state, &edge))]
            }
        }
    }

    /// The value fed to `node`'s input `input`, as an expression of type
    /// `target`: the linked source (converted) or the node's stored literal.
    fn input_expr(
        &self,
        state: &mut ShaderState,
        node: &GraphNode,
        input: &str,
        target: WgslType,
        fallback: Option<&InputValue>,
    ) -> String {
        if let Some(edge) = self.incoming(&node.id).iter().find(|e| e.target_handle == input) {
            return self.converted_source(state, edge, target);
        }
        let value = node.data.values.get(input).or(fallback);
        if self.uniforms {
            let descriptor = resolve_descriptor(node, state.graph.project);
            let uniform = descriptor
                .input(input)
                .is_none_or(|i| input_control(i, target).is_uniform());
            if uniform {
                return Self::uniform_expr(state, &uniform_name(&node.id, input), target, value);
            }
        }
        format_wgsl_literal(target, value)
    }

    fn param_expr(&self, state: &mut ShaderState, node: &GraphNode, key: &str, value: &[f64]) -> String {
        let ty = make_type(crate::types::ScalarKind::F32, value.len());
        let stored =
            if value.len() == 1 { InputValue::from(value[0]) } else { InputValue::vector(value) };
        if self.uniforms {
            return Self::uniform_expr(state, &uniform_name(&node.id, key), ty, Some(&stored));
        }
        format_wgsl_literal(ty, Some(&stored))
    }

    /// Declare a uniform holding `value`, and read it as `target`.
    fn uniform_expr(
        state: &mut ShaderState,
        name: &str,
        target: WgslType,
        value: Option<&InputValue>,
    ) -> String {
        state.set_uniform(name, uniform_value(target, value));
        let source = make_type(crate::types::ScalarKind::F32, target.components());
        convert_var(&format!("{UNIFORMS_VAR}.{name}"), source, target)
    }

    fn push(&mut self, node_id: &str, statements: Vec<String>, indent: &str) {
        let start = self.lines.len();
        self.lines.extend(statements.into_iter().map(|l| format!("{indent}{l}")));
        self.line_map.insert(node_id.to_owned(), start..self.lines.len());
    }

    fn emit_node(&mut self, state: &mut ShaderState, node: &GraphNode, indent: &str) {
        let project = state.graph.project;
        if let Some(hoisted) = self.hoisted.get(&node.id) {
            let var = var_name(&node.id);
            let multiple = resolve_descriptor(node, project).outputs.len() > 1;
            let lines = hoisted
                .iter()
                .map(|(handle, HoistedValue { field, storage, ty })| {
                    let name = if multiple { format!("{var}_{}", ident_suffix(handle)) } else { var.clone() };
                    format!("let {name} = {};", convert_var(&format!("{FRAME_VALUES_VAR}.{field}"), *storage, *ty))
                })
                .collect();
            self.push(&node.id, lines, indent);
            return;
        }
        let resolved = state.graph.resolver.resolve(project, node, &self.bindings);
        // A node only textures are read from computes nothing: they're read where they're used.
        let read = self.read_handles.get(&node.id);
        let is_texture = |id: &String| {
            resolved.resolved_outputs.iter().find(|o| &o.id == id).is_some_and(|o| o.ty.is_texture())
        };
        let seed = self.probe.as_ref().is_some_and(|p| p.seed_node() == node.id);
        if !resolved.resolved_outputs.is_empty() && read.is_none_or(|r| r.iter().all(is_texture)) && !seed {
            return;
        }
        if node.is(NODE_GROUP) {
            self.emit_group_call(state, node, indent);
            return;
        }
        let descriptor = resolve_descriptor(node, project);
        let var = var_name(&node.id);
        let output_vars: BTreeMap<String, String> = resolved
            .resolved_outputs
            .iter()
            .map(|o| {
                let name = if descriptor.outputs.len() > 1 {
                    format!("{var}_{}", ident_suffix(&o.id))
                } else {
                    var.clone()
                };
                (o.id.clone(), name)
            })
            .collect();
        let type_of = |id: &str| -> WgslType {
            resolved
                .input_types
                .get(id)
                .copied()
                .unwrap_or_else(|| {
                    resolved.resolved_inputs.iter().find(|i| i.id == id).map_or(WgslType::F32, |i| i.default_type())
                })
                .value_type()
        };
        // Variadic inputs are pre-converted, non-variadic ones are raw; a
        // Texture input gives its texture.
        let mut ctx_inputs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for input in &resolved.resolved_inputs {
            let edges: Vec<&GraphEdge> =
                self.incoming(&node.id).iter().filter(|e| e.target_handle == input.id).collect();
            let values = if resolved.input_types.get(&input.id).is_some_and(|t| t.is_texture()) {
                if edges.is_empty() {
                    Vec::new()
                } else {
                    let source = self.texture_source_of(state, node, &resolved, &input.id);
                    vec![state.texture_var(&source)]
                }
            } else if input.variadic {
                edges.iter().map(|e| self.converted_source(state, e, type_of(&input.id))).collect()
            } else {
                edges.first().map(|e| self.source_expr(state, e)).into_iter().collect()
            };
            ctx_inputs.insert(input.id.clone(), values);
        }
        let statements = {
            let mut env = NodeEnv {
                em: self,
                state,
                node,
                descriptor: descriptor.clone(),
                resolved: resolved.clone(),
                variadic: ctx_inputs
                    .iter()
                    .filter(|(id, _)| resolved.resolved_inputs.iter().any(|i| &i.id == *id && i.variadic))
                    .map(|(id, v)| (id.clone(), v.join(", ")))
                    .collect(),
            };
            let mut ctx = crate::types::WgslCtx {
                var_name: var,
                output_vars,
                data: node.data.clone(),
                inputs: ctx_inputs,
                input_types: resolved
                    .input_types
                    .iter()
                    .filter(|(_, t)| !t.is_texture())
                    .map(|(id, t)| (id.clone(), *t))
                    .collect(),
                output_type: resolved.output_type.value_type(),
                resolved_outputs: resolved.resolved_outputs.clone(),
                resolved_inputs: resolved.resolved_inputs.clone(),
                env: &mut env,
            };
            (descriptor.wgsl)(&mut ctx)
        };
        self.push(&node.id, statements, indent);
    }

    fn emit_group_call(&mut self, state: &mut ShaderState, node: &GraphNode, indent: &str) {
        let Some(definition_ref) = node.definition_ref() else { return };
        let project = state.graph.project;
        let descriptor = resolve_descriptor(node, project);
        // Fallback inputs this call leaves without a value pick the group's own default.
        let absent: BTreeSet<String> = descriptor
            .inputs
            .iter()
            .filter(|i| i.fallback && !self.is_present(&node.id, &i.id))
            .map(|i| i.id.clone())
            .collect();
        let resolved = state.graph.resolver.resolve(project, node, &self.bindings);
        // Enum values known here are compiled into the call's function.
        let group_bindings = resolved.bindings.clone().unwrap_or_default();
        let constants =
            group_constants(&self.enum_document(), node, project, &group_bindings, &self.constants);
        let inner_path: Vec<String> = self.path.iter().cloned().chain([node.id.clone()]).collect();
        let probe = self
            .probe
            .as_ref()
            .filter(|p| p.path.first() == Some(&node.id))
            .map(Probe::inner);
        let Some(decl) = emit_group_function(
            state,
            definition_ref,
            &absent,
            &constants,
            &group_bindings,
            Some(&self.read_outputs(&node.id)),
            &inner_path,
            probe.as_ref(),
        ) else {
            return;
        };
        // Converted to the function's parameters, which its body was emitted for.
        let mut args = Vec::new();
        for input in &descriptor.inputs {
            if let Some(ty) = decl.param_types.get(&input.id) {
                let default = descriptor.default_value(&input.id).copied();
                args.push(self.input_expr(state, node, &input.id, *ty, default.as_ref()));
            }
        }
        let line = format!("let {} = {}({});", var_name(&node.id), decl.fn_name, args.join(", "));
        self.push(&node.id, vec![line], indent);
    }

    /// The unit (node id, or `zone:` id of a child zone) standing for `node_id` within `scope`.
    fn unit_in(&self, scope: Option<&str>, node_id: &str) -> Option<String> {
        let mut zone = self.zones.node_zone.get(node_id).cloned().flatten();
        if zone.as_deref() == scope {
            return if self.zones.zone_of_boundary.contains_key(node_id) {
                None
            } else {
                Some(node_id.to_owned())
            };
        }
        while let Some(z) = zone {
            let parent = self.zones.zones.get(&z).and_then(|zone| zone.parent.clone());
            if parent.as_deref() == scope {
                return Some(format!("zone:{z}"));
            }
            zone = parent;
        }
        None
    }

    fn emit_scope(&mut self, state: &mut ShaderState, scope: Option<&str>, indent: &str) {
        let mut units: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        // Document order keeps the output stable.
        for node in self.doc_nodes.clone() {
            if !self.reachable.contains(&node.id) {
                continue;
            }
            if let Some(unit) = self.unit_in(scope, &node.id)
                && seen.insert(unit.clone()) {
                    units.push(unit);
                }
        }
        let mut unit_edges = Vec::new();
        for e in &self.edges {
            if let (Some(s), Some(t)) = (self.unit_in(scope, &e.source), self.unit_in(scope, &e.target))
                && s != t {
                    unit_edges.push((s, t));
                }
        }
        let order = match topological_sort(&units, &unit_edges) {
            Ok(order) => order,
            Err(error) => return state.fail(error),
        };
        for unit in order {
            if let Some(zone_id) = unit.strip_prefix("zone:") {
                let Some(zone) = self.zones.zones.get(zone_id).cloned() else { continue };
                match zone.kind {
                    ZoneKind::Repeat | ZoneKind::Passes => self.emit_zone_loop(state, &zone, indent),
                    ZoneKind::Simulation => self.emit_simulation(state, &zone, indent),
                }
                continue;
            }
            let Some(node) = self.node(&unit).cloned() else { continue };
            if node.is(GROUP_INPUT) || node.is(GROUP_OUTPUT) {
                continue;
            }
            self.emit_node(state, &node, indent);
        }
    }

    fn emit_zone_loop(&mut self, state: &mut ShaderState, zone: &Zone, indent: &str) {
        let (Some(input), Some(output)) =
            (self.node(&zone.input_id).cloned(), self.node(&zone.output_id).cloned())
        else {
            return;
        };
        let all_items: Vec<ZoneItem> = zone_items(Some(&output));
        let names = zone_var_names(zone, all_items.len());
        let live = self.live_items.get(&zone.output_id).cloned().unwrap_or_default();
        // Variables keep their item's index among all items, live or not.
        let items: Vec<(usize, &ZoneItem)> =
            all_items.iter().enumerate().filter(|(_, i)| live.contains(&i.id)).collect();
        let types = self.zone_item_types(state, zone);
        let type_of = |item: &ZoneItem| types.get(&item.id).copied().unwrap_or(WgslType::F32);

        let mut vars: HashMap<String, String> = HashMap::new();
        let mut output_vars: HashMap<String, String> = HashMap::new();
        for (k, item) in all_items.iter().enumerate() {
            vars.insert(item.id.clone(), names.state[k].clone());
            output_vars.insert(item.id.clone(), names.state[k].clone());
        }
        vars.insert(ITERATION_HANDLE.to_owned(), names.index.clone());
        self.zone_vars.insert(zone.input_id.clone(), vars);
        self.zone_vars.insert(zone.output_id.clone(), output_vars);

        let mut setup: Vec<String> = Vec::new();
        for (k, item) in &items {
            let initial = self.input_expr(state, &input, &item.id, type_of(item), None);
            setup.push(format!("var {}: {} = {initial};", names.state[*k], type_of(item).wgsl_name()));
        }
        let default_iterations = InputValue::from(DEFAULT_ITERATIONS);
        let iterations = self.input_expr(state, &input, ITERATIONS_HANDLE, WgslType::I32, Some(&default_iterations));
        let max = zone_max_iterations(&input);
        setup.push(format!("let {} = clamp({iterations}, 0, {max});", names.count));
        setup.push(format!(
            "for (var {i}: i32 = 0; {i} < {n}; {i}++) {{",
            i = names.index,
            n = names.count
        ));
        self.push(&zone.input_id, setup, indent);
        self.emit_scope(state, Some(&zone.id), &format!("{indent}  "));
        // Compute every next value before assigning any: a next value may read
        // another item's current value.
        let mut update: Vec<String> = Vec::new();
        for (k, item) in &items {
            let next = self.input_expr(state, &output, &item.id, type_of(item), None);
            update.push(format!("  let {} = {next};", names.next[*k]));
        }
        for (k, _) in &items {
            update.push(format!("  {} = {};", names.state[*k], names.next[*k]));
        }
        update.push("}".to_owned());
        self.push(&zone.output_id, update, indent);
    }
}

impl BodyEmitter {
    /// A texel of a state item as of `read`'s frame, at the pixel; one kept at
    /// another size than the pass is the nearest to the pixel.
    fn load_state(
        &self,
        state: &mut ShaderState,
        simulation: &SimulationState,
        item: &StateItem,
        read: StateRead,
    ) -> String {
        let texture = state.texture_var(&state_source(&simulation.reference, item, read));
        if same_size(&item.size, &state.target_size) {
            return format!("textureLoad({texture}, vec2i({FRAGMENT_VAR}.position), 0)");
        }
        let dimensions = format!("vec2f(textureDimensions({texture}))");
        let coordinates = texture_coordinates(&format!("{FRAGMENT_VAR}.uv"));
        format!(
            "textureLoad({texture}, vec2i(clamp({coordinates} * {dimensions}, vec2f(0.0), {dimensions} - 1.0)), 0)"
        )
    }

    /// Emits a simulation zone: its state read from the textures keeping it,
    /// but where the probe is in it (see `SimulationMode`).
    fn emit_simulation(&mut self, state: &mut ShaderState, zone: &Zone, indent: &str) {
        let (Some(input), Some(output)) =
            (self.node(&zone.input_id).cloned(), self.node(&zone.output_id).cloned())
        else {
            return;
        };
        let Some(simulation) = state.sources.simulation(&self.path, &zone.id) else { return };
        let mode = self.simulations.get(&zone.id).copied().unwrap_or(SimulationMode::Opaque);
        let all_items = zone_items(Some(&output));
        let vars = simulation_var_names(zone, all_items.len());
        let index_of = |id: &str| all_items.iter().position(|i| i.id == id).unwrap_or(0);
        // A texture item's values are read where they're used, as a texture.
        let values: Vec<&StateItem> = simulation.items.iter().filter(|i| !i.ty.is_texture()).collect();
        let targeted: Vec<StateItem> = match &self.probe {
            Some(Probe { kind: ProbeKind::Simulation { items, .. }, .. }) => items.clone(),
            _ => Vec::new(),
        };
        let is_targeted = |i: &StateItem| targeted.iter().any(|t| t.texture == i.texture);

        if mode == SimulationMode::Opaque {
            let read = self.read_outputs(&zone.output_id);
            let mut lines = Vec::new();
            for i in values.iter().filter(|i| read.contains(&i.item.id)) {
                let load = self.load_state(state, &simulation, i, StateRead::Current);
                lines.push(format!(
                    "let {} = {};",
                    vars.current[index_of(&i.item.id)],
                    unpack_state(&load, i.value_type(), i.format)
                ));
            }
            self.push(&zone.output_id, lines, indent);
            return;
        }
        if mode == SimulationMode::Start {
            self.simulation_fields = targeted
                .iter()
                .map(|i| {
                    let ty = i.value_type();
                    let initial = self.input_expr(state, &input, &i.item.id, ty, None);
                    pack_state(&initial, ty, i.format)
                })
                .collect();
            return;
        }

        let read = self.read_outputs(&zone.input_id);
        let step = mode == SimulationMode::Step;
        let mut previous = Vec::new();
        for i in values.iter().filter(|i| read.contains(&i.item.id) || (step && is_targeted(i))) {
            let load = self.load_state(state, &simulation, i, StateRead::Previous);
            previous.push(format!(
                "let {} = {};",
                vars.previous[index_of(&i.item.id)],
                unpack_state(&load, i.value_type(), i.format)
            ));
        }
        self.push(&zone.input_id, previous, indent);

        self.emit_scope(state, Some(&zone.id), indent);

        if !step {
            return;
        }
        // Skipping keeps the previous frame's state: the nodes in the zone don't change it.
        let skip = self.input_expr(state, &output, SKIP_HANDLE, WgslType::Bool, Some(&InputValue::from(false)));
        let mut update = vec![format!("let {} = {skip};", vars.skip)];
        let mut fields = Vec::new();
        for i in &targeted {
            let ty = i.value_type();
            let k = index_of(&i.item.id);
            let next = &vars.next[k];
            let kept = if i.ty.is_texture() {
                self.load_state(state, &simulation, i, StateRead::Previous)
            } else {
                vars.previous[k].clone()
            };
            let value = self.input_expr(state, &output, &i.item.id, ty, None);
            update.push(format!("let {next} = select({value}, {kept}, {});", vars.skip));
            fields.push(pack_state(next, ty, i.format));
        }
        self.simulation_fields = fields;
        self.push(&zone.output_id, update, indent);
    }
}

/// What a node's `WgslCtx` asks of the generator.
struct NodeEnv<'a, 's, 'g> {
    em: &'a mut BodyEmitter,
    state: &'a mut ShaderState<'s, 'g>,
    node: &'a GraphNode,
    descriptor: Arc<NodeDescriptor>,
    resolved: Arc<ResolvedNodeTypes>,
    /// Variadic inputs' operands, joined.
    variadic: HashMap<String, String>,
}

impl CtxEnv for NodeEnv<'_, '_, '_> {
    fn promote_input(&mut self, id: &str) -> String {
        if let Some(joined) = self.variadic.get(id) {
            return joined.clone();
        }
        if self.resolved.input_types.get(id).is_some_and(|t| t.is_texture()) {
            return CtxEnv::texture(self, id);
        }
        let ty = self
            .resolved
            .input_types
            .get(id)
            .copied()
            .unwrap_or_else(|| {
                self.resolved
                    .resolved_inputs
                    .iter()
                    .find(|i| i.id == id)
                    .map_or(WgslType::F32, |i| i.default_type())
            })
            .value_type();
        let default = self.descriptor.default_value(id).copied();
        self.em.input_expr(self.state, self.node, id, ty, default.as_ref())
    }

    fn is_present(&self, id: &str) -> bool {
        self.em.is_present(&self.node.id, id)
    }

    fn helper(&mut self, name: HelperName, ty: WgslType) -> String {
        self.state.helpers.use_helper(name, ty)
    }

    fn constant(&self, id: &str) -> Option<i64> {
        self.em.constant_of(&self.node.id, id)
    }

    fn param(&mut self, key: &str, value: &[f64]) -> String {
        self.em.param_expr(self.state, self.node, key, value)
    }

    fn define_fn(&mut self, name: &str, source: &str) -> String {
        self.state.helpers.define(name, source)
    }

    fn texture(&mut self, id: &str) -> String {
        let source = self.em.texture_source_of(self.state, self.node, &self.resolved, id);
        self.state.texture_var(&source)
    }

    fn texture_format(&self, id: &str) -> TextureFormat {
        self.em.texture_source_of(self.state, self.node, &self.resolved, id).format()
    }

    fn asset(&self) -> Option<crate::assets::Asset> {
        let id = self.node.data.asset.as_deref()?;
        self.state.graph.project.assets.iter().find(|a| a.id() == id).cloned()
    }

    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String {
        self.state.sampler_var(filter, address)
    }
}

fn input_type_in(
    state: &ShaderState,
    bindings: &TypeBindings,
    node: &GraphNode,
    input: &str,
) -> Option<WgslType> {
    let ty = state.graph.resolver.resolve(state.graph.project, node, bindings).input_type(input)?;
    (!ty.is_texture()).then_some(ty)
}

/// The frame values of the root document pixel computations read, to be read
/// from the frame block: the outputs of nodes outside zones that are frame
/// values depending on something the runtime knows (not only literals) and
/// that something computed per pixel reads. A node whose values are all read
/// only by other frame values stays inside the frame program. Groups' bodies
/// are shared by their instances, so their values aren't hoisted.
#[allow(clippy::too_many_arguments)]
fn hoist_frame_values(
    state: &mut ShaderState,
    path: &[String],
    doc: &GraphDocument,
    zones: &ZoneAnalysis,
    edges: &[GraphEdge],
    reachable: &HashSet<String>,
    bindings: &TypeBindings,
    seeds: &HashSet<String>,
) -> HashMap<String, BTreeMap<String, HoistedValue>> {
    let mut hoisted = HashMap::new();
    let Some(frame) = state.frame.filter(|_| path.is_empty()) else { return hoisted };
    let project = state.graph.project;
    let mut read: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for e in edges {
        read.entry(&e.source).or_default().insert(&e.source_handle);
    }
    // The nodes computed on the CPU, whatever reads them.
    let mut on_cpu: Vec<&GraphNode> = Vec::new();
    for node in doc.nodes.iter().filter(|n| reachable.contains(&n.id)) {
        let Some(handles) = read.get(node.id.as_str()) else { continue };
        if seeds.contains(&node.id) || zones.node_zone.get(&node.id).is_some_and(Option::is_some) {
            continue;
        }
        if resolve_descriptor(node, project).cpu.is_none() {
            continue;
        }
        if handles.iter().all(|h| frame.analysis.is_frame_value(path, &node.id, h)) {
            on_cpu.push(node);
        }
    }
    let cpu_ids: HashSet<&str> = on_cpu.iter().map(|n| n.id.as_str()).collect();
    for node in on_cpu {
        if !edges.iter().any(|e| e.source == node.id && !cpu_ids.contains(e.target.as_str())) {
            continue;
        }
        let types = state.graph.resolver.resolve(project, node, bindings);
        let handles = &read[node.id.as_str()];
        let mut values = BTreeMap::new();
        let mut uses = Vec::new();
        for handle in handles {
            let Some(reference) = frame.hoist(path, &node.id, handle) else { break };
            let Some(output) = types.resolved_outputs.iter().find(|o| o.id == *handle) else { break };
            if output.ty.is_texture() {
                break;
            }
            let ty = output.ty;
            let (scalar, kind) = match ty.kind() {
                ScalarKind::F32 => (UniformScalar::F32, ScalarKind::F32),
                ScalarKind::I32 => (UniformScalar::I32, ScalarKind::I32),
                ScalarKind::U32 | ScalarKind::Bool => (UniformScalar::U32, ScalarKind::U32),
            };
            let name = frame_value_field(&node.id, handle);
            values.insert(
                (*handle).to_owned(),
                HoistedValue { field: name.clone(), storage: make_type(kind, ty.components()), ty },
            );
            uses.push(FrameValueUse { name, reference, scalar, components: ty.components() as u32 });
        }
        // All of a node's values are hoisted or none, as what feeds it is dropped.
        if values.len() != handles.len() {
            continue;
        }
        hoisted.insert(node.id.clone(), values);
        for used in uses {
            match state.frame_values.iter_mut().find(|u| u.name == used.name) {
                Some(existing) => *existing = used,
                None => state.frame_values.push(used),
            }
        }
    }
    hoisted
}

/// Emit the nodes of a document needed to compute the `seed_type` nodes
/// and the probe's.
///
/// Nodes are grouped by scope: the document's top level, then each zone.
/// Within a scope, nodes and child zones are emitted in dependency order; a
/// child repeat zone becomes a `for` loop whose body is its own scope. Loop
/// invariants live in the enclosing scope, so they are computed once before
/// the loop. A simulation zone is computed by passes of its own, its state
/// read from the textures keeping it, but where the probe is in it (see
/// `SimulationMode`). Invalid links, and links to sockets the nodes don't
/// have with `bindings`, are ignored (inputs fall back to their stored values).
#[allow(clippy::too_many_arguments)]
fn emit_document_body(
    state: &mut ShaderState,
    doc: &GraphDocument,
    seed_type: &str,
    absent_inputs: &BTreeSet<String>,
    constants: &BTreeMap<String, i64>,
    uniforms: bool,
    bindings: &TypeBindings,
    seed_handles: Option<&BTreeSet<String>>,
    path: &[String],
    probe: Option<&Probe>,
) -> BodyEmitter {
    let project = state.graph.project;
    let zones = analyze_zones(&doc.nodes, &doc.edges, &zone_options(project));
    let node_by_id: HashMap<&str, &GraphNode> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // The node the probe is computed at, here: the group node it goes through,
    // or the socket's node, or the boundary node whose inputs a simulation
    // target computes.
    let probe_node: Option<String> = probe.and_then(|p| {
        if let Some(inner) = p.path.first() {
            return Some(inner.clone());
        }
        match &p.kind {
            ProbeKind::Simulation { zone, stage: SimulationStage::Start, .. } => Some(zone.clone()),
            ProbeKind::Simulation { zone, .. } => zones.zones.get(zone).map(|z| z.output_id.clone()),
            ProbeKind::Socket { node, .. } | ProbeKind::Textures { node, .. } => Some(node.clone()),
        }
    });
    let mut simulations: HashMap<String, SimulationMode> = HashMap::new();
    for zone in zones.zones.values().filter(|z| z.kind == ZoneKind::Simulation) {
        let mode = match probe {
            Some(Probe { path, kind: ProbeKind::Simulation { zone: z, stage, .. } })
                if path.is_empty() && *z == zone.id =>
            {
                match stage {
                    SimulationStage::Start => SimulationMode::Start,
                    SimulationStage::Step => SimulationMode::Step,
                }
            }
            _ if probe_node
                .as_ref()
                .is_some_and(|n| zone.members.contains(n) || *n == zone.input_id) =>
            {
                SimulationMode::Inline
            }
            _ => SimulationMode::Opaque,
        };
        simulations.insert(zone.id.clone(), mode);
        if mode != SimulationMode::Opaque {
            state.simulation = Some(SimulationRef { path: path.to_vec(), zone: zone.id.clone() }.id());
        }
    }
    let probe_items: HashSet<String> = match probe {
        Some(Probe { path, kind: ProbeKind::Simulation { items, .. } }) if path.is_empty() => {
            items.iter().map(|i| i.item.id.clone()).collect()
        }
        _ => HashSet::new(),
    };
    // Whether a link into a simulation's boundary node is computed here.
    let into_simulation = |e: &GraphEdge| -> bool {
        let zone = zones.zone_of_boundary.get(&e.target).and_then(|z| zones.zones.get(z));
        let mode = zone.and_then(|z| simulations.get(&z.id));
        let (Some(zone), Some(mode)) = (zone, mode) else { return true };
        if e.target == zone.input_id {
            return *mode == SimulationMode::Start;
        }
        *mode == SimulationMode::Step
            && (e.target_handle == SKIP_HANDLE || probe_items.contains(&e.target_handle))
    };

    let unavailable = unavailable_edges(&doc.edges, |id| {
        node_by_id.get(id).map(|n| state.graph.resolver.resolve(project, n, bindings))
    });
    // Links into Texture inputs compute nothing here: textures are read where
    // they're used, and values are rendered to them by passes of their own.
    let into_texture = |e: &GraphEdge| {
        node_by_id.get(e.target.as_str()).is_some_and(|n| {
            state
                .graph
                .resolver
                .resolve(project, n, bindings)
                .input_type(&e.target_handle)
                .is_some_and(WgslType::is_texture)
        })
    };
    // A Render Texture's values are computed by the pass drawing them, and only its items'.
    let into_render_texture = |e: &GraphEdge| {
        if !node_by_id.get(e.target.as_str()).is_some_and(|n| n.is(RENDER_TEXTURE)) {
            return false;
        }
        !matches!(probe, Some(Probe { path, kind: ProbeKind::Textures { node, items } })
            if path.is_empty() && *node == e.target && items.iter().any(|i| i.item == e.target_handle))
    };
    let valid: Vec<GraphEdge> = doc
        .edges
        .iter()
        .filter(|e| {
            !zones.invalid_edges.contains(&e.id)
                && !unavailable.contains(&e.id)
                && !into_texture(e)
                && !into_render_texture(e)
                && into_simulation(e)
        })
        .cloned()
        .collect();
    let input_type = |node: &str, input: &str| {
        input_type_in(state, bindings, node_by_id.get(node)?, input)
    };
    let enum_doc = EnumDocument {
        nodes: &doc.nodes,
        edges: &valid,
        inputs: &doc.inputs,
        input_type: Some(&input_type),
    };
    let used: Vec<GraphEdge> = enum_doc
        .live_edges(&valid, constants)
        .into_iter()
        .filter(|e| {
            seed_handles.is_none_or(|handles| {
                !node_by_id.get(e.target.as_str()).is_some_and(|n| n.is(seed_type))
                    || handles.contains(&e.target_handle)
            })
        })
        .collect();
    let seeds: HashSet<String> = probe_node.into_iter().collect();
    let viewed: LiveItems = match probe {
        Some(Probe { path, kind: ProbeKind::Socket { node, handle } })
            if path.is_empty() && node_by_id.get(node.as_str()).is_some_and(|n| n.is(REPEAT_OUTPUT)) =>
        {
            BTreeMap::from([(node.clone(), BTreeSet::from([handle.clone()]))])
        }
        _ => LiveItems::new(),
    };
    let computes = |input: &GraphNode| {
        !matches!(
            simulations.get(&input.id),
            Some(SimulationMode::Opaque | SimulationMode::Start)
        )
    };
    let mut needed = needed_edges(&doc.nodes, &used, seed_type, &seeds, &viewed, &computes);
    let hoisted =
        hoist_frame_values(state, path, doc, &zones, &needed.edges, &needed.reachable, bindings, &seeds);
    // What only a hoisted value read is no longer needed.
    if !hoisted.is_empty() {
        let pruned: Vec<GraphEdge> = used.iter().filter(|e| !hoisted.contains_key(&e.target)).cloned().collect();
        needed = needed_edges(&doc.nodes, &pruned, seed_type, &seeds, &viewed, &computes);
    }
    let mut incoming: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    let mut read_handles: HashMap<String, BTreeSet<String>> = HashMap::new();
    for e in &needed.edges {
        incoming.entry(e.target.clone()).or_default().push(e.clone());
        read_handles.entry(e.source.clone()).or_default().insert(e.source_handle.clone());
    }
    let mut emitter = BodyEmitter {
        lines: Vec::new(),
        line_map: BTreeMap::new(),
        node_index: doc.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect(),
        doc_nodes: doc.nodes.clone(),
        edges: needed.edges,
        incoming,
        zone_vars: HashMap::new(),
        inputs: doc.inputs.clone(),
        absent_inputs: absent_inputs.clone(),
        constants: constants.clone(),
        uniforms,
        bindings: bindings.clone(),
        zones,
        reachable: needed.reachable,
        live_items: needed.live_items,
        path: path.to_vec(),
        probe: probe.cloned(),
        simulations,
        read_handles,
        simulation_fields: Vec::new(),
        hoisted,
    };
    for zone in emitter.zones.zones.values().filter(|z| z.kind == ZoneKind::Simulation).cloned().collect::<Vec<_>>() {
        let items = zone_items(emitter.node(&zone.output_id)).len();
        let names = simulation_var_names(&zone, items);
        let ids: Vec<String> = zone_items(emitter.node(&zone.output_id)).into_iter().map(|i| i.id).collect();
        let mut previous: HashMap<String, String> =
            ids.iter().cloned().zip(names.previous.iter().cloned()).collect();
        previous.insert(DELTA_TIME_HANDLE.to_owned(), format!("{FRAME_VAR}.{SIMULATION_DELTA_FIELD}"));
        emitter.zone_vars.insert(zone.input_id.clone(), previous);
        emitter.zone_vars.insert(zone.output_id.clone(), ids.into_iter().zip(names.current).collect());
    }
    emitter.emit_scope(state, None, "  ");
    emitter
}

/// Node types reading or giving textures of their own: a simulation's keep its state.
const TEXTURE_NODE_TYPES: [&str; 6] =
    [crate::nodes::IMAGE, crate::nodes::VIDEO, SAMPLE_TEXTURE, TEXTURE_SIZE, SIMULATION_INPUT, SIMULATION_OUTPUT];

/// Whether a definition reads textures: itself, through its sockets or the groups it nests.
fn reads_textures(state: &mut ShaderState, definition_ref: &str) -> bool {
    if let Some(known) = state.reads_textures.get(definition_ref) {
        return *known;
    }
    // A group nesting itself reads none through itself.
    state.reads_textures.insert(definition_ref.to_owned(), false);
    let Some(def) = state.graph.project.document(definition_ref) else { return false };
    let texture = |h: &GroupHandleDef| h.ty.fixed().is_some_and(WgslType::is_texture);
    let reads = def.inputs.iter().any(texture)
        || def.outputs.iter().any(texture)
        || def.nodes.iter().any(|n| {
            TEXTURE_NODE_TYPES.contains(&n.node_type.as_str())
                || n.is(RENDER_TEXTURE)
                || n.definition_ref().is_some_and(|r| reads_textures(state, r))
        });
    state.reads_textures.insert(definition_ref.to_owned(), reads);
    reads
}

/// What the textures read by a group instance at `path` are (its own nodes'
/// and those of the groups it nests): its variants reading other textures
/// are other functions. Empty for a group reading none.
fn texture_fingerprint(state: &mut ShaderState, definition_ref: &str, path: &[String]) -> String {
    if !reads_textures(state, definition_ref) {
        return String::new();
    }
    let (Some(def), Some(instance)) =
        (state.graph.project.document(definition_ref), state.sources.instance(path))
    else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for node in &def.nodes {
        // Each instance keeps a state of its own.
        if node.is(SIMULATION_INPUT) {
            parts.push(format!("sim:{}", SimulationRef { path: path.to_vec(), zone: node.id.clone() }.id()));
        }
        if let Some(nested) = node.definition_ref() {
            let inner: Vec<String> = path.iter().cloned().chain([node.id.clone()]).collect();
            let nested = texture_fingerprint(state, nested, &inner);
            if !nested.is_empty() {
                parts.push(format!("{}({nested})", node.id));
            }
        }
        let types = state.graph.resolver.resolve(state.graph.project, node, &instance.bindings);
        for (id, ty) in &types.input_types {
            if ty.is_texture() {
                parts.push(format!("{}.{id}={}", node.id, state.sources.input(path, &node.id, id).source().id()));
            }
        }
        for output in &types.resolved_outputs {
            if output.ty.is_texture() {
                parts.push(format!(
                    "{}:{}={}",
                    node.id,
                    output.id,
                    state.sources.output(path, &node.id, &output.id).source().id()
                ));
            }
        }
    }
    parts.join(",")
}

/// Generate one group's `struct` + `fn` (recursively for nested groups).
#[allow(clippy::too_many_arguments)]
fn emit_group_function(
    state: &mut ShaderState,
    definition_ref: &str,
    absent_inputs: &BTreeSet<String>,
    constants: &BTreeMap<String, i64>,
    given: &TypeBindings,
    // The outputs the caller reads; all of them when `None`.
    read_outputs: Option<&BTreeSet<String>>,
    // The group nodes it's computed through, from the root, the caller last.
    path: &[String],
    probe: Option<&Probe>,
) -> Option<GroupDecl> {
    let project = state.graph.project;
    let definition = project.document(definition_ref)?;
    let variables = &definition.type_variables;
    let bindings = complete_bindings(variables, given);

    // One function per combination of absent fallback inputs, enum values,
    // variables' types and outputs read, so each only contains what it uses.
    let absent_indices: Vec<usize> = definition
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, h)| absent_inputs.contains(&h.id))
        .map(|(i, _)| i)
        .collect();
    let enum_values: Vec<String> = definition
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(i, h)| constants.get(&h.id).map(|v| format!("{i}_{v}")))
        .collect();
    let type_suffix: String =
        variables.iter().map(|v| bindings.get(&v.id).copied().unwrap_or(WgslType::F32).to_string()).collect::<Vec<_>>().join("_");
    let unread_indices: Vec<usize> = definition
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, h)| read_outputs.is_some_and(|r| !r.contains(&h.id)))
        .map(|(i, _)| i)
        .collect();
    let join = |list: &[usize]| list.iter().map(ToString::to_string).collect::<Vec<_>>().join("|");
    let textures = texture_fingerprint(state, definition_ref, path);
    let key = format!(
        "{definition_ref}|{}|e|{}|t|{type_suffix}|u|{}|x|{textures}|v|{}",
        join(&absent_indices),
        enum_values.join("|"),
        join(&unread_indices),
        probe.map_or_else(String::new, Probe::key)
    );
    if let Some((_, existing)) = state.group_decls.iter().find(|(k, _)| *k == key) {
        return Some(existing.clone());
    }

    let ident = sanitize_ident(definition_ref);
    // Variants reading other textures are numbered, their reads being too long for a name.
    let texture_variant = (!textures.is_empty()).then(|| {
        let count = state.texture_variants.entry(definition_ref.to_owned()).or_default();
        *count += 1;
        *count - 1
    });
    let mut suffix: Vec<String> = Vec::new();
    if !absent_indices.is_empty() {
        suffix.push(format!(
            "d{}",
            absent_indices.iter().map(ToString::to_string).collect::<Vec<_>>().join("_")
        ));
    }
    suffix.extend(enum_values.iter().map(|v| format!("e{v}")));
    if !unread_indices.is_empty() {
        suffix.push(format!(
            "u{}",
            unread_indices.iter().map(ToString::to_string).collect::<Vec<_>>().join("_")
        ));
    }
    if let Some(variant) = texture_variant {
        suffix.push(format!("x{variant}"));
    }
    if probe.is_some() {
        suffix.push("v".to_owned());
    }
    let mut fn_parts = vec![format!("group_{ident}")];
    fn_parts.extend(suffix);
    if !type_suffix.is_empty() {
        fn_parts.push(format!("t_{type_suffix}"));
    }
    let fn_name = fn_parts.join("__");
    let mut struct_parts = vec![format!("GroupOutputs_{ident}")];
    if !type_suffix.is_empty() {
        struct_parts.push(type_suffix);
    }
    if probe.is_some() {
        struct_parts.push("v".to_owned());
    }
    let struct_name = struct_parts.join("__");

    let read_handles: BTreeSet<String> = definition
        .outputs
        .iter()
        .enumerate()
        .filter(|(i, _)| !unread_indices.contains(i))
        .map(|(_, h)| h.id.clone())
        .collect();
    let group_uniforms = state.uniform_scopes.contains(definition_ref);
    let body = emit_document_body(
        state,
        definition,
        GROUP_OUTPUT,
        absent_inputs,
        constants,
        group_uniforms,
        &bindings,
        Some(&read_handles),
        path,
        probe,
    );

    // Struct fields follow the handle types; whatever feeds them is converted
    // on the way in, exactly like any other typed input.
    let output_types = |node: Option<&GraphNode>| {
        node.map(|n| state.graph.resolver.resolve(project, n, &bindings).input_types.clone())
            .unwrap_or_default()
    };
    // Textures are no WGSL values: they're read where they're used, never passed.
    let outputs: Vec<(&GroupHandleDef, WgslType)> = definition
        .outputs
        .iter()
        .map(|h| {
            let ty = bound_type(&h.ty, variables, &bindings)
                .or_else(|| output_types(definition.output_node_of(&h.id)).get(&h.id).copied())
                .unwrap_or(WgslType::F32);
            (h, ty)
        })
        .filter(|(_, ty)| !ty.is_texture())
        .collect();
    let inputs: Vec<(&GroupHandleDef, WgslType)> = definition
        .inputs
        .iter()
        .map(|h| (h, bound_type(&h.ty, variables, &bindings).unwrap_or(WgslType::F32)))
        .filter(|(_, ty)| !ty.is_texture())
        .collect();
    // Outputs the caller doesn't read keep their struct field, zeroed.
    let mut field_values = Vec::new();
    for (handle, ty) in &outputs {
        let node = definition.output_node_of(&handle.id);
        field_values.push(match node {
            Some(node) if read_handles.contains(&handle.id) => {
                body.input_expr(state, node, &handle.id, *ty, None)
            }
            _ => ty.zero(),
        });
    }
    field_values.extend(body.probe_fields(state));
    let mut struct_fields: Vec<String> = outputs
        .iter()
        .map(|(h, ty)| format!("  {}: {},", sanitize_ident(&h.id), ty.wgsl_name()))
        .collect();
    if let Some(probe) = probe {
        struct_fields.extend(
            probe.types().iter().enumerate().map(|(k, t)| format!("  {}: {},", Probe::field(k), t.wgsl_name())),
        );
    }
    let params: Vec<String> = inputs
        .iter()
        .map(|(h, ty)| format!("{}: {}", sanitize_ident(&h.id), ty.wgsl_name()))
        .collect();
    // The specialisations of a definition with the same types share one output struct.
    let first_of_definition = state.group_structs.insert(struct_name.clone());
    let mut decl: Vec<String> = Vec::new();
    if first_of_definition {
        decl.push(format!("struct {struct_name} {{\n{}\n}}", struct_fields.join("\n")));
        decl.push(String::new());
    }
    decl.push(format!("fn {fn_name}({}) -> {struct_name} {{", params.join(", ")));
    decl.extend(body.lines.iter().cloned());
    decl.push(format!("  return {struct_name}({});", field_values.join(", ")));
    decl.push("}".to_owned());
    let entry = GroupDecl {
        decl: decl.join("\n"),
        fn_name,
        param_types: inputs.iter().map(|(h, t)| (h.id.clone(), *t)).collect(),
    };
    state.group_decls.push((key, entry.clone()));
    Some(entry)
}

/// Emit the full WGSL fragment shader source from the graph: `frag` returns
/// a colour.
pub fn generate_shader(
    graph: CodegenGraph<'_>,
    options: &ShaderOptions,
) -> Result<ShaderSource, CodegenError> {
    let sources = TextureSources::new(graph);
    generate_shader_with(graph, options, &sources, None, &CANVAS_SIZE, None)
}

/// Emit the WGSL fragment shader source drawing `target`: a colour for a
/// socket, or, for the texels of a simulation's items or a Render Texture's,
/// a `RuntimeTargets` struct of them (`ShaderSource::targets` their types).
/// The fragment output by default. The shaders of one graph state share its
/// `sources`; `target_size` is the size of what they draw into. With a
/// `frame` compiler, the root document's frame values are hoisted out of the
/// pixel computation, read from a block the runtime writes (`FrameValueUse`)
/// from the values of the compiler's program; without it, computed per pixel.
pub fn generate_shader_with<'g>(
    graph: CodegenGraph<'g>,
    options: &ShaderOptions,
    sources: &TextureSources<'g>,
    target: Option<&ShaderTarget>,
    target_size: &TextureSize,
    frame: Option<&FrameCompiler<'g>>,
) -> Result<ShaderSource, CodegenError> {
    let mut state = ShaderState {
        graph,
        frame,
        frame_values: Vec::new(),
        helpers: HelperRegistry::default(),
        sources,
        target_size: target_size.clone(),
        textures: Vec::new(),
        samplers: Vec::new(),
        texture_variants: HashMap::new(),
        reads_textures: HashMap::new(),
        simulation: None,
        group_decls: Vec::new(),
        group_structs: HashSet::new(),
        uniform_scopes: options.uniform_scopes.iter().cloned().collect(),
        uniforms: Vec::new(),
        error: None,
    };
    let probe = target.map(Probe::of);
    let root = graph.project.root();
    let root_uniforms = state.uniform_scopes.contains(ROOT_ID);
    let body = emit_document_body(
        &mut state,
        root,
        if target.is_some() { "" } else { FRAG_OUTPUT },
        &BTreeSet::new(),
        &BTreeMap::new(),
        root_uniforms,
        &BTreeMap::new(),
        None,
        &[],
        probe.as_ref(),
    );
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    let output = root.nodes.iter().find(|n| n.is(FRAG_OUTPUT));
    let targets = probe
        .as_ref()
        .filter(|p| !matches!(p.kind, ProbeKind::Socket { .. }))
        .map(Probe::types);
    let fields = probe.as_ref().map(|_| body.probe_fields(&mut state));
    let result = if targets.is_some() {
        Some(format!("{}({})", super::program::TARGETS_STRUCT, fields.unwrap_or_default().join(", ")))
    } else if let Some(fields) = fields {
        fields.into_iter().next()
    } else {
        output.filter(|n| body.line_map.contains_key(&n.id)).map(|n| var_name(&n.id))
    };

    let mut preamble: Vec<String> = Vec::new();
    // Every group's struct + fn, emitted once each, before frag() itself.
    for (_, group) in &state.group_decls {
        preamble.push(group.decl.clone());
        preamble.push(String::new());
    }
    let helpers = state.helpers.emit();
    if !helpers.is_empty() {
        preamble.push("// ---------- Common libraries ----------".to_owned());
        preamble.push(String::new());
        for f in helpers {
            preamble.push(f);
            preamble.push(String::new());
        }
    }
    preamble.push("// ---------- User code ----------".to_owned());
    preamble.push(String::new());
    let returns = if targets.is_some() { super::program::TARGETS_STRUCT } else { "vec4f" };
    preamble.push(format!("fn frag(uv: vec2f) -> {returns} {{"));
    // Offset the line map so line numbers correspond to the final shader string.
    let offset: usize = preamble.iter().map(|l| l.matches('\n').count() + 1).sum();
    let line_map = body
        .line_map
        .iter()
        .map(|(id, r)| {
            let (start, end) = (line_offset(&body.lines, r.start), line_offset(&body.lines, r.end));
            (id.clone(), offset + start..offset + end)
        })
        .collect();
    let mut lines = preamble;
    lines.extend(body.lines);
    lines.push(match result {
        Some(value) => format!("  return {value};"),
        None => format!("  return {};", WgslType::Vec4f.zero()),
    });
    lines.push("}".to_owned());
    let _ = FRAGMENT_VAR;
    Ok(ShaderSource {
        fragment: lines.join("\n"),
        uniforms: state.uniforms,
        line_map,
        textures: state.textures,
        samplers: state.samplers,
        targets,
        simulation: state.simulation,
        frame_values: state.frame_values,
    })
}

/// The physical line a body entry starts at: entries may hold several lines.
fn line_offset(lines: &[String], index: usize) -> usize {
    lines.iter().take(index).map(|l| l.matches('\n').count() + 1).sum()
}
