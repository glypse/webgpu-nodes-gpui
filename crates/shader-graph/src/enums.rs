//! Enums: group inputs choosing among variants, and the nodes branching on
//! them while compiling (Match, Width Switch).

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::{GROUP_INPUT, GraphDocument, GraphEdge, GraphNode, GraphProject, REROUTE};
use crate::types::{
    EnumVariant, GroupHandleDef, InputOwner, TypeBindings, TypeOwner,
    TypeVariable, WgslType, enum_value, InputValue,
};

pub const MATCH: &str = "match";
pub const MATCH_SELECTOR: &str = "selector";
pub const WIDTH_SWITCH: &str = "width-switch";
pub const WIDTH_SWITCH_VALUE: &str = "value";
/// The family of a Match or Index Switch node's cases: one type for all of them.
pub const CASES_FAMILY: &str = "cases";
const CASE_PREFIX: &str = "case-";

/// A Width Switch node's cases: one per component count.
pub fn widths() -> Vec<EnumVariant> {
    vec![
        EnumVariant::new(1, "Scalar"),
        EnumVariant::new(2, "Vector 2"),
        EnumVariant::new(3, "Vector 3"),
        EnumVariant::new(4, "Vector 4"),
    ]
}

/// The input of a Match node taking the value of one variant.
pub fn case_handle(value: i64) -> String {
    format!("{CASE_PREFIX}{value}")
}

pub fn is_case_handle(handle: &str) -> bool {
    handle.starts_with(CASE_PREFIX)
}

/// A value for a new variant: one no variant of `variants` has.
pub fn next_variant_value(variants: &[EnumVariant]) -> i64 {
    variants.iter().map(|v| v.value.saturating_add(1)).max().unwrap_or(0).max(0)
}

/// The type an input of a node resolves to.
pub type InputTypeFn<'a> = dyn Fn(&str, &str) -> Option<WgslType> + 'a;

/// What enum tracing reads of a document.
pub struct EnumDocument<'a> {
    pub nodes: &'a [GraphNode],
    pub edges: &'a [GraphEdge],
    /// The inputs of the group the document is, which its Group Input nodes show.
    pub inputs: &'a [GroupHandleDef],
    /// The type an input of a node resolves to, for the case a Width Switch
    /// picks. Without it every case is used.
    pub input_type: Option<&'a InputTypeFn<'a>>,
}

impl<'a> EnumDocument<'a> {
    pub fn of(doc: &'a GraphDocument) -> Self {
        Self { nodes: &doc.nodes, edges: &doc.edges, inputs: &doc.inputs, input_type: None }
    }

    fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn feed(&self, node: &str, input: &str) -> Option<&GraphEdge> {
        self.edges.iter().find(|e| e.target == node && e.target_handle == input)
    }

    /// The enum an output carries: a group input's enum handle, through nodes
    /// passing values unchanged.
    pub fn enum_of_output(&self, node: &str, handle: &str) -> Option<&[EnumVariant]> {
        let node = self.node(node)?;
        if node.is(GROUP_INPUT) {
            return self.inputs.iter().find(|h| h.id == handle)?.variants.as_deref();
        }
        if node.is(REROUTE) {
            return self.enum_at(&node.id, "in");
        }
        None
    }

    /// The enum linked to an input, if any.
    pub fn enum_at(&self, node: &str, input: &str) -> Option<&[EnumVariant]> {
        let edge = self.feed(node, input)?;
        self.enum_of_output(&edge.source, &edge.source_handle)
    }

    /// The value an input has while compiling, when it comes from a group
    /// input whose value is known (`constants` holds those, by handle id).
    pub fn constant_at(
        &self,
        node: &str,
        input: &str,
        constants: &BTreeMap<String, i64>,
    ) -> Option<i64> {
        let edge = self.feed(node, input)?;
        let source = self.node(&edge.source)?;
        if source.is(GROUP_INPUT) {
            return constants.get(&edge.source_handle).copied();
        }
        if source.is(REROUTE) {
            return self.constant_at(&edge.source, "in", constants);
        }
        None
    }

    /// The case input a Match node uses while compiling, when its enum's value
    /// is known. A value matching no variant picks the first one.
    pub fn matched_case(
        &self,
        node: &str,
        constants: &BTreeMap<String, i64>,
    ) -> Option<String> {
        let value = self.constant_at(node, MATCH_SELECTOR, constants)?;
        let variants = self.enum_at(node, MATCH_SELECTOR).unwrap_or(&[]);
        let variant = variants.iter().find(|v| v.value == value).or(variants.first())?;
        Some(case_handle(variant.value))
    }

    /// The case input a Width Switch node uses: that of its value's component count.
    pub fn width_case(&self, node: &str) -> Option<String> {
        let ty = self.input_type?(node, WIDTH_SWITCH_VALUE)?;
        Some(case_handle(ty.components() as i64))
    }

    /// `edges` without those whose value is never used while compiling with
    /// `constants`: links into the cases a Match or Width Switch doesn't pick.
    pub fn live_edges(&self, edges: &[GraphEdge], constants: &BTreeMap<String, i64>) -> Vec<GraphEdge> {
        let mut picked: BTreeMap<String, Option<String>> = BTreeMap::new();
        edges
            .iter()
            .filter(|e| {
                if !is_case_handle(&e.target_handle) {
                    return true;
                }
                let chosen = picked.entry(e.target.clone()).or_insert_with(|| {
                    match self.node(&e.target).map(|n| n.node_type.as_str()) {
                        Some(MATCH) => self.matched_case(&e.target, constants),
                        Some(WIDTH_SWITCH) => self.width_case(&e.target),
                        _ => None,
                    }
                });
                chosen.as_ref().is_none_or(|c| *c == e.target_handle)
            })
            .cloned()
            .collect()
    }
}

/// The variants `handle` lies under, nearest first: its owner, its enum's owner...
pub fn owner_chain(handles: &[GroupHandleDef], handle: &GroupHandleDef) -> Vec<InputOwner> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut owner = handle.owner.clone();
    while let Some(o) = owner {
        if !seen.insert(o.enum_id.clone()) {
            break;
        }
        owner = handles.iter().find(|h| h.id == o.enum_id).and_then(|h| h.owner.clone());
        chain.push(o);
    }
    chain
}

/// Whether `handle` lies under `owner` (any value when `value` is `None`), at any depth.
pub fn is_under(
    handles: &[GroupHandleDef],
    handle: &GroupHandleDef,
    enum_id: &str,
    value: Option<i64>,
) -> bool {
    owner_chain(handles, handle)
        .iter()
        .any(|o| o.enum_id == enum_id && value.is_none_or(|v| o.value == v))
}

/// Whether an owner names an enum among `handles`, and one of its variants.
pub fn owner_exists(handles: &[GroupHandleDef], owner: Option<&InputOwner>) -> bool {
    owner.is_some_and(|o| {
        handles
            .iter()
            .find(|h| h.id == o.enum_id)
            .and_then(|h| h.variants.as_ref())
            .is_some_and(|vs| vs.iter().any(|v| v.value == o.value))
    })
}

fn owned_by(owner: Option<&TypeOwner>, bindings: &TypeBindings) -> bool {
    match (owner, owner.and_then(|o| bindings.get(&o.variable))) {
        (Some(owner), Some(ty)) => owner.types.contains(ty),
        _ => true,
    }
}

/// Whether `variant` is offered with its enum's variable on `ty`.
pub fn offered_for(variant: &EnumVariant, ty: Option<WgslType>) -> bool {
    match (&variant.types, ty) {
        (Some(types), Some(ty)) => types.contains(&ty),
        _ => true,
    }
}

fn offered_always(variant: &EnumVariant) -> EnumVariant {
    EnumVariant { types: None, ..variant.clone() }
}

/// The variants of an enum offered with its group's variables bound to `bindings`.
pub fn available_variants(handle: &GroupHandleDef, bindings: &TypeBindings) -> Vec<EnumVariant> {
    let ty = handle.variants_by.as_ref().and_then(|v| bindings.get(v)).copied();
    handle
        .variants
        .iter()
        .flatten()
        .filter(|v| offered_for(v, ty))
        .cloned()
        .collect()
}

/// The types of `variable` an enum is shown for, each needing a variant to offer.
pub fn variant_types(handle: &GroupHandleDef, variable: &TypeVariable) -> Vec<WgslType> {
    match &handle.type_owner {
        Some(owner) if owner.variable == variable.id => {
            variable.types.iter().copied().filter(|t| owner.types.contains(t)).collect()
        }
        _ => variable.types.clone(),
    }
}

/// `handle` with its variants' types fitted to `variables`: within those it's
/// shown for, a type none would offer is offered by all. Without its
/// variable, its variants are offered whatever the types.
pub fn fit_variants(handle: &GroupHandleDef, variables: &[TypeVariable]) -> GroupHandleDef {
    let Some(by) = &handle.variants_by else { return handle.clone() };
    let mut fitted = handle.clone();
    let variants = handle.variants.clone().unwrap_or_default();
    let Some(variable) = variables.iter().find(|v| &v.id == by) else {
        fitted.variants = Some(variants.iter().map(offered_always).collect());
        fitted.variants_by = None;
        return fitted;
    };
    let types = variant_types(handle, variable);
    let unoffered: Vec<_> = types
        .iter()
        .copied()
        .filter(|t| !variants.iter().any(|v| offered_for(v, Some(*t))))
        .collect();
    fitted.variants = Some(
        variants
            .iter()
            .map(|variant| {
                let Some(offered) = &variant.types else { return variant.clone() };
                let kept: Vec<_> = types
                    .iter()
                    .copied()
                    .filter(|t| offered.contains(t) || unoffered.contains(t))
                    .collect();
                let mut rest = offered_always(variant);
                if kept.len() != types.len() {
                    rest.types = Some(kept);
                }
                rest
            })
            .collect(),
    );
    fitted
}

fn type_owners(handles: &[GroupHandleDef], handle: &GroupHandleDef) -> Vec<TypeOwner> {
    let enums = owner_chain(handles, handle)
        .into_iter()
        .filter_map(|o| handles.iter().find(|h| h.id == o.enum_id));
    std::iter::once(handle)
        .chain(enums)
        .filter_map(|h| h.type_owner.clone())
        .collect()
}

/// The inputs a group node doesn't show with its enums on `values` and its
/// type variables bound to `bindings`: those under a variant one of them
/// isn't on, or owned by types its variable doesn't have (or under an enum
/// that is). An enum whose value is unknown shows all its variants' inputs.
pub fn hidden_inputs_of(
    handles: &[GroupHandleDef],
    values: &BTreeMap<String, i64>,
    bindings: &TypeBindings,
) -> BTreeSet<String> {
    handles
        .iter()
        .filter(|handle| {
            let off_variant = owner_chain(handles, handle)
                .iter()
                .any(|o| values.get(&o.enum_id).is_some_and(|v| *v != o.value));
            let off_type =
                type_owners(handles, handle).iter().any(|o| !owned_by(Some(o), bindings));
            off_variant || off_type
        })
        .map(|h| h.id.clone())
        .collect()
}

/// Whether two inputs are never shown together: they lie under different
/// variants of an enum, or are owned by none of the same types of a variable.
pub fn shown_apart(handles: &[GroupHandleDef], a: &GroupHandleDef, b: &GroupHandleDef) -> bool {
    let of_b = owner_chain(handles, b);
    let variants = owner_chain(handles, a)
        .iter()
        .any(|o| of_b.iter().any(|p| p.enum_id == o.enum_id && p.value != o.value));
    let owners_b = type_owners(handles, b);
    let types = type_owners(handles, a).iter().any(|o| {
        owners_b
            .iter()
            .any(|p| p.variable == o.variable && !p.types.iter().any(|t| o.types.contains(t)))
    });
    variants || types
}

/// The values a group node sets while compiling, by its definition's handle
/// id: those of unlinked enum and `compiled` inputs (its own value, or the
/// group's default), and of linked ones fed by a value known in `doc`.
pub fn group_constants(
    doc: &EnumDocument<'_>,
    node: &GraphNode,
    project: &GraphProject,
    bindings: &TypeBindings,
    constants: &BTreeMap<String, i64>,
) -> BTreeMap<String, i64> {
    let definition = node.definition_ref().and_then(|r| project.document(r));
    let mut values = BTreeMap::new();
    let Some(definition) = definition else { return values };
    for handle in &definition.inputs {
        if handle.variants.is_none() && handle.compiled != Some(true) {
            continue;
        }
        let linked = doc.edges.iter().any(|e| e.target == node.id && e.target_handle == handle.id);
        let known = if linked {
            doc.constant_at(&node.id, &handle.id, constants)
        } else {
            let stored: Option<&InputValue> =
                node.data.values.get(&handle.id).or_else(|| definition.defaults.get(&handle.id));
            Some(stored.map_or(0, InputValue::as_i64))
        };
        let Some(known) = known else { continue };
        let value = if handle.variants.is_some() {
            enum_value(Some(&InputValue::from(known as f64)), &available_variants(handle, bindings))
        } else {
            Some(known)
        };
        if let Some(value) = value {
            values.insert(handle.id.clone(), value);
        }
    }
    values
}
