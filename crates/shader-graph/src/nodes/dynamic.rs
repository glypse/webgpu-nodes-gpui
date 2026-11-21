//! Node types whose sockets depend on the node or its document: Match,
//! Index Switch, Custom WGSL, group nodes and their boundaries.

use std::collections::BTreeSet;

use crate::enums::{CASES_FAMILY, MATCH, MATCH_SELECTOR, WIDTH_SWITCH, WIDTH_SWITCH_VALUE, case_handle, widths};
use crate::graph::{GraphDocument, GraphNode, GraphProject};
use crate::types::{
    CustomSockets, EnumVariant, GroupHandleDef, HandleDef, HandleType, InputControl, InputValue,
    NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef, NodeWidget,
    TypeVariable, WgslType, bound_type, enum_value,
};

pub const INDEX_SWITCH: &str = "index-switch";
pub const INDEX_SWITCH_INDEX: &str = "index";
pub const CUSTOM_WGSL: &str = "custom-wgsl";
pub const DEFAULT_CASES: u32 = 2;
pub const MAX_CASES: u32 = 1024;

fn result_output() -> Vec<NodeOutputDef> {
    vec![NodeOutputDef::new("result", "result", WgslType::F32)]
}

/// A count of cases a node can have, the nearest to `count`.
pub fn clamp_cases(count: i64) -> u32 {
    count.clamp(1, i64::from(MAX_CASES)) as u32
}

/// Pick a value by an enum's variant, one input per variant: the cases follow
/// the enum linked to it, so none can be missing or left over (as Rust's
/// `match`). When the enum's value is known while compiling, only the picked
/// case is computed.
pub fn build_match_descriptor(variants: &[EnumVariant]) -> NodeDescriptor {
    let cases: Vec<(String, i64)> =
        variants.iter().map(|v| (case_handle(v.int()), v.int())).collect();
    let mut inputs = vec![NodeInputDef::new(MATCH_SELECTOR, "enum", &[WgslType::U32])];
    inputs.extend(
        variants
            .iter()
            .map(|v| NodeInputDef::new(case_handle(v.int()), v.label.clone(), &WgslType::ALL).family(CASES_FAMILY)),
    );
    let mut d = NodeDescriptor::new(MATCH, "Match", NodeCategory::Logic)
        .aliases(&["Menu Switch", "Switch Case", "Case", "Enum Switch", "Index Switch"])
        .inputs(inputs)
        .outputs(result_output())
        .wgsl(move |ctx| {
            let Some(((first, _), rest)) = cases.split_first() else {
                let zero = crate::types::convert_var("0.0", WgslType::F32, ctx.output_type);
                return vec![format!("let {} = {zero};", ctx.var_name)];
            };
            if let Some(value) = ctx.constant(MATCH_SELECTOR) {
                let picked = cases.iter().find(|(_, v)| *v == value).map_or(first, |(id, _)| id);
                let e = ctx.promote_input(picked);
                return vec![format!("let {} = {e};", ctx.var_name)];
            }
            let selector = ctx.promote_input(MATCH_SELECTOR);
            // Unknown values get the first case.
            let mut chosen = ctx.promote_input(first);
            for (id, value) in rest {
                let case = ctx.promote_input(id);
                chosen = format!("select({chosen}, {case}, {selector} == {value}u)");
            }
            vec![format!("let {} = {chosen};", ctx.var_name)]
        });
    d.primary_input = variants.first().map(|v| case_handle(v.int()));
    d
}

/// Pick a case by how many components a value's type has, known while
/// compiling: only that case is computed, and the result has its type, so
/// cases may differ in type.
pub fn width_switch_node() -> NodeDescriptor {
    let mut inputs = vec![NodeInputDef::new(WIDTH_SWITCH_VALUE, "value", &WgslType::ALL)];
    inputs.extend(
        widths()
            .iter()
            .map(|w| NodeInputDef::new(case_handle(w.int()), w.label.clone(), &WgslType::ALL).family(case_handle(w.int()))),
    );
    NodeDescriptor::new(WIDTH_SWITCH, "Width Switch", NodeCategory::Logic)
        .aliases(&["Type Switch", "Dimension Switch", "Switch by Width", "Component Count Switch"])
        .inputs(inputs)
        .outputs(result_output())
        .output_type(|ty, types| {
            types.get(&case_handle(ty.components() as i64)).copied().unwrap_or(WgslType::F32)
        })
        .wgsl(|ctx| {
            let case = case_handle(ctx.input_type(WIDTH_SWITCH_VALUE).components() as i64);
            let e = ctx.promote_input(&case);
            vec![format!("let {} = {e};", ctx.var_name)]
        })
}

pub fn index_switch_cases(data: &NodeData) -> u32 {
    clamp_cases(i64::from(data.cases.unwrap_or(DEFAULT_CASES)))
}

/// Pick one of its cases by a runtime index, as Blender's Index Switch. An
/// index out of range takes the nearest case.
pub fn build_index_switch_descriptor(data: &NodeData) -> NodeDescriptor {
    let count = index_switch_cases(data);
    let cases: Vec<String> = (0..count).map(|i| case_handle(i64::from(i))).collect();
    let mut inputs = vec![NodeInputDef::new(INDEX_SWITCH_INDEX, "index", &[WgslType::I32])];
    inputs.extend(
        cases
            .iter()
            .enumerate()
            .map(|(i, id)| NodeInputDef::new(id.clone(), i.to_string(), &WgslType::ALL).family(CASES_FAMILY)),
    );
    let defaults = NodeData { cases: Some(DEFAULT_CASES), ..Default::default() };
    let mut d = NodeDescriptor::new(INDEX_SWITCH, "Index Switch", NodeCategory::Logic)
        .aliases(&["Table", "Lookup Table", "Array", "Select by Index", "Pick"])
        .inputs(inputs)
        .outputs(result_output())
        .defaults(defaults)
        .wgsl(move |ctx| {
            let index = ctx.promote_input(INDEX_SWITCH_INDEX);
            if count == 1 {
                let e = ctx.promote_input(&cases[0]);
                return vec![format!("let {} = {e};", ctx.var_name)];
            }
            // A `var`: every WGSL compiler indexes those at runtime, not all do `let` arrays.
            let table = format!("{}_cases", ctx.var_name);
            let ty = ctx.output_type.wgsl_name();
            let values: Vec<String> = cases.iter().map(|id| ctx.promote_input(id)).collect();
            vec![
                format!("var {table} = array<{ty}, {count}>({});", values.join(", ")),
                format!("let {} = {table}[clamp({index}, 0, {})];", ctx.var_name, count - 1),
            ]
        });
    d.primary_input = Some(case_handle(0));
    d.widget = Some(NodeWidget::Cases);
    d
}

// ── Custom WGSL ──────────────────────────────────────────────────────────────

pub fn default_sockets() -> CustomSockets {
    CustomSockets {
        inputs: vec![HandleDef::new("uv", "uv", WgslType::Vec2f)],
        outputs: vec![HandleDef::new("color", "color", WgslType::Vec3f)],
    }
}

pub const DEFAULT_CODE: &str = "color = vec3f(uv, 0.5);";

const RESERVED: &[&str] = &[
    "alias", "bitcast", "bool", "break", "case", "const", "const_assert", "continue", "continuing",
    "default", "diagnostic", "discard", "else", "enable", "f16", "f32", "false", "fn", "for",
    "i32", "if", "let", "loop", "override", "requires", "return", "struct", "switch", "true",
    "u32", "var", "while",
];

/// Whether `name` can name a socket: a WGSL identifier (no leading `__`) that
/// isn't a keyword or a type.
pub fn is_socket_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let valid = starts && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    let vector = ["vec", "mat"].iter().any(|p| {
        name.strip_prefix(p).is_some_and(|rest| {
            let rest = rest.trim_end_matches(['f', 'h', 'i', 'u']);
            matches!(rest, "2" | "3" | "4") || (*p == "mat" && rest.len() == 3 && rest.as_bytes()[1] == b'x')
        })
    });
    valid && name != "_" && !name.starts_with("__") && !RESERVED.contains(&name) && !vector
}

pub fn custom_sockets(data: &NodeData) -> CustomSockets {
    data.sockets.clone().unwrap_or_else(default_sockets)
}

/// The function a Custom WGSL node compiles to: its inputs are parameters,
/// its outputs variables (zero until the code sets them) returned together.
pub fn custom_function(name: &str, sockets: &CustomSockets, code: &str) -> String {
    let decl = |s: &HandleDef| format!("{}: {}", s.label, s.ty.wgsl_name());
    let struct_name = format!("{name}_outputs");
    let mut lines = vec![format!("struct {struct_name} {{")];
    lines.extend(sockets.outputs.iter().map(|s| format!("  {},", decl(s))));
    lines.push("}".to_owned());
    lines.push(String::new());
    let params: Vec<String> = sockets.inputs.iter().map(decl).collect();
    lines.push(format!("fn {name}({}) -> {struct_name} {{", params.join(", ")));
    lines.extend(sockets.outputs.iter().map(|s| format!("  var {};", decl(s))));
    lines.extend(code.split('\n').map(|l| format!("  {l}")));
    let labels: Vec<&str> = sockets.outputs.iter().map(|s| s.label.as_str()).collect();
    lines.push(format!("  return {struct_name}({});", labels.join(", ")));
    lines.push("}".to_owned());
    lines.join("\n")
}

/// A free label for a new socket.
pub fn new_socket_label(sockets: &CustomSockets, input: bool) -> String {
    let taken: BTreeSet<&str> =
        sockets.inputs.iter().chain(&sockets.outputs).map(|s| s.label.as_str()).collect();
    let base = if input { "input" } else { "output" };
    if !taken.contains(base) {
        return base.to_owned();
    }
    (2..).map(|n| format!("{base}{n}")).find(|n| !taken.contains(n.as_str())).expect("unbounded")
}

/// A Custom WGSL node, whose sockets are its own (`data.sockets`).
pub fn build_custom_wgsl_descriptor(data: &NodeData) -> NodeDescriptor {
    let sockets = custom_sockets(data);
    let defaults = NodeData {
        code: Some(DEFAULT_CODE.to_owned()),
        sockets: Some(default_sockets()),
        ..Default::default()
    };
    let mut d = NodeDescriptor::new(CUSTOM_WGSL, "Custom WGSL", NodeCategory::Misc)
        .aliases(&["Code", "Script", "Expression", "Custom Function", "Shader Code"])
        .static_outputs()
        .inputs(
            sockets.inputs.iter().map(|s| NodeInputDef::new(s.id.clone(), s.label.clone(), &[s.ty])).collect(),
        )
        .outputs(
            sockets.outputs.iter().map(|s| NodeOutputDef::new(s.id.clone(), s.label.clone(), s.ty)).collect(),
        )
        .defaults(defaults)
        .wgsl(move |ctx| {
            if sockets.outputs.is_empty() {
                return Vec::new();
            }
            let name = format!("custom_{}", ctx.var_name);
            let source = custom_function(&name, &sockets, ctx.data.code.as_deref().unwrap_or(""));
            let name = ctx.define_fn(&name, &source);
            let args: Vec<String> = sockets.inputs.iter().map(|s| ctx.promote_input(&s.id)).collect();
            let call = format!("{name}({})", args.join(", "));
            if let [only] = sockets.outputs.as_slice() {
                return vec![format!("let {} = {call}.{};", ctx.var_name, only.label)];
            }
            let mut lines = vec![format!("let {} = {call};", ctx.var_name)];
            lines.extend(sockets.outputs.iter().map(|s| {
                let variable = ctx.output_vars.get(&s.id).cloned().unwrap_or_else(|| format!("{}_{}", ctx.var_name, s.label));
                format!("let {variable} = {}.{};", ctx.var_name, s.label)
            }));
            lines
        });
    d.widget = Some(NodeWidget::Code);
    d
}

// ── Groups ───────────────────────────────────────────────────────────────────

/// The types a socket accepts and the family it's in: a variable's sockets
/// share one, an inferred output has its own.
fn socket_types(handle: &GroupHandleDef, variables: &[TypeVariable]) -> (Vec<WgslType>, Option<String>) {
    match &handle.ty {
        HandleType::Fixed(t) => (vec![*t], None),
        HandleType::Inferred { .. } => (WgslType::ALL.to_vec(), Some(format!("inferred:{}", handle.id))),
        HandleType::Variable { variable } => {
            let types = variables
                .iter()
                .find(|v| &v.id == variable)
                .map_or_else(|| vec![WgslType::F32], |v| v.types.clone());
            (types, Some(variable.clone()))
        }
    }
}

fn default_type(handle: &GroupHandleDef, variables: &[TypeVariable]) -> WgslType {
    bound_type(&handle.ty, variables, &Default::default()).unwrap_or(WgslType::F32)
}

pub fn build_group_input_descriptor(
    handles: &[GroupHandleDef],
    variables: &[TypeVariable],
) -> NodeDescriptor {
    NodeDescriptor::new("group-input", "Group Input", NodeCategory::Input)
        .hidden()
        .static_outputs()
        .outputs(
            handles
                .iter()
                .map(|h| NodeOutputDef::new(h.id.clone(), h.label.clone(), default_type(h, variables)))
                .collect(),
        )
}

pub fn build_group_output_descriptor(
    handles: &[GroupHandleDef],
    variables: &[TypeVariable],
) -> NodeDescriptor {
    NodeDescriptor::new("group-output", "Group Output", NodeCategory::Misc)
        .hidden()
        .inputs(
            handles
                .iter()
                .map(|h| {
                    let (types, family) = socket_types(h, variables);
                    let mut input = NodeInputDef::new(h.id.clone(), h.label.clone(), &types);
                    input.family = family;
                    input
                })
                .collect(),
        )
}

/// The input a group input handle gives group nodes.
pub fn group_input_def(handle: &GroupHandleDef, variables: &[TypeVariable]) -> NodeInputDef {
    let compiled = handle.compiled == Some(true);
    let control = if let Some(variants) = &handle.variants {
        Some(if handle.ty == HandleType::Fixed(WgslType::Bool) {
            InputControl::Toggle { compiled: true }
        } else {
            InputControl::Enum { variants: variants.clone() }
        })
    } else if handle.display == Some(crate::types::ColorDisplay::Swatch) {
        Some(InputControl::Color)
    } else if handle.display == Some(crate::types::ColorDisplay::Wheel) {
        Some(InputControl::Wheel)
    } else if handle.min.is_some() || handle.max.is_some() {
        Some(InputControl::Number {
            min: handle.min,
            max: handle.max,
            soft_min: None,
            soft_max: None,
            compiled,
        })
    } else if compiled {
        Some(InputControl::Number { min: None, max: None, soft_min: None, soft_max: None, compiled: true })
    } else {
        None
    };
    let (types, family) = socket_types(handle, variables);
    let mut input = NodeInputDef::new(handle.id.clone(), handle.label.clone(), &types);
    input.family = family;
    input.control = control;
    input
}

/// Group-input handles of a definition whose value goes straight (through
/// reroutes at most) into a Fallback node, or into a fallback input of a
/// nested group: unlinked on a group node, they fall back to the group's own
/// default instead of showing an editor.
pub fn fallback_handle_ids(
    definition: &GraphDocument,
    project: &GraphProject,
    visiting: &mut Vec<*const GraphDocument>,
) -> BTreeSet<String> {
    visiting.push(definition);
    let input_nodes: BTreeSet<&str> =
        definition.nodes.iter().filter(|n| n.is("group-input")).map(|n| n.id.as_str()).collect();
    fn is_fallback(
        definition: &GraphDocument,
        project: &GraphProject,
        visiting: &mut Vec<*const GraphDocument>,
        target: &str,
        handle: &str,
        depth: usize,
    ) -> bool {
        let Some(node) = definition.node(target) else { return false };
        if node.is("fallback") {
            return handle == "value";
        }
        if node.is("reroute") && depth < 64 {
            return definition.edges_from(target).any(|e| {
                is_fallback(definition, project, visiting, &e.target, &e.target_handle, depth + 1)
            });
        }
        let Some(inner) = node.definition_ref().and_then(|r| project.document(r)) else {
            return false;
        };
        if visiting.contains(&std::ptr::from_ref(inner)) {
            return false;
        }
        fallback_handle_ids(inner, project, visiting).contains(handle)
    }
    let mut result = BTreeSet::new();
    for e in &definition.edges {
        if input_nodes.contains(e.source.as_str())
            && is_fallback(definition, project, visiting, &e.target, &e.target_handle, 0)
        {
            result.insert(e.source_handle.clone());
        }
    }
    visiting.pop();
    result
}

pub fn build_group_descriptor(node: &GraphNode, project: &GraphProject) -> NodeDescriptor {
    let definition_ref = node.definition_ref();
    let Some((reference, definition)) =
        definition_ref.and_then(|r| project.document(r).map(|d| (r, d)))
    else {
        return NodeDescriptor::new("node-group", "Missing group", NodeCategory::Misc).static_outputs();
    };
    let fallbacks = fallback_handle_ids(definition, project, &mut Vec::new());
    // The group's defaults are what instances hold until they're edited.
    let mut data = NodeData::default();
    for h in &definition.inputs {
        let stored = definition.defaults.get(&h.id);
        let value: Option<InputValue> = match &h.variants {
            Some(variants) => enum_value(stored, variants).map(|v| InputValue::from(v as f64)),
            None => stored.copied(),
        };
        if let Some(value) = value {
            data.values.insert(h.id.clone(), value);
        }
    }
    let mut descriptor =
        NodeDescriptor::new("node-group", definition.label.as_deref().unwrap_or(reference), NodeCategory::Misc)
        .static_outputs()
        .inputs(
            definition
                .inputs
                .iter()
                .map(|h| {
                    let mut input = group_input_def(h, &definition.type_variables);
                    input.fallback = fallbacks.contains(&h.id);
                    input
                })
                .collect(),
        )
        .outputs(
            definition
                .outputs
                .iter()
                .map(|h| {
                    NodeOutputDef::new(h.id.clone(), h.label.clone(), default_type(h, &definition.type_variables))
                })
                .collect(),
        )
        .defaults(data);
    descriptor.widget = definition.widget.clone().map(NodeWidget::Color);
    descriptor
}
