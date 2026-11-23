//! Helpers for the CPU implementations of nodes. Each node computes in f64
//! and rounds its result to the WGSL type of the output: f32 rounds, float to
//! integer conversions saturate (NaN gives 0), and integer arithmetic wraps.
//! Rounding once per node, from f64, is at least as accurate as the GPU's f32
//! steps, so results agree with it to within f32 precision, not bit for bit.

use std::collections::BTreeMap;

use super::{FALLBACK, FALLBACK_DEFAULT, FALLBACK_VALUE};
use crate::graph::REROUTE;
use crate::types::{CpuCtx, InputValue, CpuFn, ScalarKind, Value, WgslType};
use std::sync::Arc;

/// Components of the widest node input.
const MAX_ARGS: usize = 4;

/// Component-wise `f`, a single component standing for all of them.
pub fn zip(values: &[Value], f: impl Fn(&[f64]) -> f64) -> Value {
    let width = values.iter().map(Vec::len).max().unwrap_or(0);
    let count = values.len().min(MAX_ARGS);
    (0..width)
        .map(|i| {
            let mut args = [0.0; MAX_ARGS];
            for (slot, v) in args.iter_mut().zip(values) {
                let at = if v.len() == 1 { 0 } else { i };
                *slot = v.get(at).copied().unwrap_or(0.0);
            }
            f(&args[..count])
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

/// An integer result of kind `kind`, wrapped to its 32 bits.
fn wrap_int(kind: ScalarKind, n: i64) -> f64 {
    match kind {
        ScalarKind::U32 => f64::from(n as u32),
        _ => f64::from(n as i32),
    }
}

/// A number as a value of `kind`: f32s round, integers saturate (NaN is 0).
pub fn as_kind(kind: ScalarKind, n: f64) -> f64 {
    match kind {
        ScalarKind::I32 => f64::from(n as i32),
        ScalarKind::U32 => f64::from(n as u32),
        ScalarKind::Bool => f64::from(u8::from(n != 0.0)),
        ScalarKind::F32 => f64::from(n as f32),
    }
}

/// The value an unlinked input holds, as `ty`.
pub fn stored_value(stored: Option<&InputValue>, ty: WgslType) -> Value {
    let components: Vec<f64> = (0..ty.components()).map(|i| stored.map_or(0.0, |v| v.component(i))).collect();
    coerce(&components, ty)
}

/// A value as `ty`: its width and kind.
pub fn coerce(value: &[f64], ty: WgslType) -> Value {
    let kind = ty.kind();
    fit(value, ty.components()).into_iter().map(|n| as_kind(kind, n)).collect()
}

/// WGSL's `round`: halves go to the even neighbour.
pub fn round_half_even(n: f64) -> f64 {
    n.round_ties_even()
}

/// WGSL's `sign`: 0 for 0.
pub fn sign(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 { n } else { n.signum() }
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
    unary_int_cpu(f, None)
}

/// As [`unary_cpu`], with integer outputs computed by `integer` on wrapping 32-bit values.
pub fn unary_int_cpu(
    f: impl Fn(f64) -> f64 + Send + Sync + 'static,
    integer: Option<fn(i64) -> i64>,
) -> CpuFn {
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let ty = ctx.output_type;
        let mapped: Value = match integer.filter(|_| is_integer(ty)) {
            Some(g) => ctx.input("e").into_iter().map(|n| wrap_int(ty.kind(), g(n as i64))).collect(),
            None => ctx.input("e").into_iter().map(&f).collect(),
        };
        result(coerce(&mapped, ty))
    })
}

/// A node folding its `operands` by `f`; integers use `integer` where given, which wraps.
pub fn variadic_cpu(
    f: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
    integer: Option<fn(i64, i64) -> i64>,
) -> CpuFn {
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let ty = ctx.output_type;
        let op = |a: f64, b: f64| match integer.filter(|_| is_integer(ty)) {
            Some(g) => wrap_int(ty.kind(), g(a as i64, b as i64)),
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

/// A node computing `f` of the components of two inputs, giving `result`;
/// integers use `integer` where given, which wraps.
pub fn binary_cpu(
    a: &str,
    b: &str,
    f: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
    integer: Option<fn(i64, i64) -> i64>,
) -> CpuFn {
    let (a, b) = (a.to_owned(), b.to_owned());
    Arc::new(move |ctx: &CpuCtx<'_>| {
        let ty = ctx.output_type;
        let op = |n: &[f64]| match integer.filter(|_| is_integer(ty)) {
            Some(g) => wrap_int(ty.kind(), g(n[0] as i64, n[1] as i64)),
            None => f(n[0], n[1]),
        };
        result(coerce(&zip(&[ctx.input(&a), ctx.input(&b)], op), ty))
    })
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
        "abs" => unary_int_cpu(f64::abs, Some(i64::abs)),
        "sign" => unary_cpu(sign),
        "negate" => unary_int_cpu(|n| -n, Some(i64::wrapping_neg)),
        "add" => variadic_cpu(|a, b| a + b, Some(i64::wrapping_add)),
        "mul" => variadic_cpu(|a, b| a * b, Some(i64::wrapping_mul)),
        "min" => variadic_cpu(f64::min, None),
        "max" => variadic_cpu(f64::max, None),
        "sub" => binary_cpu("a", "b", |a, b| a - b, Some(i64::wrapping_sub)),
        "div" => binary_cpu("a", "b", |a, b| if b == 0.0 { 0.0 } else { a / b }, None),
        "mod" => binary_cpu("a", "b", |a, b| if b == 0.0 { 0.0 } else { a % b }, None),
        "pow" => binary_cpu("e1", "e2", |a, b| if b == 0.0 { 1.0 } else { a.powf(b) }, None),
        "step" => binary_cpu("e1", "e2", |edge, x| f64::from(u8::from(x >= edge)), None),
        "atan2" => binary_cpu("e1", "e2", f64::atan2, None),
        "clamp" => nary_cpu(&["e", "low", "high"], |n| n[2].min(n[1].max(n[0]))),
        "mix" => nary_cpu(&["e1", "e2", "e3"], |n| n[0] * (1.0 - n[2]) + n[1] * n[2]),
        "smoothstep" => nary_cpu(&["low", "high", "x"], |n| {
            let t = ((n[2] - n[0]) / (n[1] - n[0])).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }),
        "dot" => Arc::new(|ctx: &CpuCtx<'_>| {
            let products = zip(&[ctx.input("e1"), ctx.input("e2")], |n| n[0] * n[1]);
            result(coerce(&[products.iter().sum()], ctx.output_type))
        }),
        "distance" => Arc::new(|ctx: &CpuCtx<'_>| {
            let diff = zip(&[ctx.input("e1"), ctx.input("e2")], |n| n[0] - n[1]);
            result(vec![length(&diff) as f32 as f64])
        }),
        "length" => Arc::new(|ctx: &CpuCtx<'_>| result(vec![length(&ctx.input("e1")) as f32 as f64])),
        "normalize" => Arc::new(|ctx: &CpuCtx<'_>| {
            let v = ctx.input("e");
            let len = length(&v);
            let scaled: Value = v.iter().map(|n| n / len).collect();
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
        FALLBACK => Arc::new(|ctx: &CpuCtx<'_>| {
            let chosen = if ctx.is_present(FALLBACK_VALUE) { FALLBACK_VALUE } else { FALLBACK_DEFAULT };
            result(coerce(&ctx.input(chosen), ctx.output_type))
        }),
        "bitwise" => Arc::new(|ctx: &CpuCtx<'_>| {
            let ty = ctx.output_type;
            let op: fn(i64, i64) -> i64 = match setting(ctx, "op", "&") {
                "|" => |a, b| a | b,
                "^" => |a, b| a ^ b,
                _ => |a, b| a & b,
            };
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| {
                wrap_int(ty.kind(), op(n[0] as i64, n[1] as i64))
            });
            result(coerce(&v, ty))
        }),
        "bitwise-not" => unary_int_cpu(|n| f64::from(!(n as i32)), Some(|n| !n)),
        "bit-shift" => Arc::new(|ctx: &CpuCtx<'_>| {
            let right = setting(ctx, "op", "<<") == ">>";
            let ty = ctx.output_type;
            let v = zip(&[ctx.input("a"), ctx.input("b")], |n| {
                let (a, by) = (n[0] as i64, (n[1] as i64 & 31) as u32);
                match (right, ty.kind()) {
                    (true, ScalarKind::U32) => f64::from((a as u32) >> by),
                    (true, _) => f64::from((a as i32) >> by),
                    _ => wrap_int(ty.kind(), a << by),
                }
            });
            result(coerce(&v, ty))
        }),
        "declare" => Arc::new(|ctx: &CpuCtx<'_>| {
            output(
                "value",
                ctx.resolved_inputs.iter().map(|i| ctx.input(&i.id).first().copied().unwrap_or(0.0)).collect(),
            )
        }),
        REROUTE => Arc::new(|ctx: &CpuCtx<'_>| output("out", ctx.input("in"))),
        _ => return None,
    };
    Some(f)
}

/// The length of a vector.
fn length(v: &[f64]) -> f64 {
    v.iter().map(|n| n * n).sum::<f64>().sqrt()
}
