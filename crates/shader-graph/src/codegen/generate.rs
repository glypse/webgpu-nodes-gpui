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
use super::program::{FRAGMENT_VAR, UNIFORMS_VAR};
use crate::enums::{EnumDocument, group_constants};
use crate::graph::{
    FRAG_OUTPUT, GROUP_INPUT, GROUP_OUTPUT, GraphDocument, GraphEdge, GraphNode, GraphProject,
    NODE_GROUP, REROUTE, ROOT_ID,
};
use crate::nodes::resolve_descriptor;
use crate::types::{
    CtxEnv, GroupHandleDef, HelperName, InputValue, NodeDescriptor, ResolvedNodeTypes, TypeBindings,
    TypeResolver, WgslType, ZoneItem, bound_type, complete_bindings, convert_var, input_control,
    make_type, unavailable_edges,
};
use crate::zones::{
    DEFAULT_ITERATIONS, ITERATIONS_HANDLE, ITERATION_HANDLE, REPEAT_INPUT, REPEAT_OUTPUT, Zone,
    ZoneAnalysis, ZoneKind, analyze_zones, paired_output_id, zone_items, zone_max_iterations,
    zone_options,
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
}

/// A generated fragment (defining `frag`) and the uniform values it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderSource {
    pub fragment: String,
    /// In the order the shader declares them.
    pub uniforms: Vec<(String, UniformValue)>,
    /// The lines of `fragment` each node's statements are on, by node id.
    pub line_map: BTreeMap<String, Range<usize>>,
}

// ── Graph walking ────────────────────────────────────────────────────────────

/// Walk backwards from the `seed_type` nodes through edges to find every node
/// contributing to them. A zone output implicitly depends on its input.
fn find_reachable(nodes: &[GraphNode], edges: &[GraphEdge], seed_type: &str) -> HashSet<String> {
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        reverse.entry(&e.target).or_default().push(&e.source);
    }
    for n in nodes {
        if let Some(output) = paired_output_id(n) {
            reverse.entry(output).or_default().push(&n.id);
        }
    }
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for n in nodes.iter().filter(|n| n.is(seed_type)) {
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
/// by something needed: others are left out, with what only they used.
fn needed_edges(nodes: &[GraphNode], edges: &[GraphEdge], seed_type: &str) -> Needed {
    let by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut live_items: LiveItems = BTreeMap::new();
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
        let reachable = find_reachable(nodes, &active, seed_type);
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

struct ShaderState<'g> {
    graph: CodegenGraph<'g>,
    helpers: HelperRegistry,
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

impl ShaderState<'_> {
    fn fail(&mut self, error: CodegenError) {
        self.error.get_or_insert(error);
    }

    fn set_uniform(&mut self, name: &str, value: UniformValue) {
        match self.uniforms.iter_mut().find(|(n, _)| n == name) {
            Some((_, existing)) => *existing = value,
            None => self.uniforms.push((name.to_owned(), value)),
        }
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
        resolved.resolved_outputs.iter().map(|o| (o.id.clone(), o.ty)).collect()
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

    fn source_type(&self, state: &ShaderState, edge: &GraphEdge) -> WgslType {
        state.graph.resolver.infer_handle_type(
            state.graph.project,
            &edge.source,
            &edge.source_handle,
            &self.bindings,
        )
    }

    fn source_expr(&self, state: &ShaderState, edge: &GraphEdge) -> String {
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

    fn converted_source(&self, state: &ShaderState, edge: &GraphEdge, target: WgslType) -> String {
        convert_var(&self.source_expr(state, edge), self.source_type(state, edge), target)
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
        if node.is(NODE_GROUP) {
            self.emit_group_call(state, node, indent);
            return;
        }
        let project = state.graph.project;
        let resolved = state.graph.resolver.resolve(project, node, &self.bindings);
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
            resolved.input_types.get(id).copied().unwrap_or_else(|| {
                resolved.resolved_inputs.iter().find(|i| i.id == id).map_or(WgslType::F32, |i| i.default_type())
            })
        };
        // Variadic inputs are pre-converted, non-variadic ones are raw.
        let mut ctx_inputs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for input in &resolved.resolved_inputs {
            let edges: Vec<&GraphEdge> =
                self.incoming(&node.id).iter().filter(|e| e.target_handle == input.id).collect();
            let values = if input.variadic {
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
                input_types: resolved.input_types.clone(),
                output_type: resolved.output_type,
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
        let Some(decl) =
            emit_group_function(state, definition_ref, &absent, &constants, &group_bindings, None)
        else {
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
                    ZoneKind::Repeat => self.emit_zone_loop(state, &zone, indent),
                    ZoneKind::Simulation => state.fail(CodegenError::Unsupported(
                        "Simulation zones keep their state in textures, which aren't supported yet"
                            .to_owned(),
                    )),
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

/// What a node's `WgslCtx` asks of the generator.
struct NodeEnv<'a, 'g> {
    em: &'a mut BodyEmitter,
    state: &'a mut ShaderState<'g>,
    node: &'a GraphNode,
    descriptor: Arc<NodeDescriptor>,
    resolved: Arc<ResolvedNodeTypes>,
    /// Variadic inputs' operands, joined.
    variadic: HashMap<String, String>,
}

impl CtxEnv for NodeEnv<'_, '_> {
    fn promote_input(&mut self, id: &str) -> String {
        if let Some(joined) = self.variadic.get(id) {
            return joined.clone();
        }
        let ty = self.resolved.input_types.get(id).copied().unwrap_or_else(|| {
            self.resolved
                .resolved_inputs
                .iter()
                .find(|i| i.id == id)
                .map_or(WgslType::F32, |i| i.default_type())
        });
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
}

fn input_type_in(
    state: &ShaderState,
    bindings: &TypeBindings,
    node: &GraphNode,
    input: &str,
) -> Option<WgslType> {
    state.graph.resolver.resolve(state.graph.project, node, bindings).input_type(input)
}

/// Emit the nodes of a document needed to compute the `seed_type` nodes.
///
/// Nodes are grouped by scope: the document's top level, then each zone.
/// Within a scope, nodes and child zones are emitted in dependency order; a
/// child repeat zone becomes a `for` loop whose body is its own scope. Loop
/// invariants live in the enclosing scope, so they are computed once before
/// the loop. Invalid links, and links to sockets the nodes don't have with
/// `bindings`, are ignored (inputs fall back to their stored values).
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
) -> BodyEmitter {
    let project = state.graph.project;
    let zones = analyze_zones(&doc.nodes, &doc.edges, &zone_options(project));
    let node_by_id: HashMap<&str, &GraphNode> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let unavailable = unavailable_edges(&doc.edges, |id| {
        node_by_id.get(id).map(|n| state.graph.resolver.resolve(project, n, bindings))
    });
    let valid: Vec<GraphEdge> = doc
        .edges
        .iter()
        .filter(|e| !zones.invalid_edges.contains(&e.id) && !unavailable.contains(&e.id))
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
    let needed = needed_edges(&doc.nodes, &used, seed_type);
    let mut incoming: HashMap<String, Vec<GraphEdge>> = HashMap::new();
    for e in &needed.edges {
        incoming.entry(e.target.clone()).or_default().push(e.clone());
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
    };
    emitter.emit_scope(state, None, "  ");
    emitter
}

/// Generate one group's `struct` + `fn` (recursively for nested groups).
fn emit_group_function(
    state: &mut ShaderState,
    definition_ref: &str,
    absent_inputs: &BTreeSet<String>,
    constants: &BTreeMap<String, i64>,
    given: &TypeBindings,
    read_outputs: Option<&BTreeSet<String>>,
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
    let key = format!(
        "{definition_ref}|{}|e|{}|t|{type_suffix}|u|{}",
        join(&absent_indices),
        enum_values.join("|"),
        join(&unread_indices)
    );
    if let Some((_, existing)) = state.group_decls.iter().find(|(k, _)| *k == key) {
        return Some(existing.clone());
    }

    let ident = sanitize_ident(definition_ref);
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
    );

    // Struct fields follow the handle types; whatever feeds them is converted
    // on the way in, exactly like any other typed input.
    let output_types = |node: Option<&GraphNode>| {
        node.map(|n| state.graph.resolver.resolve(project, n, &bindings).input_types.clone())
            .unwrap_or_default()
    };
    let outputs: Vec<(&GroupHandleDef, WgslType)> = definition
        .outputs
        .iter()
        .map(|h| {
            let ty = bound_type(&h.ty, variables, &bindings)
                .or_else(|| output_types(definition.output_node_of(&h.id)).get(&h.id).copied())
                .unwrap_or(WgslType::F32);
            (h, ty)
        })
        .collect();
    let inputs: Vec<(&GroupHandleDef, WgslType)> = definition
        .inputs
        .iter()
        .map(|h| (h, bound_type(&h.ty, variables, &bindings).unwrap_or(WgslType::F32)))
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
    let struct_fields: Vec<String> = outputs
        .iter()
        .map(|(h, ty)| format!("  {}: {},", sanitize_ident(&h.id), ty.wgsl_name()))
        .collect();
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

/// Emit the full WGSL fragment shader source from the graph: `frag` returns a colour.
pub fn generate_shader(
    graph: CodegenGraph<'_>,
    options: &ShaderOptions,
) -> Result<ShaderSource, CodegenError> {
    let mut state = ShaderState {
        graph,
        helpers: HelperRegistry::default(),
        group_decls: Vec::new(),
        group_structs: HashSet::new(),
        uniform_scopes: options.uniform_scopes.iter().cloned().collect(),
        uniforms: Vec::new(),
        error: None,
    };
    let root = graph.project.root();
    let root_uniforms = state.uniform_scopes.contains(ROOT_ID);
    let body = emit_document_body(
        &mut state,
        root,
        FRAG_OUTPUT,
        &BTreeSet::new(),
        &BTreeMap::new(),
        root_uniforms,
        &BTreeMap::new(),
        None,
    );
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    let output = root.nodes.iter().find(|n| n.is(FRAG_OUTPUT));
    let result = output
        .filter(|n| body.line_map.contains_key(&n.id))
        .map(|n| var_name(&n.id));

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
    preamble.push("fn frag(uv: vec2f) -> vec4f {".to_owned());
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
        Some(var) => format!("  return {var};"),
        None => format!("  return {};", WgslType::Vec4f.zero()),
    });
    lines.push("}".to_owned());
    let _ = FRAGMENT_VAR;
    Ok(ShaderSource { fragment: lines.join("\n"), uniforms: state.uniforms, line_map })
}

/// The physical line a body entry starts at: entries may hold several lines.
fn line_offset(lines: &[String], index: usize) -> usize {
    lines.iter().take(index).map(|l| l.matches('\n').count() + 1).sum()
}
