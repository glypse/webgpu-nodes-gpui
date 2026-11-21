//! Type inference: every node's input and output types, resolved from what is
//! linked to it, what the user forced and what a group's type variables are
//! bound to.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::descriptors::{NodeDescriptor, NodeInputDef, NodeOutputDef, input_family_key};
use super::node_data::{NodeData, TypeBindings, TypeVariable, bound_type};
use super::values::{WgslType, highest_type};
use crate::enums::available_variants;
use crate::graph::{GROUP_INPUT, GROUP_OUTPUT, GraphEdge, GraphNode, GraphProject, NODE_GROUP};
use crate::nodes::resolve_descriptor;
use crate::types::InputControl;
use crate::zones::{
    inferred_item_family, is_inferred_item, is_zone_input, is_zone_node_type, paired_input,
    paired_output_id, zone_items,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedNodeTypes {
    pub input_types: BTreeMap<String, WgslType>,
    pub output_type: WgslType,
    pub resolved_inputs: Vec<NodeInputDef>,
    pub resolved_outputs: Vec<NodeOutputDef>,
    /// On a group node: the types its definition's variables take for it.
    pub bindings: Option<TypeBindings>,
}

impl ResolvedNodeTypes {
    pub fn input_type(&self, id: &str) -> Option<WgslType> {
        self.input_types.get(id).copied()
    }

    pub fn output_handle_type(&self, id: &str) -> WgslType {
        self.resolved_outputs.iter().find(|o| o.id == id).map_or(self.output_type, |o| o.ty)
    }
}

/// A stable key for `bindings` (the empty string for none).
pub fn bindings_key(bindings: &TypeBindings) -> String {
    bindings.iter().map(|(id, ty)| format!("{id}={ty}")).collect::<Vec<_>>().join(",")
}

/// Every variable's type: the bound one, or its first.
pub fn complete_bindings(variables: &[TypeVariable], bindings: &TypeBindings) -> TypeBindings {
    variables
        .iter()
        .map(|v| {
            let ty = bindings
                .get(&v.id)
                .copied()
                .or_else(|| v.types.first().copied())
                .unwrap_or(WgslType::F32);
            (v.id.clone(), ty)
        })
        .collect()
}

/// Resolves and memoises every node's types across the project's documents.
/// Nodes inside a group with type variables resolve differently for each
/// group node: results are memoised by the bindings of the node's own
/// document's variables, all of them bound. The cache doesn't watch the
/// project: call `clear` after it changes.
#[derive(Default)]
pub struct TypeResolver {
    cache: Mutex<HashMap<(String, String), Arc<ResolvedNodeTypes>>>,
}

impl TypeResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every node's types.
    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    fn variables<'a>(project: &'a GraphProject, node_id: &str) -> &'a [TypeVariable] {
        project.document_of(node_id).map_or(&[], |d| d.type_variables.as_slice())
    }

    fn key(project: &GraphProject, node_id: &str, bindings: &TypeBindings) -> String {
        let variables = Self::variables(project, node_id);
        if variables.is_empty() {
            String::new()
        } else {
            bindings_key(&complete_bindings(variables, bindings))
        }
    }

    /// A node's types, memoised.
    pub fn resolve(
        &self,
        project: &GraphProject,
        node: &GraphNode,
        bindings: &TypeBindings,
    ) -> Arc<ResolvedNodeTypes> {
        let key = (node.id.clone(), Self::key(project, &node.id, bindings));
        if let Some(found) = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return found.clone();
        }
        let result = Arc::new(self.resolve_with(project, node, &node.data, bindings));
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, result.clone());
        result
    }

    /// Resolve types for a node as if it held `data`, bypassing the cache:
    /// e.g. to see what its links infer without the user's type overrides.
    pub fn resolve_with(
        &self,
        project: &GraphProject,
        node: &GraphNode,
        data: &NodeData,
        bindings: &TypeBindings,
    ) -> ResolvedNodeTypes {
        let descriptor = resolve_descriptor(node, project);
        let incoming: Vec<&GraphEdge> =
            project.document_of(&node.id).map_or_else(Vec::new, |d| d.edges_into(&node.id).collect());
        let env = Env { resolver: self, project };
        env.resolve_node(node, &descriptor, &incoming, data, bindings)
    }

    /// The type of an edge's source handle.
    pub fn infer_handle_type(
        &self,
        project: &GraphProject,
        source: &str,
        handle: &str,
        bindings: &TypeBindings,
    ) -> WgslType {
        project.resolve_node(source).map_or(WgslType::F32, |node| {
            self.resolve(project, node, bindings).output_handle_type(handle)
        })
    }
}

struct Env<'a> {
    resolver: &'a TypeResolver,
    project: &'a GraphProject,
}

impl Env<'_> {
    fn resolve_node(
        &self,
        node: &GraphNode,
        descriptor: &NodeDescriptor,
        incoming: &[&GraphEdge],
        data: &NodeData,
        bindings: &TypeBindings,
    ) -> ResolvedNodeTypes {
        if node.is(GROUP_INPUT) || node.is(GROUP_OUTPUT) {
            let doc = self.project.document_of(&node.id);
            let variables = doc.map_or(&[][..], |d| d.type_variables.as_slice());
            let bound = complete_bindings(variables, bindings);
            if node.is(GROUP_OUTPUT) {
                // The variables' families are forced to their bound types.
                let mut forced = data.clone();
                forced.types.extend(bound.iter().map(|(k, v)| (k.clone(), *v)));
                return self.resolve_regular(descriptor, incoming, &forced, &bound);
            }
            let mut resolved = self.resolve_regular(descriptor, incoming, data, &bound);
            resolved.resolved_outputs = doc.map_or_else(Vec::new, |d| {
                d.inputs
                    .iter()
                    .map(|h| {
                        NodeOutputDef::new(
                            h.id.clone(),
                            h.label.clone(),
                            bound_type(&h.ty, variables, &bound).unwrap_or(WgslType::F32),
                        )
                    })
                    .collect()
            });
            return resolved;
        }
        if is_zone_node_type(&node.node_type) {
            return self.resolve_zone(node, descriptor, incoming, data, bindings);
        }
        let resolved = self.resolve_regular(descriptor, incoming, data, bindings);
        if node.is(NODE_GROUP) { self.resolve_group(node, resolved) } else { resolved }
    }

    /// A zone input or output: items typed by a variable take its bound type,
    /// and each item's output carries its input's type. An inferred item takes
    /// the type of its initial value on the zone input, which the zone
    /// output's sockets are forced to: its next value would make the type
    /// depend on itself, through the loop.
    fn resolve_zone(
        &self,
        node: &GraphNode,
        descriptor: &NodeDescriptor,
        incoming: &[&GraphEdge],
        data: &NodeData,
        bindings: &TypeBindings,
    ) -> ResolvedNodeTypes {
        let doc = self.project.document_of(&node.id);
        let variables = doc.map_or(&[][..], |d| d.type_variables.as_slice());
        let bound = complete_bindings(variables, bindings);
        let mut types: BTreeMap<String, WgslType> = data.types.clone();
        types.extend(bound.iter().map(|(k, v)| (k.clone(), *v)));
        if is_zone_input(&node.node_type) {
            let linked: HashSet<&str> = incoming.iter().map(|e| e.target_handle.as_str()).collect();
            let output = paired_output_id(node).and_then(|id| self.project.resolve_node(id));
            for item in zone_items(output) {
                if is_inferred_item(&item) && linked.contains(item.id.as_str()) {
                    types.remove(&inferred_item_family(&item.id));
                }
            }
        } else if let Some(doc) = doc
            && let Some(input) = paired_input(&doc.nodes, &node.id) {
                let paired = self.resolver.resolve(self.project, input, bindings);
                for item in zone_items(Some(node)) {
                    if let (true, Some(ty)) = (is_inferred_item(&item), paired.input_type(&item.id)) {
                        types.insert(inferred_item_family(&item.id), ty);
                    }
                }
            }
        let mut data = data.clone();
        data.types = types;
        let mut resolved = self.resolve_regular(descriptor, incoming, &data, bindings);
        // A texture item's values are colours, its outputs textures.
        for output in &mut resolved.resolved_outputs {
            if output.ty.is_texture() {
                continue;
            }
            if let Some(ty) = resolved.input_types.get(&output.id) {
                output.ty = *ty;
            }
        }
        resolved
    }

    /// A group node's variables and outputs, from its resolved inputs: a
    /// variable takes the type of its sockets' family (forced on the node or
    /// inferred from links), or the one picked on the node when it has no
    /// input socket.
    fn resolve_group(&self, node: &GraphNode, mut resolved: ResolvedNodeTypes) -> ResolvedNodeTypes {
        let Some(def) = node.definition_ref().and_then(|r| self.project.document(r)) else {
            return resolved;
        };
        let mut bindings = TypeBindings::new();
        for variable in &def.type_variables {
            let socket = resolved
                .resolved_inputs
                .iter()
                .find(|i| i.family.as_deref() == Some(&variable.id));
            let ty = match socket {
                Some(socket) => resolved.input_types.get(&socket.id).copied(),
                None => node.data.types.get(&variable.id).copied(),
            };
            let ty = ty
                .filter(|t| !t.is_texture() && variable.types.contains(t))
                .or_else(|| variable.types.first().copied())
                .unwrap_or(WgslType::F32);
            bindings.insert(variable.id.clone(), ty);
        }
        // An inferred output takes the type of what reaches it, on the Group
        // Output node it's linked on.
        let inner_type = |handle: &str| -> Option<WgslType> {
            let output = def.output_node_of(handle)?;
            self.resolver.resolve(self.project, output, &bindings).input_type(handle)
        };
        resolved.resolved_outputs = def
            .outputs
            .iter()
            .map(|h| {
                let ty = bound_type(&h.ty, &def.type_variables, &bindings)
                    .or_else(|| inner_type(&h.id))
                    .unwrap_or(WgslType::F32);
                NodeOutputDef::new(h.id.clone(), h.label.clone(), ty)
            })
            .collect();
        // Enums offer the variants their types have.
        for input in &mut resolved.resolved_inputs {
            let handle = def.inputs.iter().find(|h| h.id == input.id);
            if let (Some(InputControl::Enum { variants }), Some(handle)) =
                (&mut input.control, handle)
            {
                *variants = available_variants(handle, &bindings);
            }
        }
        resolved.bindings = Some(bindings);
        resolved
    }

    fn source_handle_type(&self, edge: &GraphEdge, bindings: &TypeBindings) -> Option<WgslType> {
        let source = self.project.resolve_node(&edge.source)?;
        let resolved = self.resolver.resolve(self.project, source, bindings);
        resolved.resolved_outputs.iter().find(|o| o.id == edge.source_handle).map(|o| o.ty)
    }

    /// The standard type resolution for non-boundary nodes.
    fn resolve_regular(
        &self,
        descriptor: &NodeDescriptor,
        incoming: &[&GraphEdge],
        data: &NodeData,
        bindings: &TypeBindings,
    ) -> ResolvedNodeTypes {
        let overrides = &data.types;

        // Phase 0: a user-selected output type (typeFamily nodes such as
        // "declare") decides which inputs are active.
        let family_output_type = descriptor.type_family.as_ref().map(|family| {
            overrides
                .get(&family.key)
                .copied()
                .filter(|t| !t.is_texture())
                .or_else(|| family.types.first().copied())
                .unwrap_or(WgslType::F32)
        });
        let active_inputs: Vec<NodeInputDef> = match (family_output_type, &descriptor.resolve_inputs)
        {
            (Some(ty), Some(resolve)) => resolve(ty, data),
            _ => descriptor.inputs.clone(),
        };
        let family_of: HashMap<&str, String> =
            active_inputs.iter().map(|i| (i.id.as_str(), input_family_key(i, &active_inputs))).collect();
        let family = |input: &NodeInputDef| family_of[input.id.as_str()].clone();

        // Links from outputs their source doesn't have with these bindings don't count.
        let mut linked: HashMap<String, Vec<WgslType>> = HashMap::new();
        let mut source_types_of = |input_id: &str| -> Vec<WgslType> {
            linked
                .entry(input_id.to_owned())
                .or_insert_with(|| {
                    incoming
                        .iter()
                        .filter(|e| e.target_handle == input_id)
                        .filter_map(|e| self.source_handle_type(e, bindings))
                        .collect()
                })
                .clone()
        };

        // Phase 1: infer each input independently.
        let mut raw: HashMap<String, WgslType> = HashMap::new();
        for input in &active_inputs {
            let ty = if let Some(forced) = overrides.get(&family(input)) {
                *forced
            } else {
                let sources =
                    if input.types.len() == 1 { Vec::new() } else { source_types_of(&input.id) };
                if sources.is_empty() {
                    input.default_type()
                } else {
                    highest_type(&sources, &input.types)
                }
            };
            raw.insert(input.id.clone(), ty);
        }

        // Phase 2: join every input of a family. Unconnected inputs only count
        // when the family is overridden, so a lone i32 source isn't widened to
        // f32 by its unconnected (default-typed) siblings.
        let mut candidates: Vec<(String, Vec<WgslType>, Vec<WgslType>)> = Vec::new();
        for input in &active_inputs {
            let key = family(input);
            let at = candidates.iter().position(|(k, ..)| *k == key).unwrap_or_else(|| {
                candidates.push((key.clone(), input.types.clone(), Vec::new()));
                candidates.len() - 1
            });
            let connected = if input.types.len() == 1 {
                incoming.iter().any(|e| e.target_handle == input.id)
            } else {
                !source_types_of(&input.id).is_empty()
            };
            if connected || overrides.contains_key(&key) {
                candidates[at].2.push(raw[&input.id]);
            }
        }
        let family_resolved: HashMap<String, WgslType> = candidates
            .into_iter()
            .map(|(key, types, found)| (key, highest_type(&found, &types)))
            .collect();

        // Phase 3: all inputs in the same family get the family's resolved type.
        let input_types: BTreeMap<String, WgslType> = active_inputs
            .iter()
            .map(|i| {
                (i.id.clone(), family_resolved.get(&family(i)).copied().unwrap_or(i.default_type()))
            })
            .collect();

        // Phase 4: the output type, from the primary input.
        let primary = descriptor
            .primary_input
            .as_deref()
            .and_then(|id| active_inputs.iter().find(|i| i.id == id))
            .or(active_inputs.first());
        // A texture passes through as it is (reroutes, switches): only values have callbacks.
        let representative = match primary {
            Some(p) => input_types.get(&p.id).copied().unwrap_or(p.default_type()),
            None => descriptor.outputs.first().map_or(WgslType::F32, |o| o.ty),
        };
        let value_input_types = || -> BTreeMap<String, WgslType> {
            input_types.iter().filter(|(_, t)| !t.is_texture()).map(|(k, t)| (k.clone(), *t)).collect()
        };
        let output_type = family_output_type.unwrap_or_else(|| {
            match &descriptor.resolve_output_type {
                Some(resolve) if !representative.is_texture() => {
                    resolve(representative, &value_input_types())
                }
                _ => representative,
            }
        });

        // Phase 5: dynamic inputs and outputs. Outputs carry their concrete
        // type so downstream consumers can read per-handle types directly.
        let raw_outputs = match &descriptor.resolve_outputs {
            Some(resolve) if !representative.is_texture() => resolve(representative),
            _ => descriptor.outputs.clone(),
        };
        let resolved_outputs = if descriptor.static_output_types {
            raw_outputs
        } else {
            raw_outputs.into_iter().map(|o| NodeOutputDef { ty: output_type, ..o }).collect()
        };
        let resolved_inputs = match (&descriptor.resolve_inputs, family_output_type) {
            (Some(resolve), None) if !output_type.is_texture() => resolve(output_type, data),
            _ => active_inputs,
        };
        ResolvedNodeTypes { input_types, output_type, resolved_inputs, resolved_outputs, bindings: None }
    }
}

/// The links among `edges` to sockets their nodes don't have with the types
/// `types_of` gives them: a socket that comes and goes with types may exist
/// for some instances of a generic group and not others, so its links are
/// kept but don't count.
pub fn unavailable_edges<'a>(
    edges: impl IntoIterator<Item = &'a GraphEdge>,
    types_of: impl Fn(&str) -> Option<Arc<ResolvedNodeTypes>>,
) -> HashSet<String> {
    edges
        .into_iter()
        .filter(|e| {
            let source_missing = types_of(&e.source)
                .is_some_and(|t| !t.resolved_outputs.iter().any(|o| o.id == e.source_handle));
            let target_missing = types_of(&e.target)
                .is_some_and(|t| !t.resolved_inputs.iter().any(|i| i.id == e.target_handle));
            source_missing || target_missing
        })
        .map(|e| e.id.clone())
        .collect()
}
