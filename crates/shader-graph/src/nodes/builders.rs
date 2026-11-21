//! Factories for the node shapes many types share.

use crate::types::{
    FLOAT_TYPES, NUMERIC_TYPES, NodeCategory, NodeData, NodeDescriptor, NodeInputDef,
    NodeOutputDef, NodeSetting, ScalarKind, WgslCtx, WgslType, make_type,
};

pub(crate) fn float_values<const N: usize>(values: [(&'static str, f64); N]) -> NodeData {
    NodeData::with_values(values)
}

fn result_output() -> Vec<NodeOutputDef> {
    vec![NodeOutputDef::new("result", "result", WgslType::F32)]
}

/// One input `e`, one output, `fn(e)`: sin, abs, floor...
pub fn unary_fn(
    ty: &str,
    label: &str,
    wgsl_fn: &'static str,
    types: &[WgslType],
    category: NodeCategory,
) -> NodeDescriptor {
    NodeDescriptor::new(ty, label, category)
        .inputs(vec![NodeInputDef::new("e", "e", types)])
        .outputs(result_output())
        .defaults(float_values([("e", 0.0)]))
        .wgsl(move |ctx| {
            let e = ctx.promote_input("e");
            vec![format!("let {} = {wgsl_fn}({e});", ctx.var_name)]
        })
}

/// A variadic operator joining its operands: add, multiply.
pub fn variadic_op(ty: &str, label: &str, joiner: &'static str) -> NodeDescriptor {
    NodeDescriptor::new(ty, label, NodeCategory::Math)
        .inputs(vec![NodeInputDef::new("operands", "operands", NUMERIC_TYPES).variadic()])
        .outputs(result_output())
        .wgsl(move |ctx| {
            let list = ctx.operands("operands");
            if list.is_empty() {
                return vec![format!("let {} = {};", ctx.var_name, ctx.output_type.zero())];
            }
            // Variadic operands are pre-converted to the resolved type.
            vec![format!("let {} = {};", ctx.var_name, list.join(joiner))]
        })
}

/// A variadic function folded over its operands: min, max.
pub fn variadic_fn(ty: &str, label: &str, wgsl_fn: &'static str) -> NodeDescriptor {
    NodeDescriptor::new(ty, label, NodeCategory::Math)
        .inputs(vec![NodeInputDef::new("operands", "operands", NUMERIC_TYPES).variadic()])
        .outputs(result_output())
        .wgsl(move |ctx| {
            let list = ctx.operands("operands");
            let Some((first, rest)) = list.split_first() else {
                return vec![format!("let {} = {};", ctx.var_name, ctx.output_type.zero())];
            };
            let nested = rest.iter().fold(first.clone(), |acc, cur| format!("{wgsl_fn}({acc}, {cur})"));
            vec![format!("let {} = {nested};", ctx.var_name)]
        })
}

pub struct BinaryOptions {
    pub types: &'static [WgslType],
    pub labels: Option<[&'static str; 2]>,
    pub ids: [&'static str; 2],
    pub category: NodeCategory,
}

impl Default for BinaryOptions {
    fn default() -> Self {
        Self { types: NUMERIC_TYPES, labels: None, ids: ["a", "b"], category: NodeCategory::Math }
    }
}

/// Two inputs combined by an expression: subtract, step, atan2...
pub fn binary_node(
    ty: &str,
    label: &str,
    emit: impl Fn(&str, &str, &mut WgslCtx<'_>) -> String + Send + Sync + 'static,
    opts: BinaryOptions,
) -> NodeDescriptor {
    let [id_a, id_b] = opts.ids;
    let [label_a, label_b] = opts.labels.unwrap_or(opts.ids);
    NodeDescriptor::new(ty, label, opts.category)
        .inputs(vec![
            NodeInputDef::new(id_a, label_a, opts.types),
            NodeInputDef::new(id_b, label_b, opts.types),
        ])
        .outputs(result_output())
        .defaults(float_values([(id_a, 0.0), (id_b, 0.0)]))
        .wgsl(move |ctx| {
            let a = ctx.promote_input(id_a);
            let b = ctx.promote_input(id_b);
            let expr = emit(&a, &b, ctx);
            vec![format!("let {} = {expr};", ctx.var_name)]
        })
}

/// `fn(a, b, c...)` over the given inputs: clamp, mix.
pub fn n_ary_fn(
    ty: &str,
    label: &str,
    wgsl_fn: &str,
    inputs: Vec<NodeInputDef>,
    defaults: &[(&'static str, f64)],
) -> NodeDescriptor {
    let wgsl_fn = wgsl_fn.to_owned();
    let ids: Vec<String> = inputs.iter().map(|i| i.id.clone()).collect();
    NodeDescriptor::new(ty, label, NodeCategory::Math)
        .inputs(inputs)
        .outputs(result_output())
        .defaults(NodeData::with_values(defaults.iter().copied()))
        .wgsl(move |ctx| {
            let args: Vec<String> = ids.iter().map(|id| ctx.promote_input(id)).collect();
            vec![format!("let {} = {wgsl_fn}({});", ctx.var_name, args.join(", "))]
        })
}

/// An input reading a value the shader is given.
pub fn uniform_reader(
    ty: &str,
    label: &str,
    output: &'static str,
    output_type: WgslType,
    expr: String,
) -> NodeDescriptor {
    NodeDescriptor::new(ty, label, NodeCategory::Input)
        .outputs(vec![NodeOutputDef::new(output, output, output_type)])
        .wgsl(move |ctx| {
            vec![format!("let {}: {} = {expr};", ctx.var_name, output_type.wgsl_name())]
        })
}

pub fn rounding_setting() -> NodeSetting {
    NodeSetting::new(
        "rounding",
        "Rounding",
        &[("trunc", "Truncate"), ("floor", "Floor"), ("ceil", "Ceil"), ("round", "Round")],
    )
}

/// Conversion to a scalar kind: To Float, To Integer...
pub fn convert_node(ty: &str, label: &str, kind: ScalarKind, rounding: bool) -> NodeDescriptor {
    let setting = rounding_setting();
    let mut data = float_values([("e", 0.0)]);
    if rounding {
        data = data.setting("rounding", "trunc");
    }
    let mut descriptor = NodeDescriptor::new(ty, label, NodeCategory::Converter)
        .inputs(vec![NodeInputDef::new("e", "e", &WgslType::ALL)])
        .outputs(result_output())
        .defaults(data)
        .output_type(move |t, _| make_type(kind, t.components()));
    if rounding {
        descriptor = descriptor.settings(vec![setting.clone()]);
    }
    descriptor
        .cpu(move |ctx| {
            let mode = if rounding { setting_mode(ctx.data.settings.get("rounding")) } else { "trunc" };
            let round: fn(f64) -> f64 = match mode {
                "floor" => f64::floor,
                "ceil" => f64::ceil,
                "round" => super::cpu::round_half_even,
                _ => f64::trunc,
            };
            let e = ctx.input("e");
            let rounded: Vec<f64> =
                if rounding { e.into_iter().map(round).collect() } else { e };
            super::cpu::result(super::cpu::coerce(&rounded, ctx.output_type))
        })
        .wgsl(move |ctx| {
        let source = ctx.input_type("e");
        let mut expr = ctx.promote_input("e");
        if rounding && source.kind() == ScalarKind::F32 {
            expr = format!("{}({expr})", setting.value(&ctx.data));
        }
        let converted = crate::types::convert_var(&expr, source, ctx.output_type);
        vec![format!("let {} = {converted};", ctx.var_name)]
    })
}

fn setting_mode(stored: Option<&String>) -> &str {
    stored.map_or("trunc", String::as_str)
}

pub const FLOATS: &[WgslType] = FLOAT_TYPES;
