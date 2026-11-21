//! Helpers for the CPU implementations of nodes, which compute as JavaScript
//! numbers (f64) rounded to WGSL's types after each node.

use std::collections::BTreeMap;

use crate::types::{CpuCtx, CpuFn, ScalarKind, Value, WgslType};
use std::sync::Arc;

/// Component-wise `f`, a single component standing for all of them.
pub fn zip(values: &[Value], f: impl Fn(&[f64]) -> f64) -> Value {
    let width = values.iter().map(Vec::len).max().unwrap_or(0);
    (0..width)
        .map(|i| {
            let args: Vec<f64> = values
                .iter()
                .map(|v| {
                    let at = if v.len() == 1 { 0 } else { i };
                    v.get(at).copied().unwrap_or(0.0)
                })
                .collect();
            f(&args)
        })
        .collect()
}

/// A value with `width` components, a single one standing for all of them.
pub fn fit(value: &[f64], width: usize) -> Value {
    if value.len() == 1 {
        return vec![value[0]; width];
    }
    (0..width).map(|i| value.get(i).copied().unwrap_or(0.0)).collect()
}

/// A number as JavaScript's `ToInt32`: wrapping, with non-finite numbers 0.
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u64 as u32 as i32
}

pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

/// A number as a value of `kind`: f32s round, integers wrap as in WGSL.
pub fn as_kind(kind: ScalarKind, n: f64) -> f64 {
    match kind {
        ScalarKind::I32 => f64::from(to_int32(n)),
        ScalarKind::U32 => f64::from(to_uint32(n)),
        ScalarKind::Bool => f64::from(u8::from(n != 0.0)),
        ScalarKind::F32 => f64::from(n as f32),
    }
}

/// A value as `ty`: its width and kind.
pub fn coerce(value: &[f64], ty: WgslType) -> Value {
    let kind = ty.kind();
    fit(value, ty.components()).into_iter().map(|n| as_kind(kind, n)).collect()
}

/// JavaScript's `Math.round`: halves go up.
pub fn js_round(n: f64) -> f64 {
    let floor = n.floor();
    if n - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// WGSL's `round`: halves go to the even neighbour.
pub fn round_half_even(n: f64) -> f64 {
    if (n % 1.0).abs() == 0.5 { 2.0 * js_round(n / 2.0) } else { js_round(n) }
}

/// JavaScript's `Math.sign`.
pub fn js_sign(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 { n } else { n.signum() }
}

/// JavaScript's `Math.min`: NaN wins.
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

fn is_integer(ty: WgslType) -> bool {
    matches!(ty.kind(), ScalarKind::I32 | ScalarKind::U32)
}

/// The outputs of a node with one, `result`.
pub fn result(value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([("result".to_owned(), value)])
}

pub fn output(id: &str, value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(id.to_owned(), value)])
}

/// A node computing `f` of each component of `e`, giving `result`.
pub fn unary_cpu(f: impl Fn(f64) -> f64 + Send + Sync + 'static) -> CpuFn {
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let mapped: Value = ctx.input("e").into_iter().map(&f).collect();
        result(coerce(&mapped, ctx.output_type))
    })
}

/// A node folding its `operands` by `f`; integers use `integer` where given, for products past a float's precision.
pub fn variadic_cpu(
    f: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
    integer: Option<fn(f64, f64) -> f64>,
) -> CpuFn {
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let ty = ctx.output_type;
        let op = |a: f64, b: f64| match integer.filter(|_| is_integer(ty)) {
            Some(g) => g(a, b),
            None => f(a, b),
        };
        let operands = ctx.operands("operands");
        let mut rest = operands.iter();
        let first = rest.next().map_or_else(|| vec![0.0], Clone::clone);
        let folded = rest.fold(coerce(&first, ty), |a, b| {
            coerce(&zip(&[a, b.clone()], |n| op(n[0], n[1])), ty)
        });
        result(folded)
    })
}

/// A node computing `f` of the components of two inputs, giving `result`.
pub fn binary_cpu(
    a: &str,
    b: &str,
    f: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
) -> CpuFn {
    nary_cpu(&[a, b], move |n| f(n[0], n[1]))
}

/// A node computing `f` of the components of its inputs, in order, giving `result`.
pub fn nary_cpu(ids: &[&str], f: impl Fn(&[f64]) -> f64 + Send + Sync + 'static) -> CpuFn {
    let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let values: Vec<Value> = ids.iter().map(|id| ctx.input(id)).collect();
        result(coerce(&zip(&values, &f), ctx.output_type))
    })
}

fn setting<'a>(ctx: &'a CpuCtx<'_>, key: &str, default: &'a str) -> &'a str {
    ctx.data.settings.get(key).map_or(default, String::as_str)
}

/// The CPU implementation of a node type with fixed sockets.
pub(super) fn standard_cpu(type_id: &str) -> Option<CpuFn> {
    let f: CpuFn = match type_id {
        "sin" => unary_cpu(f64::sin),
        "cos" => unary_cpu(f64::cos),
        "tan" => unary_cpu(f64::tan),
        "atan" => unary_cpu(f64::atan),
        "floor" => unary_cpu(f64::floor),
        "ceil" => unary_cpu(f64::ceil),
        "fract" => unary_cpu(|n| n - n.floor()),
        "round" => unary_cpu(round_half_even),
        "sqrt" => unary_cpu(f64::sqrt),
        "log" => unary_cpu(f64::ln),
        "exp" => unary_cpu(f64::exp),
        "abs" => unary_cpu(f64::abs),
        "sign" => unary_cpu(js_sign),
        "negate" => unary_cpu(|n| -n),
        "add" => variadic_cpu(|a, b| a + b, None),
        "mul" => variadic_cpu(
            |a, b| a * b,
            Some(|a, b| f64::from(to_int32(a).wrapping_mul(to_int32(b)))),
        ),
        "min" => variadic_cpu(js_min, None),
        "max" => variadic_cpu(js_max, None),
        "sub" => binary_cpu("a", "b", |a, b| a - b),
        "div" => binary_cpu("a", "b", |a, b| if b == 0.0 { 0.0 } else { a / b }),
        "mod" => binary_cpu("a", "b", |a, b| if b == 0.0 { 0.0 } else { a % b }),
        "pow" => binary_cpu("e1", "e2", |a, b| if b == 0.0 { 1.0 } else { a.powf(b) }),
        "step" => binary_cpu("e1", "e2", |edge, x| f64::from(u8::from(x >= edge))),
        "atan2" => binary_cpu("e1", "e2", f64::atan2),
        "clamp" => nary_cpu(&["e", "low", "high"], |n| js_min(n[2], js_max(n[1], n[0]))),
        "mix" => nary_cpu(&["e1", "e2", "e3"], |n| n[0] * (1.0 - n[2]) + n[1] * n[2]),
        "smoothstep" => nary_cpu(&["low", "high", "x"], |n| {
            let t = js_min(1.0, js_max(0.0, (n[2] - n[0]) / (n[1] - n[0])));
            t * t * (3.0 - 2.0 * t)
        }),
        "dot" => Arc::new(|ctx: &CpuCtx<'_>| {
            let products = zip(&[ctx.input("e1"), ctx.input("e2")], |n| n[0] * n[1]);
            result(coerce(&[products.iter().sum()], ctx.output_type))
        }),
        "distance" => Arc::new(|ctx: &CpuCtx<'_>| {
            let diff = zip(&[ctx.input("e1"), ctx.input("e2")], |n| n[0] - n[1]);
            result(vec![hypot(&diff) as f32 as f64])
        }),
        "length" => Arc::new(|ctx: &CpuCtx<'_>| result(vec![hypot(&ctx.input("e1")) as f32 as f64])),
        "normalize" => Arc::new(|ctx: &CpuCtx<'_>| {
            let v = ctx.input("e");
            let length = hypot(&v);
            let scaled: Value = v.iter().map(|n| n / length).collect();
            result(coerce(&scaled, ctx.output_type))
        }),
        "cross" => Arc::new(|ctx: &CpuCtx<'_>| {
            let (a, d) = (fit(&ctx.input("e1"), 3), fit(&ctx.input("e2"), 3));
            let v = [a[1] * d[2] - a[2] * d[1], a[2] * d[0] - a[0] * d[2], a[0] * d[1] - a[1] * d[0]];
            result(coerce(&v, WgslType::Vec3f))
        }),
        "separate-vector" => Arc::new(|ctx: &CpuCtx<'_>| {
            let v = ctx.input("v");
            ["x", "y", "z", "w"]
                .iter()
                .enumerate()
                .map(|(i, c)| ((*c).to_owned(), vec![v.get(i).copied().unwrap_or(0.0)]))
                .collect()
        }),
        "component-count" => Arc::new(|ctx: &CpuCtx<'_>| {
            output("count", vec![ctx.input_type("value").components() as f64])
        }),
        "compare" => Arc::new(|ctx: &CpuCtx<'_>| {
            let op: fn(f64, f64) -> bool = match setting(ctx, "op", "<") {
                "<=" => |a, b| a <= b,
                "==" => |a, b| a == b,
                "!=" => |a, b| a != b,
                ">=" => |a, b| a >= b,
                ">" => |a, b| a > b,
                _ => |a, b| a < b,
            };
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| f64::from(u8::from(op(n[0], n[1]))));
            result(coerce(&v, ctx.output_type))
        }),
        "switch" => Arc::new(|ctx: &CpuCtx<'_>| {
            let values = [ctx.input("condition"), ctx.input("false"), ctx.input("true")];
            let v = zip(&values, |n| if n[0] != 0.0 { n[2] } else { n[1] });
            result(coerce(&v, ctx.output_type))
        }),
        "boolean-math" => Arc::new(|ctx: &CpuCtx<'_>| {
            let op: fn(bool, bool) -> bool = match setting(ctx, "op", "and") {
                "or" => |a, b| a | b,
                "xor" => |a, b| a != b,
                "nand" => |a, b| !(a & b),
                "nor" => |a, b| !(a | b),
                "xnor" => |a, b| a == b,
                _ => |a, b| a & b,
            };
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| {
                f64::from(u8::from(op(n[0] != 0.0, n[1] != 0.0)))
            });
            result(coerce(&v, ctx.output_type))
        }),
        "not" => unary_cpu(|n| f64::from(u8::from(n == 0.0))),
        "reduce-boolean" => Arc::new(|ctx: &CpuCtx<'_>| {
            let values = ctx.input("e");
            let any = setting(ctx, "op", "any") == "any";
            let reduced =
                if any { values.iter().any(|n| *n != 0.0) } else { values.iter().all(|n| *n != 0.0) };
            result(vec![f64::from(u8::from(reduced))])
        }),
        "fallback" => Arc::new(|ctx: &CpuCtx<'_>| {
            let chosen = if ctx.is_present("value") { "value" } else { "default" };
            result(coerce(&ctx.input(chosen), ctx.output_type))
        }),
        "bitwise" => Arc::new(|ctx: &CpuCtx<'_>| {
            let op: fn(i32, i32) -> i32 = match setting(ctx, "op", "&") {
                "|" => |a, b| a | b,
                "^" => |a, b| a ^ b,
                _ => |a, b| a & b,
            };
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| {
                f64::from(op(to_int32(n[0]), to_int32(n[1])))
            });
            result(coerce(&v, ctx.output_type))
        }),
        "bitwise-not" => unary_cpu(|n| f64::from(!to_int32(n))),
        "bit-shift" => Arc::new(|ctx: &CpuCtx<'_>| {
            let right = setting(ctx, "op", "<<") == ">>";
            let unsigned = ctx.output_type.kind() == ScalarKind::U32;
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| {
                let by = (to_int32(n[1]) & 31) as u32;
                match (right, unsigned) {
                    (true, true) => f64::from(to_uint32(n[0]) >> by),
                    (true, false) => f64::from(to_int32(n[0]) >> by),
                    _ => f64::from(to_int32(n[0]) << by),
                }
            });
            result(coerce(&v, ctx.output_type))
        }),
        "declare" => Arc::new(|ctx: &CpuCtx<'_>| {
            output(
                "value",
                ctx.resolved_inputs.iter().map(|i| ctx.input(&i.id).first().copied().unwrap_or(0.0)).collect(),
            )
        }),
        "reroute" => Arc::new(|ctx: &CpuCtx<'_>| output("out", ctx.input("in"))),
        _ => return None,
    };
    Some(f)
}

/// `Math.hypot`: the length of a vector.
fn hypot(v: &[f64]) -> f64 {
    v.iter().map(|n| n * n).sum::<f64>().sqrt()
}
