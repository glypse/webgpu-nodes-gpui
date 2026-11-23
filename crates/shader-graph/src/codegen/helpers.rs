use std::collections::BTreeMap;

use crate::types::{HelperName, WgslType};

// WGSL built-ins (sin, min, clamp, ...) and operators already work on every
// vector type. Helpers cover the gaps: guarded division / modulo / pow that
// must not produce inf/NaN (or shader-creation errors on constant operands).
// Each helper is overloaded per concrete type; the code generator records the
// requests and only emits the overloads the shader actually uses.

fn body(op: HelperName, t: &str) -> String {
    match op {
        HelperName::SafeDiv => format!("select(a / b, {t}(), b == {t}())"),
        HelperName::SafeMod => format!("select(a % b, {t}(), b == {t}())"),
        // x^0 = 1, where WGSL may give NaN.
        HelperName::SafePow => format!("select(pow(a, b), {t}(1), b == {t}())"),
    }
}

fn prefix(op: HelperName) -> &'static str {
    match op {
        HelperName::SafeDiv => "safeDiv",
        HelperName::SafeMod => "safeMod",
        HelperName::SafePow => "safePow",
    }
}

/// The name of a helper's overload for `ty`.
pub fn helper_fn_name(op: HelperName, ty: WgslType) -> String {
    let id = ty.id();
    let (head, tail) = id.split_at(1);
    format!("{}{}{tail}", prefix(op), head.to_uppercase())
}

fn emit_helper(op: HelperName, ty: WgslType) -> String {
    let t = ty.wgsl_name();
    format!(
        "fn {}(a: {t}, b: {t}) -> {t} {{\n  return {};\n}}",
        helper_fn_name(op, ty),
        body(op, &t)
    )
}

/// Collects the helper overloads and functions requested while generating one shader.
#[derive(Default)]
pub struct HelperRegistry {
    used: BTreeMap<String, (HelperName, WgslType)>,
    functions: BTreeMap<String, String>,
}

impl HelperRegistry {
    pub fn use_helper(&mut self, op: HelperName, ty: WgslType) -> String {
        let name = helper_fn_name(op, ty);
        self.used.entry(name.clone()).or_insert((op, ty));
        name
    }

    /// Declare a function by name; the first source declared under it stays.
    pub fn define(&mut self, name: &str, source: &str) -> String {
        self.functions
            .entry(name.to_owned())
            .or_insert_with(|| source.to_owned());
        name.to_owned()
    }

    /// Every requested overload and function, in a stable (sorted) order.
    pub fn emit(&self) -> Vec<String> {
        let mut all: BTreeMap<&str, String> = BTreeMap::new();
        for (name, (op, ty)) in &self.used {
            all.insert(name, emit_helper(*op, *ty));
        }
        for (name, source) in &self.functions {
            all.insert(name, source.clone());
        }
        all.into_values().collect()
    }
}
