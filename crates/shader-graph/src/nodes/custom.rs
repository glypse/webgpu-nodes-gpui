//! The Custom WGSL node: user-written code over sockets of its own.

use std::collections::BTreeSet;

use crate::types::{
    CustomSockets, HandleDef, NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef,
    NodeWidget, WgslType,
};

pub const CUSTOM_WGSL: &str = "custom-wgsl";

pub fn default_sockets() -> CustomSockets {
    CustomSockets {
        inputs: vec![HandleDef::new("uv", "uv", WgslType::Vec2f)],
        outputs: vec![HandleDef::new("color", "color", WgslType::Vec3f)],
    }
}

pub const DEFAULT_CODE: &str = "color = vec3f(uv, 0.5);";

/// WGSL words a socket can't be named, as they'd clash with the language.
const RESERVED: &[&str] = &[
    "alias",
    "bitcast",
    "bool",
    "break",
    "case",
    "const",
    "const_assert",
    "continue",
    "continuing",
    "default",
    "diagnostic",
    "discard",
    "else",
    "enable",
    "f16",
    "f32",
    "false",
    "fn",
    "for",
    "i32",
    "if",
    "let",
    "loop",
    "override",
    "requires",
    "return",
    "struct",
    "switch",
    "true",
    "u32",
    "var",
    "while",
];

/// Whether `name` can name a socket: a WGSL identifier (no leading `__`, which
/// WGSL reserves) that isn't a keyword or a type.
pub fn is_socket_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let valid = starts && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid
        && name != "_"
        && !name.starts_with("__")
        && !RESERVED.contains(&name)
        && !is_vector_or_matrix_name(name)
}

/// `vec2`..`vec4` and `mat2x2`..`mat4x4`, optionally suffixed `f`, `h`, `i` or `u`.
fn is_vector_or_matrix_name(name: &str) -> bool {
    let suffixless = name.strip_suffix(['f', 'h', 'i', 'u']).unwrap_or(name);
    let dims = |rest: &str| rest.len() == 1 && matches!(rest, "2" | "3" | "4");
    if let Some(rest) = suffixless.strip_prefix("vec") {
        return dims(rest);
    }
    suffixless.strip_prefix("mat").is_some_and(|rest| {
        rest.split_once('x')
            .is_some_and(|(c, r)| dims(c) && dims(r))
    })
}

/// Whether every socket has a name of its own that WGSL accepts. Labels are
/// written into the shader as they are, so a file can't be trusted to have them.
pub fn has_valid_socket_names(sockets: &CustomSockets) -> bool {
    let labels: Vec<&str> = sockets
        .inputs
        .iter()
        .chain(&sockets.outputs)
        .map(|s| s.label.as_str())
        .collect();
    labels.iter().all(|l| is_socket_name(l))
        && labels.iter().collect::<BTreeSet<_>>().len() == labels.len()
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
    lines.push(format!(
        "fn {name}({}) -> {struct_name} {{",
        params.join(", ")
    ));
    lines.extend(
        sockets
            .outputs
            .iter()
            .map(|s| format!("  var {};", decl(s))),
    );
    lines.extend(code.split('\n').map(|l| format!("  {l}")));
    let labels: Vec<&str> = sockets.outputs.iter().map(|s| s.label.as_str()).collect();
    lines.push(format!("  return {struct_name}({});", labels.join(", ")));
    lines.push("}".to_owned());
    lines.join("\n")
}

/// A free label for a new socket.
pub fn new_socket_label(sockets: &CustomSockets, input: bool) -> String {
    let taken: BTreeSet<&str> = sockets
        .inputs
        .iter()
        .chain(&sockets.outputs)
        .map(|s| s.label.as_str())
        .collect();
    let base = if input { "input" } else { "output" };
    if !taken.contains(base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| format!("{base}{n}"))
        .find(|n| !taken.contains(n.as_str()))
        .unwrap_or_else(|| base.to_owned())
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
        .aliases(&[
            "Code",
            "Script",
            "Expression",
            "Custom Function",
            "Shader Code",
        ])
        .static_outputs()
        .inputs(
            sockets
                .inputs
                .iter()
                .map(|s| NodeInputDef::new(s.id.clone(), s.label.clone(), &[s.ty]))
                .collect(),
        )
        .outputs(
            sockets
                .outputs
                .iter()
                .map(|s| NodeOutputDef::new(s.id.clone(), s.label.clone(), s.ty))
                .collect(),
        )
        .defaults(defaults)
        .wgsl(move |ctx| {
            if sockets.outputs.is_empty() {
                return Vec::new();
            }
            let name = format!("custom_{}", ctx.var_name);
            let source = custom_function(&name, &sockets, ctx.data.code.as_deref().unwrap_or(""));
            let name = ctx.define_fn(&name, &source);
            let args: Vec<String> = sockets
                .inputs
                .iter()
                .map(|s| ctx.promote_input(&s.id))
                .collect();
            let call = format!("{name}({})", args.join(", "));
            if let [only] = sockets.outputs.as_slice() {
                return vec![format!("let {} = {call}.{};", ctx.var_name, only.label)];
            }
            let mut lines = vec![format!("let {} = {call};", ctx.var_name)];
            lines.extend(sockets.outputs.iter().map(|s| {
                let variable = ctx
                    .output_vars
                    .get(&s.id)
                    .cloned()
                    .unwrap_or_else(|| format!("{}_{}", ctx.var_name, s.label));
                format!("let {variable} = {}.{};", ctx.var_name, s.label)
            }));
            lines
        });
    d.widget = Some(NodeWidget::Code);
    d
}
