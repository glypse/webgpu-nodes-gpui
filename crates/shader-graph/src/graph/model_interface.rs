//! Editing what a group's inputs say of their types, variants and values.

use super::model::GraphModel;
use super::{GraphDocument, GroupSide};
use crate::enums::{fit_variants, offered_for, variant_types};
use crate::types::{ColorDisplay, GroupHandleDef, TypeOwner, WgslType};

/// `owner` within the types its variable may take, or `None` once it owns
/// none of them, or all: either way it hides nothing.
pub(super) fn fit_type_owner(owner: &TypeOwner, allowed: &[WgslType]) -> Option<TypeOwner> {
    let types: Vec<WgslType> = allowed.iter().copied().filter(|t| owner.types.contains(t)).collect();
    (!types.is_empty() && types.len() != allowed.len())
        .then(|| TypeOwner { variable: owner.variable.clone(), types })
}

impl GraphModel {
    /// Edit a vec3f / vec4f group input as a colour, or as numbers with `None`.
    pub fn set_handle_display(&mut self, definition_ref: &str, handle_id: &str, display: Option<ColorDisplay>) {
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| h.display = display);
    }

    /// Show a group input only while one of the group's type variables has
    /// one of some types, or whatever the types with `None`.
    pub fn set_handle_type_owner(&mut self, definition_ref: &str, handle_id: &str, type_owner: Option<TypeOwner>) {
        let Some(def) = self.project.document(definition_ref) else { return };
        let variables = def.type_variables.clone();
        let owner = match &type_owner {
            None => None,
            Some(o) => {
                let Some(variable) = variables.iter().find(|v| v.id == o.variable) else { return };
                if !variable.types.iter().any(|t| o.types.contains(t)) {
                    return;
                }
                fit_type_owner(o, &variable.types)
            }
        };
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| {
            h.type_owner = owner;
            *h = fit_variants(h, &variables);
        });
    }

    /// Decide which variants of a group's enum input are offered by the type
    /// of one of the group's type variables, all of them to begin with; or,
    /// with `None`, offer them whatever the types.
    pub fn set_variants_by(&mut self, definition_ref: &str, handle_id: &str, variable_id: Option<&str>) {
        let Some(def) = self.project.document(definition_ref) else { return };
        if variable_id.is_some_and(|id| !def.type_variables.iter().any(|v| v.id == id)) {
            return;
        }
        let by = variable_id.map(str::to_owned);
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| {
            let Some(variants) = &mut h.variants else { return };
            for v in variants {
                v.types = None;
            }
            h.variants_by = by;
        });
    }

    /// Offer a variant of an enum input for a type of its `variants_by`
    /// variable, or stop. Each type keeps a variant: taking the last one away
    /// does nothing.
    pub fn set_variant_offered(
        &mut self,
        definition_ref: &str,
        handle_id: &str,
        value: f64,
        ty: WgslType,
        offered: bool,
    ) {
        let Some(def) = self.project.document(definition_ref) else { return };
        let variables = def.type_variables.clone();
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| {
            let Some(variable) = variables.iter().find(|v| Some(&v.id) == h.variants_by.as_ref()) else {
                return;
            };
            let variants = h.variants.clone().unwrap_or_default();
            if !offered && !variants.iter().any(|v| v.value != value && offered_for(v, Some(ty))) {
                return;
            }
            let all = variant_types(h, variable);
            if let Some(variants) = &mut h.variants {
                for v in variants.iter_mut().filter(|v| v.value == value) {
                    let mut others: Vec<WgslType> =
                        v.types.clone().unwrap_or_else(|| all.clone()).into_iter().filter(|t| *t != ty).collect();
                    if offered {
                        others.push(ty);
                    }
                    v.types = Some(others);
                }
            }
            *h = fit_variants(h, &variables);
        });
    }

    /// Compile a group's integer input in, as an enum's value is, or pass it
    /// as a uniform again.
    pub fn set_handle_compiled(&mut self, definition_ref: &str, handle_id: &str, compiled: bool) {
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| {
            h.compiled = (compiled && h.can_compile()).then_some(true);
        });
    }

    /// Keep a group input's value within `min` and `max` while edited on
    /// group nodes (`None` for no bound).
    pub fn set_handle_range(&mut self, definition_ref: &str, handle_id: &str, min: Option<f64>, max: Option<f64>) {
        self.update_handle(definition_ref, GroupSide::Input, handle_id, |h| {
            h.min = min;
            h.max = max;
        });
    }
}

/// Fit what inputs and variants say of types to the variables of `def`, as they now are.
pub(super) fn fit_type_owners(def: &mut GraphDocument) {
    let variables = def.type_variables.clone();
    if !def.inputs.iter().any(|h| h.type_owner.is_some() || h.variants_by.is_some()) {
        return;
    }
    for h in &mut def.inputs {
        if let Some(owner) = &h.type_owner {
            let variable = variables.iter().find(|v| v.id == owner.variable);
            h.type_owner = variable.and_then(|v| fit_type_owner(owner, &v.types));
        }
        let fitted: GroupHandleDef = fit_variants(h, &variables);
        *h = fitted;
    }
}

impl GraphModel {
    /// Remove links into Match cases their enum no longer has: a variant was
    /// removed, or another enum (or none) was linked to the node.
    pub(super) fn drop_stale_case_links(&mut self) {
        let keys: Vec<String> = self.project.definitions.keys().cloned().collect();
        for key in keys {
            let Some(doc) = self.project.document(&key) else { continue };
            let stale: Vec<String> = doc
                .edges
                .iter()
                .filter(|e| crate::enums::is_case_handle(&e.target_handle))
                .filter(|e| {
                    let Some(node) = doc.node(&e.target).filter(|n| n.is(crate::enums::MATCH)) else {
                        return false;
                    };
                    !crate::nodes::resolve_descriptor(node, &self.project)
                        .inputs
                        .iter()
                        .any(|i| i.id == e.target_handle)
                })
                .map(|e| e.id.clone())
                .collect();
            if let Some(doc) = self.project.definitions.get_mut(&key).filter(|_| !stale.is_empty()) {
                doc.edges.retain(|e| !stale.contains(&e.id));
            }
        }
    }
}
