//! The registered node types with fixed sockets: input, math, vector,
//! integer, logic and converters.

use crate::codegen::{DISPLAY_P3_FIELD, FRAGMENT_VAR, FRAME_VAR};
use crate::types::{
    BOOL_TYPES, FLOAT_VEC_TYPES, INTEGER_TYPES, InputControl, NUMERIC_TYPES, NodeCategory,
    NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef, NodeSetting, ScalarKind, SIGNED_TYPES,
    TypeFamily, UINT_TYPES, VEC_TYPES, WgslType, make_type,
};
use crate::types::{FLOAT_TYPES, HelperName, SOCKET_TYPES};

use super::builders::*;

type D = NodeDescriptor;

const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];
pub const COMPONENT_COUNT: &str = "component-count";
pub const COMPONENT_COUNT_INPUT: &str = "value";
pub const FALLBACK: &str = "fallback";
pub const FALLBACK_VALUE: &str = "value";
pub const FALLBACK_DEFAULT: &str = "default";
/// Measurable as WGSL's `length` and `distance` measure them.
const MEASURED_TYPES: &[WgslType] = &[WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f, WgslType::F32];

fn op_options(types: &'static [WgslType], ids: [&'static str; 2]) -> BinaryOptions {
    BinaryOptions { types, ids, ..BinaryOptions::default() }
}

fn math(d: D, aliases: &[&str]) -> D {
    d.aliases(aliases)
}

pub(super) fn input_nodes() -> Vec<D> {
    let declare = {
        let mut d = D::new("declare", "Declare", NodeCategory::Input)
            .aliases(&[
                "Combine Vector",
                "Combine XYZ",
                "Make Vector",
                "Make Float3",
                "Append",
                "Vector",
                "Value",
                "Constant",
                "Float",
            ])
            .inputs(
                COMPONENTS
                    .iter()
                    .map(|c| NodeInputDef::new(*c, *c, &[WgslType::F32]))
                    .collect(),
            )
            .outputs(vec![NodeOutputDef::new("value", "value", WgslType::F32)])
            .defaults(float_values([("x", 0.0), ("y", 0.0), ("z", 0.0), ("w", 0.0)]))
            .wgsl(|ctx| {
                let ids: Vec<String> = ctx.resolved_inputs.iter().map(|i| i.id.clone()).collect();
                let args: Vec<String> = ids.iter().map(|id| ctx.promote_input(id)).collect();
                let args = args.join(", ");
                if ctx.output_type.components() == 1 {
                    vec![format!("let {} = {args};", ctx.var_name)]
                } else {
                    vec![format!("let {} = {}({args});", ctx.var_name, ctx.output_type.wgsl_name())]
                }
            });
        d.type_family =
            Some(TypeFamily { key: "declare".to_owned(), types: WgslType::ALL.to_vec() });
        d.resolve_inputs = Some(std::sync::Arc::new(|out, _| {
            let scalar = make_type(out.kind(), 1);
            COMPONENTS[..out.components()]
                .iter()
                .map(|c| NodeInputDef::new(*c, *c, &[scalar]))
                .collect()
        }));
        d
    };

    let time = D::new("time", "Time", NodeCategory::Input)
        .aliases(&["Scene Time", "Absolute Time", "Seconds", "Frame", "Frame Index", "iTime", "iFrame"])
        .static_outputs()
        .outputs(vec![
            NodeOutputDef::new("seconds", "seconds", WgslType::F32),
            NodeOutputDef::new("frame", "frame", WgslType::U32),
        ])
        .wgsl(|ctx| {
            vec![
                format!("let {} = {FRAME_VAR}.time;", ctx.output_var("seconds")),
                format!("let {} = {FRAME_VAR}.frame;", ctx.output_var("frame")),
            ]
        });

    vec![
        declare,
        uniform_reader("uv", "UV", "uv", WgslType::Vec2f, format!("{FRAGMENT_VAR}.uv")).aliases(&[
            "Texture Coordinate",
            "TexCoord",
            "Screen Position",
            "Fragment Coordinate",
            "fragCoord",
        ]),
        time,
        uniform_reader("delta", "Delta", "delta", WgslType::F32, format!("{FRAME_VAR}.delta"))
            .aliases(&["Delta Time", "Frame Time", "iTimeDelta"]),
        uniform_reader(
            "resolution",
            "Resolution",
            "resolution",
            WgslType::Vec2f,
            format!("{FRAME_VAR}.resolution"),
        )
        .aliases(&["Screen Size", "View Size", "Canvas Size", "iResolution"]),
    ]
}

fn float_unary(ty: &str, label: &str, f: &'static str, aliases: &[&str]) -> D {
    math(unary_fn(ty, label, f, FLOAT_TYPES, NodeCategory::Math), aliases)
}

pub(super) fn math_nodes() -> Vec<D> {
    let cat = NodeCategory::Math;
    let safe = |helper: HelperName| {
        move |a: &str, b: &str, ctx: &mut crate::types::WgslCtx<'_>| {
            format!("{}({a}, {b})", ctx.helper(helper, ctx.output_type))
        }
    };
    let float_pair = |ty: &str, label: &str, f: &'static str, labels: [&'static str; 2]| {
        binary_fn(
            ty,
            label,
            f,
            BinaryOptions {
                types: FLOAT_TYPES,
                ids: ["e1", "e2"],
                labels: Some(labels),
                ..BinaryOptions::default()
            },
        )
    };
    let float3 = |ty: &str, label: &str, ids: [&'static str; 3], defaults: [f64; 3]| {
        let inputs = ids.iter().map(|id| NodeInputDef::new(*id, *id, FLOAT_TYPES)).collect();
        let d: Vec<_> = ids.iter().copied().zip(defaults).collect();
        n_ary_fn(ty, label, ty, inputs, &d)
    };
    let number3 = || {
        let ids = ["e", "low", "high"];
        n_ary_fn(
            "clamp",
            "Clamp",
            "clamp",
            ids.iter().map(|id| NodeInputDef::new(*id, *id, NUMERIC_TYPES)).collect(),
            &[("e", 0.0), ("low", 0.0), ("high", 1.0)],
        )
    };
    vec![
        math(variadic_op("add", "Add", " + "), &["Plus", "Sum", "Vector Math"]),
        math(
            binary_node("sub", "Subtract", |a, b, _| format!("{a} - {b}"), BinaryOptions::default()),
            &["Minus", "Difference", "Vector Math"],
        ),
        math(variadic_op("mul", "Multiply", " * "), &["Times", "Product", "Scale", "Vector Math"]),
        math(
            binary_node("div", "Divide", safe(HelperName::SafeDiv), BinaryOptions::default()),
            &["Quotient", "Vector Math"],
        ),
        math(
            binary_node("mod", "Modulo", safe(HelperName::SafeMod), BinaryOptions::default()),
            &["Remainder", "fmod", "Truncated Modulo", "Vector Math"],
        ),
        math(
            unary_node("negate", "Negate", SIGNED_TYPES, cat).wgsl(|ctx| {
                let e = ctx.promote_input("e");
                vec![format!("let {} = -({e});", ctx.var_name)]
            }),
            &["Negative", "Flip Sign"],
        ),
        math(
            float3("mix", "Mix", ["e1", "e2", "e3"], [0.0; 3]),
            &["Lerp", "Linear Interpolate", "Interpolate", "Blend", "Mix Color", "Cross Fade"],
        ),
        math(
            float3("smoothstep", "Smoothstep", ["low", "high", "x"], [0.0, 1.0, 0.0]),
            &["Smooth Step", "Hermite", "Ease", "Smooth Threshold"],
        ),
        math(unary_fn("abs", "Absolute", "abs", NUMERIC_TYPES, cat), &["Vector Math"]),
        math(variadic_fn("min", "Minimum", "min"), &["Darken", "Vector Math"]),
        math(variadic_fn("max", "Maximum", "max"), &["Lighten", "Vector Math"]),
        math(
            float_pair("step", "Step", "step", ["edge", "x"]),
            &["Threshold", "Greater Than", "Heaviside"],
        ),
        math(number3(), &["Saturate", "Limit", "Clip"]),
        math(
            binary_node(
                "pow",
                "Power",
                safe(HelperName::SafePow),
                op_options(FLOAT_TYPES, ["e1", "e2"]),
            ),
            &["Exponent", "Gamma"],
        ),
        float_unary("sin", "Sine", "sin", &[]),
        float_unary("cos", "Cosine", "cos", &[]),
        float_unary("tan", "Tangent", "tan", &[]),
        float_unary("atan", "Arc Tangent", "atan", &["atan", "Arctangent", "Inverse Tangent"]),
        math(float_pair("atan2", "Arc Tangent 2", "atan2", ["y", "x"]), &["atan2", "Arctangent 2", "Angle"]),
        float_unary("floor", "Floor", "floor", &["Round Down"]),
        float_unary("ceil", "Ceil", "ceil", &["Ceiling", "Round Up"]),
        float_unary("fract", "Fract", "fract", &["Fraction", "Fractional Part"]),
        float_unary("round", "Round", "round", &["Nearest Integer", "rint"]),
        math(unary_fn("sign", "Sign", "sign", SIGNED_TYPES, cat), &["Signum", "sgn"]),
        float_unary("log", "Logarithm", "log", &["ln", "Natural Logarithm", "log"]),
        float_unary("exp", "Exponential", "exp", &["exp", "e^x", "Natural Exponent"]),
        float_unary("sqrt", "Square Root", "sqrt", &["sqrt", "Root"]),
    ]
}

pub(super) fn vector_nodes() -> Vec<D> {
    let cat = NodeCategory::Vector;
    let pair = |ty: &str, label: &str, f: &'static str, types: &'static [WgslType], labels| {
        binary_fn(ty, label, f, BinaryOptions { types, ids: ["e1", "e2"], labels, category: cat })
    };
    let length = D::new("length", "Length", cat)
        .aliases(&["Magnitude", "Norm", "Vector Math"])
        .inputs(vec![NodeInputDef::new("e1", "e1", MEASURED_TYPES)])
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .defaults(float_values([("e1", 0.0)]))
        .output_type(|_, _| WgslType::F32)
        .wgsl(|ctx| {
            let e = ctx.promote_input("e1");
            vec![format!("let {} = length({e});", ctx.var_name)]
        });
    let separate = {
        let mut d = D::new("separate-vector", "Separate Vector", cat)
            .aliases(&[
                "Separate XYZ",
                "Separate Color",
                "Separate RGB",
                "Split Vector",
                "Break Vector",
                "Break Out Float3 Components",
                "Component Mask",
                "Swizzle",
                "Shuffle",
            ])
            .inputs(vec![NodeInputDef::new("v", "v", VEC_TYPES)])
            .outputs(COMPONENTS.iter().map(|c| NodeOutputDef::new(*c, *c, WgslType::F32)).collect())
            .output_type(|t, _| make_type(t.kind(), 1))
            .wgsl(|ctx| {
                let source = ctx.promote_input("v");
                let outputs = ctx.resolved_outputs.clone();
                outputs
                    .iter()
                    .map(|o| format!("let {} = {source}.{};", ctx.output_var(&o.id), o.id))
                    .collect()
            });
        d.resolve_outputs = Some(std::sync::Arc::new(|t| {
            COMPONENTS[..t.components()]
                .iter()
                .map(|c| NodeOutputDef::new(*c, *c, make_type(t.kind(), 1)))
                .collect()
        }));
        d
    };
    let count = D::new(COMPONENT_COUNT, "Component Count", cat)
        .aliases(&["Dimensions", "Vector Size", "Vector Width", "Length of Type"])
        .inputs(vec![NodeInputDef::new(COMPONENT_COUNT_INPUT, "value", &WgslType::ALL)])
        .outputs(vec![NodeOutputDef::new("count", "count", WgslType::U32)])
        .static_outputs()
        .wgsl(|ctx| {
            let n = ctx.input_type(COMPONENT_COUNT_INPUT).components();
            vec![format!("let {} = {n}u;", ctx.var_name)]
        });
    vec![
        pair("dot", "Dot Product", "dot", FLOAT_VEC_TYPES, None)
            .aliases(&["Scalar Product", "Inner Product", "Vector Math"])
            .output_type(|_, _| WgslType::F32),
        pair("distance", "Distance", "distance", MEASURED_TYPES, None)
            .aliases(&["Vector Math"])
            .output_type(|_, _| WgslType::F32),
        length,
        math(unary_fn("normalize", "Normalize", "normalize", FLOAT_VEC_TYPES, cat), &["Normalise", "Unit Vector", "Vector Math"]),
        pair("cross", "Cross Product", "cross", &[WgslType::Vec3f], None)
            .aliases(&["Vector Product", "Vector Math"])
            .output_type(|_, _| WgslType::Vec3f),
        pair("reflect", "Reflect", "reflect", FLOAT_VEC_TYPES, Some(["incident", "normal"]))
            .aliases(&["Reflection", "Vector Math"]),
        separate,
        count,
    ]
}

pub(super) fn integer_nodes() -> Vec<D> {
    let cat = NodeCategory::Integer;
    let bitwise_setting =
        NodeSetting::new("op", "Operation", &[("&", "And"), ("|", "Or"), ("^", "Xor")]);
    let shift_setting = NodeSetting::new("op", "Direction", &[("<<", "Left"), (">>", "Right")]);
    let bitwise = {
        let setting = bitwise_setting.clone();
        binary_node(
            "bitwise",
            "Bitwise",
            move |a, b, ctx| format!("{a} {} {b}", setting.value(&ctx.data)),
            BinaryOptions { types: INTEGER_TYPES, category: cat, ..BinaryOptions::default() },
        )
        .aliases(&["Bitwise And", "Bitwise Or", "Bitwise Xor"])
        .defaults(float_values([("a", 0.0), ("b", 0.0)]).setting("op", "&"))
        .settings(vec![bitwise_setting])
    };
    let shift = {
        let setting = shift_setting.clone();
        D::new("bit-shift", "Bit Shift", cat)
            .aliases(&["Shift Left", "Shift Right", "Left Shift", "Right Shift"])
            .inputs(vec![
                NodeInputDef::new("a", "value", INTEGER_TYPES),
                NodeInputDef::new("b", "shift", UINT_TYPES),
            ])
            .outputs(vec![NodeOutputDef::new("result", "result", WgslType::I32)])
            .defaults(float_values([("a", 0.0), ("b", 0.0)]).setting("op", "<<"))
            .settings(vec![shift_setting])
            .wgsl(move |ctx| {
                // WGSL wants the shift amount as u32 (or a u32 vector of the
                // value's width), strictly below the bit width: mask it so
                // constants stay valid.
                let shift_type = make_type(ScalarKind::U32, ctx.output_type.components());
                let b = ctx.promote_input("b");
                let amount = crate::types::convert_var(&b, ctx.input_type("b"), shift_type);
                let mask = if shift_type.components() == 1 {
                    "31u".to_owned()
                } else {
                    format!("{shift_type}(31u)")
                };
                let a = ctx.promote_input("a");
                vec![format!("let {} = {a} {} ({amount} & {mask});", ctx.var_name, setting.value(&ctx.data))]
            })
    };
    let not = math(
        unary_node("bitwise-not", "Bitwise Not", INTEGER_TYPES, cat).wgsl(|ctx| {
            let e = ctx.promote_input("e");
            vec![format!("let {} = ~({e});", ctx.var_name)]
        }),
        &["Complement", "Bit Invert"],
    );
    vec![bitwise, not, shift]
}

pub(super) fn logic_nodes() -> Vec<D> {
    let cat = NodeCategory::Logic;
    let compare_setting = NodeSetting::new(
        "op",
        "Operation",
        &[
            ("<", "Less Than"),
            ("<=", "Less Than or Equal"),
            ("==", "Equal"),
            ("!=", "Not Equal"),
            (">=", "Greater Than or Equal"),
            (">", "Greater Than"),
        ],
    );
    let compare = {
        let setting = compare_setting.clone();
        D::new("compare", "Compare", cat)
            .aliases(&["Greater Than", "Less Than", "Equal", "Not Equal", "Comparison", "If"])
            .inputs(vec![
                NodeInputDef::new("a", "a", NUMERIC_TYPES),
                NodeInputDef::new("b", "b", NUMERIC_TYPES),
            ])
            .outputs(vec![NodeOutputDef::new("result", "result", WgslType::Bool)])
            .defaults(float_values([("a", 0.0), ("b", 0.0)]).setting("op", "<"))
            .settings(vec![compare_setting])
            .output_type(|t, _| make_type(ScalarKind::Bool, t.components()))
            .wgsl(move |ctx| {
                let (a, b) = (ctx.promote_input("a"), ctx.promote_input("b"));
                vec![format!("let {} = {a} {} {b};", ctx.var_name, setting.value(&ctx.data))]
            })
    };
    let switch = D::new("switch", "Switch", cat)
        .aliases(&["If", "Select", "Branch", "Conditional", "Ternary", "Static Switch"])
        .primary("false")
        .inputs(vec![
            NodeInputDef::new("condition", "condition", BOOL_TYPES),
            NodeInputDef::new("false", "false", &WgslType::ALL),
            NodeInputDef::new("true", "true", &WgslType::ALL),
        ])
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .defaults({
            let mut d = float_values([("false", 0.0), ("true", 1.0)]);
            d.values.insert("condition".to_owned(), false.into());
            d
        })
        .wgsl(|ctx| {
            let value_type = ctx.output_type;
            let condition_type = ctx.input_type("condition");
            let mut condition = ctx.promote_input("condition");
            // `select` takes either a scalar bool or a bool vector of the value's width.
            if condition_type.components() != 1 {
                let target = make_type(ScalarKind::Bool, value_type.components());
                condition = crate::types::convert_var(&condition, condition_type, target);
            }
            let (f, t) = (ctx.promote_input("false"), ctx.promote_input("true"));
            vec![format!("let {} = select({f}, {t}, {condition});", ctx.var_name)]
        });
    let boolean_setting = NodeSetting::new(
        "op",
        "Operation",
        &[
            ("and", "And"),
            ("or", "Or"),
            ("xor", "Xor"),
            ("nand", "Nand"),
            ("nor", "Nor"),
            ("xnor", "Xnor"),
        ],
    );
    let boolean_math = {
        let setting = boolean_setting.clone();
        binary_node(
            "boolean-math",
            "Boolean Math",
            move |a, b, ctx| match setting.value(&ctx.data) {
                "or" => format!("{a} | {b}"),
                "xor" => format!("{a} != {b}"),
                "nand" => format!("!({a} & {b})"),
                "nor" => format!("!({a} | {b})"),
                "xnor" => format!("{a} == {b}"),
                _ => format!("{a} & {b}"),
            },
            BinaryOptions { types: BOOL_TYPES, category: cat, ..BinaryOptions::default() },
        )
        .aliases(&["And", "Or", "Xor", "Nand", "Nor", "Logic"])
        .defaults({
            let mut d = NodeData::default().setting("op", "and");
            d.values.insert("a".to_owned(), false.into());
            d.values.insert("b".to_owned(), false.into());
            d
        })
        .settings(vec![boolean_setting])
    };
    let bool_default = || {
        let mut d = NodeData::default();
        d.values.insert("e".to_owned(), false.into());
        d
    };
    let not = unary_node("not", "Not", BOOL_TYPES, cat)
        .aliases(&["Logical Not", "Boolean Not", "Invert Boolean"])
        .defaults(bool_default())
        .wgsl(|ctx| {
            let e = ctx.promote_input("e");
            vec![format!("let {} = !({e});", ctx.var_name)]
        });
    let reduce_setting = NodeSetting::new("op", "Operation", &[("any", "Any"), ("all", "All")]);
    let reduce = {
        let setting = reduce_setting.clone();
        unary_node("reduce-boolean", "Any / All", BOOL_TYPES, cat)
            .aliases(&["Reduce"])
            .defaults(bool_default().setting("op", "any"))
            .settings(vec![reduce_setting])
            .output_type(|_, _| WgslType::Bool)
            .wgsl(move |ctx| {
                let e = ctx.promote_input("e");
                vec![format!("let {} = {}({e});", ctx.var_name, setting.value(&ctx.data))]
            })
    };
    // Presence is known while compiling (never a runtime "null"): `value` is
    // absent when nothing feeds it, or when it comes from a group input whose
    // socket the calling group node left unlinked.
    let fallback = D::new(FALLBACK, "Fallback", cat)
        .aliases(&["Default Value", "Coalesce", "Optional"])
        .inputs(vec![
            NodeInputDef::new(FALLBACK_VALUE, "value", &WgslType::ALL),
            NodeInputDef::new(FALLBACK_DEFAULT, "default", &WgslType::ALL),
        ])
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .defaults(float_values([(FALLBACK_DEFAULT, 0.0)]))
        .wgsl(|ctx| {
            let chosen = if ctx.is_present(FALLBACK_VALUE) { FALLBACK_VALUE } else { FALLBACK_DEFAULT };
            let e = ctx.promote_input(chosen);
            vec![format!("let {} = {e};", ctx.var_name)]
        });
    vec![compare, switch, boolean_math, not, reduce, fallback]
}

pub(super) fn converter_nodes() -> Vec<D> {
    vec![
        convert_node("to-float", "To Float", ScalarKind::F32, false)
            .aliases(&["Float", "Cast", "Convert"]),
        convert_node("to-int", "To Integer", ScalarKind::I32, true)
            .aliases(&["Truncate", "Cast", "Convert"]),
        convert_node("to-uint", "To Unsigned", ScalarKind::U32, true)
            .aliases(&["uint", "Cast", "Convert"]),
        convert_node("to-bool", "To Boolean", ScalarKind::Bool, false).aliases(&["Cast", "Convert"]),
    ]
}

const SRGB_TO_DISPLAY_P3: [[f64; 3]; 3] = [
    [0.822461969, 0.177538031, 0.0],
    [0.033194199, 0.966805801, 0.0],
    [0.017082631, 0.072397441, 0.910519929],
];
const DISPLAY_P3_TO_SRGB: [[f64; 3]; 3] = [
    [1.224940176, -0.224940176, 0.0],
    [-0.042056955, 1.042056955, 0.0],
    [-0.019637555, -0.078636046, 1.0982736],
];

/// `m` as a `mat3x3f`, whose constructor takes columns.
fn wgsl_mat3(m: &[[f64; 3]; 3]) -> String {
    let column = |i: usize| format!("vec3f({:.9}, {:.9}, {:.9})", m[0][i], m[1][i], m[2][i]);
    format!("mat3x3f({}, {}, {})", column(0), column(1), column(2))
}

pub(super) fn color_nodes() -> Vec<D> {
    // Per draw rather than per project, so an sRGB export of a Display P3
    // project fits sRGB.
    let clip = D::new("display-gamut-clip", "Clip to Display Gamut", NodeCategory::Color)
        .aliases(&["Gamut Clip", "Clip Color", "Display P3", "sRGB Gamut", "Color Space Conversion"])
        .inputs(vec![NodeInputDef::new("color", "color", &[WgslType::Vec3f])])
        .outputs(vec![NodeOutputDef::new("color", "color", WgslType::Vec3f)])
        .defaults(float_values([("color", 0.0)]))
        .wgsl(|ctx| {
            let source = format!(
                "fn clip_to_display_gamut(c: vec3f) -> vec3f {{\n  if ({FRAME_VAR}.{DISPLAY_P3_FIELD} == 0u) {{\n    return clamp(c, vec3f(0.0), vec3f(1.0));\n  }}\n  let p3 = clamp({} * c, vec3f(0.0), vec3f(1.0));\n  return {} * p3;\n}}",
                wgsl_mat3(&SRGB_TO_DISPLAY_P3),
                wgsl_mat3(&DISPLAY_P3_TO_SRGB),
            );
            let name = ctx.define_fn("clip_to_display_gamut", &source);
            let color = ctx.promote_input("color");
            vec![format!("let {} = {name}({color});", ctx.var_name)]
        });
    vec![clip]
}

pub(super) fn misc_nodes() -> Vec<D> {
    // A bare handle to route links through: forwards what it receives, keeping its type.
    let reroute = D::new("reroute", "Reroute", NodeCategory::Misc)
        .aliases(&["Null", "Knot", "Dot", "Passthrough"])
        .inputs(vec![NodeInputDef::new("in", "in", &SOCKET_TYPES)])
        .outputs(vec![NodeOutputDef::new("out", "out", WgslType::F32)])
        .wgsl(|ctx| {
            let e = ctx.promote_input("in");
            vec![format!("let {} = {e};", ctx.var_name)]
        });
    let layout_frame = D::new("layout-frame", "Frame", NodeCategory::Layout)
        .aliases(&["Comment", "Box", "Region", "Note"]);
    // Padding a vec3f to vec4f would give it a zero alpha: an opaque colour is
    // meant instead, so vec3f is accepted as is and gets an alpha of 1 here.
    let output = D::new("frag-output", "Output", NodeCategory::Misc)
        .hidden()
        .inputs(vec![
            NodeInputDef::new("color", "color", &[WgslType::Vec4f, WgslType::Vec3f])
                .control(InputControl::Color),
        ])
        .wgsl(|ctx| {
            let color = ctx.promote_input("color");
            let rgba = if ctx.input_type("color") == WgslType::Vec3f {
                format!("vec4f({color}, 1.0)")
            } else {
                color
            };
            vec![format!("let {} = {rgba};", ctx.var_name)]
        });
    vec![reroute, layout_frame, output]
}
