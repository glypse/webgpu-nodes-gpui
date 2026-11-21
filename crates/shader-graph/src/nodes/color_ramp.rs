//! The Color Ramp node, after Blender's: a colour picked along stops by a
//! factor. Each stop's colour is an input of its own (so it can be linked),
//! its position is the node's data. The colours are interpolated in RGB, HSV
//! or OKLch, any of the interpolations in any space; these are enum inputs,
//! so groups can pass theirs on.

use std::collections::{BTreeMap, HashSet};
use std::f64::consts::TAU;

use crate::color::{LINEAR_RGB_TO_LMS, LMS_TO_LINEAR_RGB, LMS_TO_OKLAB, OKLAB_TO_LMS, wgsl_mat3};
use crate::graph::{GraphModel, GraphProject};
use crate::ramp::{
    HUE_DECREASING, HUE_INCREASING, HUE_LONGER, HUE_SHORTER, RAMP_ACHROMATIC, RAMP_B_SPLINE,
    RAMP_CARDINAL, RAMP_CARDINAL_TENSION, RAMP_CONSTANT, RAMP_EASE, RAMP_HSV, RAMP_HUES,
    RAMP_INTERPOLATIONS, RAMP_LINEAR, RAMP_OKLCH, RAMP_RGB, RAMP_SMOOTH, RAMP_SPACES, RampStop,
    default_ramp, is_ramp_stop_id, sorted_ramp_stops,
};
use crate::types::{
    EnumVariant, InputControl, InputValue, NodeCategory, NodeData, NodeDescriptor, NodeInputDef,
    NodeOutputDef, NodeWidget, WgslCtx, WgslType, enum_value,
};

pub const COLOR_RAMP: &str = "color-ramp";
pub const RAMP_FACTOR: &str = "factor";
pub const RAMP_INTERPOLATION: &str = "interpolation";
pub const RAMP_SPACE: &str = "space";
pub const RAMP_HUE: &str = "hue";
/// The type family of the stops' colours, which is the output's type.
const COLOR_FAMILY: &str = "color";

/// The node's stops, sorted and cleaned.
pub fn color_ramp_stops(data: &NodeData) -> Vec<RampStop> {
    sorted_ramp_stops(data.ramp.as_deref())
}

/// The inputs a Color Ramp doesn't show: its hue interpolation while its
/// colour space is RGB (picked on the node), which has no hue.
pub fn color_ramp_hidden_inputs(data: &NodeData, linked: &HashSet<String>) -> HashSet<String> {
    let space = data
        .values
        .get(RAMP_SPACE)
        .map_or(RAMP_RGB, InputValue::as_i64);
    let rgb = !linked.contains(RAMP_SPACE) && space == RAMP_RGB;
    if rgb {
        HashSet::from([RAMP_HUE.to_owned()])
    } else {
        HashSet::new()
    }
}

const ENCODE_FN: &str = "color_ramp_encode";
const DECODE_FN: &str = "color_ramp_decode";
const TO_HSV_FN: &str = "color_ramp_to_hsv";
const FROM_HSV_FN: &str = "color_ramp_from_hsv";
const TO_OKLCH_FN: &str = "color_ramp_to_oklch";
const FROM_OKLCH_FN: &str = "color_ramp_from_oklch";
const TO_SPACE_FN: &str = "color_ramp_to_space";
const FROM_SPACE_FN: &str = "color_ramp_from_space";
const HUE_DELTA_FN: &str = "color_ramp_hue_delta";
const SLOPE_FN: &str = "color_ramp_slope";

fn encode_source() -> String {
    format!(
        "fn {ENCODE_FN}(c: vec3f) -> vec3f {{
  let x = max(c, vec3f(0.0));
  return select(1.055 * pow(x, vec3f(1.0 / 2.4)) - 0.055, 12.92 * x, x <= vec3f(0.0031308));
}}"
    )
}

fn decode_source() -> String {
    format!(
        "fn {DECODE_FN}(c: vec3f) -> vec3f {{
  let x = max(c, vec3f(0.0));
  return select(pow((x + 0.055) / 1.055, vec3f(2.4)), x / 12.92, x <= vec3f(0.04045));
}}"
    )
}

fn to_hsv_source() -> String {
    format!(
        "fn {TO_HSV_FN}(c: vec4f) -> vec4f {{
  let e = {ENCODE_FN}(c.rgb);
  let high = max(e.r, max(e.g, e.b));
  let delta = high - min(e.r, min(e.g, e.b));
  var h = 0.0;
  if (delta > 0.0) {{
    if (high == e.r) {{
      h = (e.g - e.b) / delta + select(0.0, 6.0, e.g < e.b);
    }} else if (high == e.g) {{
      h = (e.b - e.r) / delta + 2.0;
    }} else {{
      h = (e.r - e.g) / delta + 4.0;
    }}
  }}
  return vec4f(h / 6.0, select(0.0, delta / high, high > 0.0), high, c.a);
}}"
    )
}

fn from_hsv_source() -> String {
    format!(
        "fn {FROM_HSV_FN}(c: vec4f) -> vec4f {{
  let s = clamp(c.y, 0.0, 1.0);
  let v = max(c.z, 0.0);
  let k = (vec3f(5.0, 3.0, 1.0) + fract(c.x) * 6.0) % 6.0;
  let rgb = v - v * s * clamp(min(k, 4.0 - k), vec3f(0.0), vec3f(1.0));
  return vec4f({DECODE_FN}(rgb), c.a);
}}"
    )
}

fn to_oklch_source() -> String {
    format!(
        "fn {TO_OKLCH_FN}(c: vec4f) -> vec4f {{
  let lms = {} * c.rgb;
  let lab = {} * (sign(lms) * pow(abs(lms), vec3f(1.0 / 3.0)));
  return vec4f(fract(atan2(lab.z, lab.y) / {TAU:.9}), length(lab.yz), lab.x, c.a);
}}",
        wgsl_mat3(&LINEAR_RGB_TO_LMS),
        wgsl_mat3(&LMS_TO_OKLAB),
    )
}

fn from_oklch_source() -> String {
    format!(
        "fn {FROM_OKLCH_FN}(c: vec4f) -> vec4f {{
  let angle = fract(c.x) * {TAU:.9};
  let chroma = max(c.y, 0.0);
  let lms = {} * vec3f(max(c.z, 0.0), chroma * cos(angle), chroma * sin(angle));
  return vec4f({} * (lms * lms * lms), c.a);
}}",
        wgsl_mat3(&OKLAB_TO_LMS),
        wgsl_mat3(&LMS_TO_LINEAR_RGB),
    )
}

fn switch_source(name: &str, hsv: &str, oklch: &str) -> String {
    format!(
        "fn {name}(c: vec4f, space: u32) -> vec4f {{
  switch (space) {{
    case {RAMP_HSV}u: {{ return {hsv}(c); }}
    case {RAMP_OKLCH}u: {{ return {oklch}(c); }}
    default: {{ return c; }}
  }}
}}"
    )
}

fn hue_delta_source() -> String {
    format!(
        "fn {HUE_DELTA_FN}(a: f32, b: f32, mode: u32) -> f32 {{
  let d = fract(b - a);
  switch (mode) {{
    case {HUE_LONGER}u: {{ return select(d, d - 1.0, d < 0.5); }}
    case {HUE_INCREASING}u: {{ return d; }}
    case {HUE_DECREASING}u: {{ return select(d - 1.0, 0.0, d == 0.0); }}
    default: {{ return select(d, d - 1.0, d > 0.5); }}
  }}
}}"
    )
}

fn slope_source() -> String {
    format!(
        "fn {SLOPE_FN}(q0: vec4f, q1: vec4f, q2: vec4f, h0: f32, h1: f32) -> vec4f {{
  let left = select(vec4f(0.0), (q1 - q0) / h0, h0 > 0.0);
  let right = select(vec4f(0.0), (q2 - q1) / h1, h1 > 0.0);
  let span = h0 + h1;
  let middle = select(left, (left * h1 + right * h0) / span, span > 0.0);
  return (sign(left) + sign(right)) * min(min(abs(left), abs(right)), 0.5 * abs(middle));
}}"
    )
}

/// A float as a WGSL literal.
fn f(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.1}")
    } else {
        n.to_string()
    }
}

/// Statements setting `r` to the segment's colour, interpolated as `interpolation` picks.
fn interpolation_lines(ctx: &mut WgslCtx, interpolation: i64) -> Vec<String> {
    match interpolation {
        RAMP_CONSTANT => vec!["r = select(q1, q2, t >= 1.0);".into()],
        RAMP_EASE => vec!["r = mix(q1, q2, t * t * (3.0 - 2.0 * t));".into()],
        RAMP_CARDINAL => {
            let fc = RAMP_CARDINAL_TENSION;
            vec![
                "let t2 = t * t;".into(),
                "let t3 = t2 * t;".into(),
                format!(
                    "r = ({} * t3 + {} * t2 - {} * t) * q0 + ({} * t3 + {} * t2 + 1.0) * q1 + ({} * t3 + {} * t2 + {} * t) * q2 + ({} * t3 - {} * t2) * q3;",
                    f(-fc),
                    f(2.0 * fc),
                    f(fc),
                    f(2.0 - fc),
                    f(fc - 3.0),
                    f(fc - 2.0),
                    f(3.0 - 2.0 * fc),
                    f(fc),
                    f(fc),
                    f(fc),
                ),
            ]
        }
        RAMP_B_SPLINE => vec![
            "let b0 = select(q0, 2.0 * q1 - q2, first);".into(),
            "let b3 = select(q3, 2.0 * q2 - q1, last);".into(),
            "let t2 = t * t;".into(),
            "let t3 = t2 * t;".into(),
            "r = ((-t3 + 3.0 * t2 - 3.0 * t + 1.0) * b0 + (3.0 * t3 - 6.0 * t2 + 4.0) * q1 + (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) * q2 + t3 * b3) / 6.0;".into(),
        ],
        RAMP_SMOOTH => {
            let slope = ctx.define_fn(SLOPE_FN, &slope_source());
            vec![
                "let h = x2 - x1;".into(),
                format!("let m1 = select({slope}(q0, q1, q2, x1 - x0, h), vec4f(0.0), first);"),
                format!("let m2 = select({slope}(q1, q2, q3, h, x3 - x2), vec4f(0.0), last);"),
                "let t2 = t * t;".into(),
                "let t3 = t2 * t;".into(),
                "r = (2.0 * t3 - 3.0 * t2 + 1.0) * q1 + (-2.0 * t3 + 3.0 * t2) * q2 + (t3 - 2.0 * t2 + t) * h * m1 + (t3 - t2) * h * m2;".into(),
            ]
        }
        _ => vec!["r = mix(q1, q2, t);".into()],
    }
}

/// Polar spaces' hues unwrapped along the four stops; mirrors `unwrap_hues`.
fn hue_lines(ctx: &mut WgslCtx) -> Vec<String> {
    let delta = ctx.define_fn(HUE_DELTA_FN, &hue_delta_source());
    let near = format!("{HUE_SHORTER}u");
    let mut lines: Vec<String> = (0..4)
        .map(|i| format!("let grey{i} = q{i}.y < {RAMP_ACHROMATIC};"))
        .collect();
    lines.extend([
        "let h1 = select(select(select(q3.x, q0.x, !grey0), q2.x, !grey2), q1.x, !grey1);".into(),
        format!("let h2 = h1 + select({delta}(q1.x, q2.x, hue), 0.0, grey1 || grey2);"),
        format!("let h0 = select(h1 - {delta}(q0.x, h1, select(hue, {near}, grey1)), h1, grey0);"),
        format!("let h3 = select(h2 + {delta}(h2, q3.x, select(hue, {near}, grey2)), h2, grey3);"),
        "q0.x = h0;".into(),
        "q1.x = h1;".into(),
        "q2.x = h2;".into(),
        "q3.x = h3;".into(),
    ]);
    lines
}

fn variants(list: &[(i64, &str)]) -> Vec<EnumVariant> {
    list.iter()
        .map(|(value, label)| EnumVariant::new(*value, *label))
        .collect()
}

fn indented(lines: Vec<String>, by: &str) -> impl Iterator<Item = String> {
    lines.into_iter().map(move |l| format!("{by}{l}"))
}

/// The function interpolating a segment of a ramp, the enums known while
/// compiling built in (only what they pick is computed), the others switched
/// on per pixel. Mirrors `evaluate_ramp` past finding the segment.
fn ramp_function(
    ctx: &mut WgslCtx,
    interpolation: Option<i64>,
    space: Option<i64>,
    hue: Option<i64>,
) -> String {
    let polar = space == Some(RAMP_HSV) || space == Some(RAMP_OKLCH);
    let known = |n: Option<i64>| n.map_or("any".to_owned(), |n| n.to_string());
    let hue_part = if polar || space.is_none() {
        known(hue)
    } else {
        "none".to_owned()
    };
    let name = format!(
        "color_ramp_{}_{}_{hue_part}",
        known(interpolation),
        known(space)
    );

    // How a colour goes to and from the ramp's space, as expressions.
    let (to, from): (String, String) = match space {
        Some(RAMP_HSV) => {
            ctx.define_fn(ENCODE_FN, &encode_source());
            ctx.define_fn(DECODE_FN, &decode_source());
            (
                ctx.define_fn(TO_HSV_FN, &to_hsv_source()),
                ctx.define_fn(FROM_HSV_FN, &from_hsv_source()),
            )
        }
        Some(RAMP_OKLCH) => (
            ctx.define_fn(TO_OKLCH_FN, &to_oklch_source()),
            ctx.define_fn(FROM_OKLCH_FN, &from_oklch_source()),
        ),
        None => {
            ctx.define_fn(ENCODE_FN, &encode_source());
            ctx.define_fn(DECODE_FN, &decode_source());
            ctx.define_fn(TO_HSV_FN, &to_hsv_source());
            ctx.define_fn(FROM_HSV_FN, &from_hsv_source());
            ctx.define_fn(TO_OKLCH_FN, &to_oklch_source());
            ctx.define_fn(FROM_OKLCH_FN, &from_oklch_source());
            (
                ctx.define_fn(
                    TO_SPACE_FN,
                    &switch_source(TO_SPACE_FN, TO_HSV_FN, TO_OKLCH_FN),
                ),
                ctx.define_fn(
                    FROM_SPACE_FN,
                    &switch_source(FROM_SPACE_FN, FROM_HSV_FN, FROM_OKLCH_FN),
                ),
            )
        }
        Some(_) => (String::new(), String::new()),
    };
    let convert = |function: &str, c: &str| match (space, function.is_empty()) {
        (_, true) => c.to_owned(),
        (None, false) => format!("{function}({c}, space)"),
        (Some(_), false) => format!("{function}({c})"),
    };

    let hues: Vec<String> = match space {
        None => {
            let mut lines = vec![format!(
                "if (space == {RAMP_HSV}u || space == {RAMP_OKLCH}u) {{"
            )];
            lines.extend(indented(hue_lines(ctx), "  "));
            lines.push("}".into());
            lines
        }
        Some(_) if polar => hue_lines(ctx),
        Some(_) => Vec::new(),
    };

    let mixing: Vec<String> = match interpolation {
        None => {
            let mut lines = vec!["switch (interpolation) {".to_owned()];
            for (value, _) in RAMP_INTERPOLATIONS
                .iter()
                .filter(|(v, _)| *v != RAMP_LINEAR)
            {
                lines.push(format!("  case {value}u: {{"));
                lines.extend(indented(interpolation_lines(ctx, *value), "    "));
                lines.push("  }".into());
            }
            lines.push("  default: {".into());
            lines.extend(indented(interpolation_lines(ctx, RAMP_LINEAR), "    "));
            lines.push("  }".into());
            lines.push("}".into());
            lines
        }
        Some(i) => interpolation_lines(ctx, i),
    };

    let mut body: Vec<String> = (0..4)
        .map(|i| format!("var q{i} = {};", convert(&to, &format!("c{i}"))))
        .collect();
    body.extend(hues);
    body.extend([
        "if (first) { q0 = q1; }".to_owned(),
        "if (last) { q3 = q2; }".to_owned(),
        "let t = select(select(0.0, 1.0, x >= x2), clamp((x - x1) / (x2 - x1), 0.0, 1.0), x2 > x1);"
            .to_owned(),
        "var r: vec4f;".to_owned(),
    ]);
    body.extend(mixing);
    body.push(format!("return {};", convert(&from, "r")));
    let source = format!(
        "fn {name}(x: f32, x0: f32, x1: f32, x2: f32, x3: f32, c0: vec4f, c1: vec4f, c2: vec4f, c3: vec4f, first: bool, last: bool, interpolation: u32, space: u32, hue: u32) -> vec4f {{\n{}\n}}",
        indented(body, "  ").collect::<Vec<_>>().join("\n")
    );
    ctx.define_fn(&name, &source)
}

/// An enum input's value while compiling: set by the calling group node, or
/// stored on the node while unlinked. `None` when it's only known per pixel.
fn known_enum(ctx: &WgslCtx, id: &str, variants: &[EnumVariant]) -> Option<i64> {
    if let Some(constant) = ctx.constant(id) {
        return enum_value(Some(&InputValue::from(constant as f64)), variants);
    }
    if !ctx.operands(id).is_empty() {
        return None;
    }
    enum_value(ctx.data.values.get(id), variants)
}

fn enum_input(id: &str, label: &str, variants: Vec<EnumVariant>) -> NodeInputDef {
    NodeInputDef::new(id, label, &[WgslType::U32]).control(InputControl::Enum { variants })
}

pub fn build_color_ramp_descriptor(data: &NodeData) -> NodeDescriptor {
    let stops = color_ramp_stops(data);
    let stop_inputs = stops.iter().enumerate().map(|(i, stop)| {
        NodeInputDef::new(
            stop.id.clone(),
            format!("Stop {}", i + 1),
            &[WgslType::Vec3f, WgslType::Vec4f],
        )
        .family(COLOR_FAMILY)
        .control(InputControl::Color)
    });
    let mut inputs = vec![
        NodeInputDef::new(RAMP_FACTOR, "factor", &[WgslType::F32]).control(InputControl::Number {
            min: None,
            max: None,
            soft_min: Some(0.0),
            soft_max: Some(1.0),
            compiled: false,
        }),
        enum_input(
            RAMP_INTERPOLATION,
            "interpolation",
            variants(&RAMP_INTERPOLATIONS),
        ),
        enum_input(RAMP_SPACE, "color space", variants(&RAMP_SPACES)),
        enum_input(RAMP_HUE, "hue", variants(&RAMP_HUES)),
    ];
    inputs.extend(stop_inputs);

    let mut defaults = NodeData {
        ramp: Some(default_ramp()),
        values: BTreeMap::from([
            (RAMP_FACTOR.to_owned(), 0.5.into()),
            (RAMP_INTERPOLATION.to_owned(), (RAMP_LINEAR as f64).into()),
            (RAMP_SPACE.to_owned(), (RAMP_RGB as f64).into()),
            (RAMP_HUE.to_owned(), (HUE_SHORTER as f64).into()),
        ]),
        ..NodeData::default()
    };
    for stop in default_ramp()
        .iter()
        .filter(|s| stops.iter().any(|k| k.id == s.id))
    {
        let n = stop.position;
        defaults
            .values
            .insert(stop.id.clone(), InputValue::vector(&[n, n, n, 1.0]));
    }

    let primary = stops.first().map(|s| s.id.clone());
    let cpu_stops = stops.clone();
    let mut d = NodeDescriptor::new(COLOR_RAMP, "Color Ramp", NodeCategory::Color)
        .aliases(&[
            "ColorRamp",
            "Color Band",
            "Gradient",
            "Gradient Map",
            "Lookup Table",
            "Palette",
            "Ramp",
            "Heatmap",
        ])
        .inputs(inputs)
        .outputs(vec![NodeOutputDef::new("color", "color", WgslType::Vec3f)])
        .defaults(defaults)
        .cpu(move |ctx| {
            let mode = |id: &str| ctx.input(id).first().copied().unwrap_or(0.0) as i64;
            let width = ctx.output_type.components();
            let colors: Vec<crate::ramp::RampColorStop> = cpu_stops
                .iter()
                .map(|stop| {
                    let c = ctx.input(&stop.id);
                    let at = |i: usize, d: f64| c.get(i).copied().unwrap_or(d);
                    let alpha = if width == 4 { at(3, 1.0) } else { 1.0 };
                    crate::ramp::RampColorStop {
                        position: stop.position,
                        color: [at(0, 0.0), at(1, 0.0), at(2, 0.0), alpha],
                    }
                })
                .collect();
            let factor = ctx.input(RAMP_FACTOR).first().copied().unwrap_or(0.0);
            let modes = crate::ramp::RampModes {
                interpolation: mode(RAMP_INTERPOLATION),
                space: mode(RAMP_SPACE),
                hue: mode(RAMP_HUE),
            };
            let color = crate::ramp::evaluate_ramp(&colors, factor, modes);
            super::cpu::output("color", super::cpu::coerce(&color[..width], ctx.output_type))
        })
        .wgsl(move |ctx| ramp_statements(ctx, &stops));
    d.primary_input = primary;
    d.widget = Some(NodeWidget::Ramp);
    d
}

fn ramp_statements(ctx: &mut WgslCtx, stops: &[RampStop]) -> Vec<String> {
    let v = ctx.var_name.clone();
    let count = stops.len();
    let rgba = ctx.output_type == WgslType::Vec4f;
    let Some(only) = stops.first().filter(|_| count >= 2) else {
        let id = stops.first().map_or("", |s| s.id.as_str());
        return vec![format!("let {v} = {};", ctx.promote_input(id))];
    };
    let _ = only;

    let known = |ctx: &mut WgslCtx, id: &str, variants: Vec<EnumVariant>| {
        let value = known_enum(ctx, id, &variants);
        let arg = match value {
            Some(n) => format!("{n}u"),
            None => ctx.promote_input(id),
        };
        (value, arg)
    };
    let (interpolation, interpolation_arg) =
        known(ctx, RAMP_INTERPOLATION, variants(&RAMP_INTERPOLATIONS));
    let (space, space_arg) = known(ctx, RAMP_SPACE, variants(&RAMP_SPACES));
    let (hue, hue_arg) = known(ctx, RAMP_HUE, variants(&RAMP_HUES));
    let segment = ramp_function(ctx, interpolation, space, hue);

    let n = count;
    let last = count - 1;
    let x = format!("{v}_x");
    let positions = format!("{v}_positions");
    let colors = format!("{v}_colors");
    let i = format!("{v}_segment");
    let k = format!("{v}_k");
    let around = [
        format!("max({i} - 1, 0)"),
        i.clone(),
        format!("{i} + 1"),
        format!("min({i} + 2, {last})"),
    ];
    let mut args = vec![x.clone()];
    args.extend(around.iter().map(|index| format!("{positions}[{index}]")));
    args.extend(around.iter().map(|index| format!("{colors}[{index}]")));
    args.extend([
        format!("{i} == 0"),
        format!("{i} + 2 >= {n}"),
        interpolation_arg,
        space_arg,
        hue_arg,
    ]);
    let position_params: Vec<String> = stops
        .iter()
        .enumerate()
        .map(|(index, s)| ctx.param(&format!("position{index}"), &[s.position]))
        .collect();
    let color_list: Vec<String> = stops
        .iter()
        .map(|s| {
            let color = ctx.promote_input(&s.id);
            if rgba {
                color
            } else {
                format!("vec4f({color}, 1.0)")
            }
        })
        .collect();
    let factor = ctx.promote_input(RAMP_FACTOR);
    vec![
        format!("let {x} = {factor};"),
        format!(
            "var {positions} = array<f32, {n}>({});",
            position_params.join(", ")
        ),
        format!(
            "var {colors} = array<vec4f, {n}>({});",
            color_list.join(", ")
        ),
        format!("var {i} = 0;"),
        format!(
            "for (var {k} = 1; {k} < {last}; {k}++) {{ if ({positions}[{k}] <= {x}) {{ {i} = {k}; }} }}"
        ),
        format!("let {v}_rgba = {segment}({});", args.join(", ")),
        format!(
            "let {v} = {};",
            if rgba {
                format!("{v}_rgba")
            } else {
                format!("{v}_rgba.rgb")
            }
        ),
    ]
}

pub(super) fn color_ramp_node() -> NodeDescriptor {
    build_color_ramp_descriptor(&NodeData::default())
}

/// `stops` as a ramp node's, with `colors` (by stop id) set and the links
/// and colours of the stops it loses dropped. Moving stops without
/// reordering them only changes values the shader reads.
pub fn apply_color_ramp(
    project: &mut GraphProject,
    node_id: &str,
    stops: &[RampStop],
    colors: &BTreeMap<String, InputValue>,
) {
    let Some(key) = project.definition_ref_of(node_id).map(str::to_owned) else {
        return;
    };
    let Some(doc) = project.definitions.get_mut(&key) else {
        return;
    };
    let Some(node) = doc.node_mut(node_id).filter(|n| n.is(COLOR_RAMP)) else {
        return;
    };
    let ramp = sorted_ramp_stops(Some(stops));
    let removed = |handle: &str| is_ramp_stop_id(handle) && !ramp.iter().any(|s| s.id == handle);
    let order = |list: &[RampStop]| list.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    let reordered = order(&ramp) != order(&color_ramp_stops(&node.data));
    node.data.values.retain(|handle, _| !removed(handle));
    node.data
        .values
        .extend(colors.iter().map(|(k, v)| (k.clone(), *v)));
    node.data.ramp = Some(ramp.clone());
    if reordered {
        doc.edges
            .retain(|e| e.target != node_id || !removed(&e.target_handle));
    }
}

impl GraphModel {
    /// Give a Color Ramp `stops`, setting the colours in `colors` (by stop id).
    pub fn set_color_ramp(
        &mut self,
        node_id: &str,
        stops: &[RampStop],
        colors: &BTreeMap<String, InputValue>,
    ) {
        self.edit(|project| apply_color_ramp(project, node_id, stops, colors));
    }

    /// Like `set_color_ramp` for a gesture's steps: after the first, they
    /// replace its undo step instead of adding their own.
    pub fn set_color_ramp_gesture(
        &mut self,
        first: bool,
        node_id: &str,
        stops: &[RampStop],
        colors: &BTreeMap<String, InputValue>,
    ) {
        self.edit_gesture(first, |project| {
            apply_color_ramp(project, node_id, stops, colors)
        });
    }
}

#[cfg(test)]
mod tests;
