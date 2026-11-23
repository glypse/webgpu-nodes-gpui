//! The Curve node.

use crate::curve::{CURVE_SOLVE_STEPS, CurvePoint, curve_segments, linear_curve};
use crate::types::{
    InputControl, NodeCategory, NodeDescriptor, NodeInputDef, NodeOutputDef, NodeWidget, WgslType,
};

use super::builders::{FLOATS, float_values};

const SEGMENT_FN: &str = "curveSegment";
const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

/// y at `x` on a Bezier segment packed as `a` = (start, first control) and
/// `b` = (second control, end). Solves the same way `segment_parameter` does.
fn segment_source() -> String {
    format!(
        "fn {SEGMENT_FN}(x: f32, a: vec4f, b: vec4f) -> f32 {{
  let p0 = a.xy;
  let c0 = a.zw;
  let c1 = b.xy;
  let p1 = b.zw;
  var low = 0.0;
  var high = 1.0;
  var t = 0.5;
  if (p1.x > p0.x) {{ t = clamp((x - p0.x) / (p1.x - p0.x), 0.0, 1.0); }}
  for (var i = 0; i < {CURVE_SOLVE_STEPS}; i++) {{
    let u = 1.0 - t;
    let error = u * u * u * p0.x + 3.0 * u * u * t * c0.x + 3.0 * u * t * t * c1.x + t * t * t * p1.x - x;
    if (abs(error) < 1e-7) {{ break; }}
    if (error < 0.0) {{ low = t; }} else {{ high = t; }}
    let slope = 3.0 * u * u * (c0.x - p0.x) + 6.0 * u * t * (c1.x - c0.x) + 3.0 * t * t * (p1.x - c1.x);
    let next = t - error / slope;
    t = select(0.5 * (low + high), next, slope > 0.0 && next > low && next < high);
  }}
  let u = 1.0 - t;
  return u * u * u * p0.y + 3.0 * u * u * t * c0.y + 3.0 * u * t * t * c1.y + t * t * t * p1.y;
}}"
    )
}

/// y at `x` on a curve of `count` segments, flat past its ends.
fn curve_source(name: &str, count: usize) -> String {
    let size = 2 * count;
    let last = size - 1;
    format!(
        "fn {name}(x: f32, segments: array<vec4f, {size}>) -> f32 {{
  var s = segments;
  if (x <= s[0].x) {{ return s[0].y; }}
  for (var i = 0; i < {count}; i++) {{
    if (x <= s[2 * i + 1].z) {{ return {SEGMENT_FN}(x, s[2 * i], s[2 * i + 1]); }}
  }}
  return s[{last}].w;
}}"
    )
}

/// Maps each component of a value through the curve drawn on the node, mixed
/// with the value by a factor (past 1, the mapping is exaggerated).
pub(super) fn curve_node() -> NodeDescriptor {
    let mut defaults = float_values([("factor", 1.0), ("value", 0.0)]);
    defaults.curve = Some(linear_curve());
    let mut d = NodeDescriptor::new("curve", "Curve", NodeCategory::Math)
        .aliases(&[
            "Float Curve",
            "Vector Curves",
            "RGB Curves",
            "Curves",
            "Tone Curve",
            "Lookup",
            "Easing",
            "Ease",
        ])
        .inputs(vec![
            NodeInputDef::new("factor", "factor", &[WgslType::F32]).control(InputControl::Number {
                min: Some(0.0),
                max: None,
                soft_min: None,
                soft_max: Some(1.0),
                compiled: false,
            }),
            NodeInputDef::new("value", "value", FLOATS),
        ])
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .primary("value")
        .defaults(defaults)
        .cpu(|ctx| {
            let points: Vec<CurvePoint> = ctx.data.curve.clone().unwrap_or_else(linear_curve);
            let factor = ctx.input("factor").first().copied().unwrap_or(0.0);
            let mapped: Vec<f64> = ctx
                .input("value")
                .into_iter()
                .map(|x| x * (1.0 - factor) + crate::curve::evaluate_curve(&points, x) * factor)
                .collect();
            super::cpu::result(super::cpu::coerce(&mapped, ctx.output_type))
        })
        .wgsl(|ctx| {
            let points: Vec<CurvePoint> = ctx.data.curve.clone().unwrap_or_else(linear_curve);
            let segments = curve_segments(&points);
            let value = format!("{}_value", ctx.var_name);
            let mut lines = vec![format!("let {value} = {};", ctx.promote_input("value"))];
            let map: Box<dyn Fn(&str) -> String> = if segments.is_empty() {
                let level = ctx.param("level", &[points.first().map_or(0.0, |p| p.y)]);
                Box::new(move |_| level.clone())
            } else {
                ctx.define_fn(SEGMENT_FN, &segment_source());
                let name = format!("curve{}", segments.len());
                ctx.define_fn(&name, &curve_source(&name, segments.len()));
                let mut packed = Vec::new();
                for (i, [p0, c0, c1, p1]) in segments.iter().enumerate() {
                    packed.push(ctx.param(&format!("segment{i}a"), &[p0.x, p0.y, c0.x, c0.y]));
                    packed.push(ctx.param(&format!("segment{i}b"), &[c1.x, c1.y, p1.x, p1.y]));
                }
                let array = format!("{}_segments", ctx.var_name);
                lines.push(format!(
                    "let {array} = array<vec4f, {}>({});",
                    packed.len(),
                    packed.join(", ")
                ));
                Box::new(move |x| format!("{name}({x}, {array})"))
            };
            let ty = ctx.output_type;
            let n = ty.components();
            let mapped = if n == 1 {
                map(&value)
            } else {
                let each: Vec<String> = COMPONENTS[..n]
                    .iter()
                    .map(|c| map(&format!("{value}.{c}")))
                    .collect();
                format!("{}({})", ty.wgsl_name(), each.join(", "))
            };
            let factor = ctx.promote_input("factor");
            lines.push(format!(
                "let {} = mix({value}, {mapped}, {factor});",
                ctx.var_name
            ));
            lines
        });
    d.widget = Some(NodeWidget::Curve);
    d
}
