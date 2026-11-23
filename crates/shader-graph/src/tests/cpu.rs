use crate::nodes::registered_nodes;
use crate::testing::TestGraph;
use crate::types::WgslType;

#[test]
fn f32_results_round_as_the_gpu_does() {
    let mut g = TestGraph::new();
    let (a, b, sum) = (g.add("declare"), g.add("declare"), g.add("add"));
    g.set(&a, "x", 0.1).set(&b, "x", 0.2);
    g.link((&a, "value"), (&sum, "operands")).link((&b, "value"), (&sum, "operands"));
    let [value] = g.eval(&sum, "result")[..] else { panic!("one component") };
    assert_eq!(value, f64::from(0.1f32 + 0.2f32));
    assert_ne!(value, 0.1 + 0.2);
}

#[test]
fn integers_wrap_as_wgsls_do() {
    let mut g = TestGraph::new();
    let sum = g.add("add");
    g.force_type(&sum, WgslType::I32);
    for _ in 0..3 {
        let a = g.add("declare");
        g.force_type(&a, WgslType::I32).set(&a, "x", f64::from(1u32 << 30));
        g.link((&a, "value"), (&sum, "operands"));
    }
    assert_eq!(g.eval(&sum, "result"), [-f64::from(1u32 << 30)]);
}

#[test]
fn only_nodes_with_a_meaning_on_the_cpu_have_one() {
    let has = |ty: &str| registered_nodes().iter().any(|d| d.type_id == ty && d.cpu.is_some());
    for ty in ["add", "sin", "declare", "switch", "color-ramp", "curve", "reroute"] {
        assert!(has(ty), "{ty} computes on the CPU");
    }
    for ty in ["uv", "sample-texture", "custom-wgsl", "image"] {
        assert!(!has(ty), "{ty} doesn't");
    }
}

#[test]
fn rounding_goes_to_the_even_neighbour() {
    let mut g = TestGraph::new();
    let r = g.add("round");
    let mut at = |x: f64| {
        g.set(&r, "e", x);
        g.eval(&r, "result")[0]
    };
    assert_eq!(at(0.5), 0.0);
    assert_eq!(at(1.5), 2.0);
    assert_eq!(at(2.5), 2.0);
    assert_eq!(at(-0.5), 0.0);
    assert_eq!(at(-1.5), -2.0);
}

#[test]
fn a_switch_picks_by_its_condition() {
    let mut g = TestGraph::new();
    let s = g.add("switch");
    g.set(&s, "false", 3.0).set(&s, "true", 7.0);
    assert_eq!(g.eval(&s, "result"), [3.0]);
    g.set(&s, "condition", true);
    assert_eq!(g.eval(&s, "result"), [7.0]);
}

#[test]
fn division_by_zero_is_zero() {
    let mut g = TestGraph::new();
    let d = g.add("div");
    g.set(&d, "a", 4.0);
    assert_eq!(g.eval(&d, "result"), [0.0]);
}

fn convert(ty: &str, x: f64) -> f64 {
    let mut g = TestGraph::new();
    let c = g.add(ty);
    g.set(&c, "e", x);
    g.eval(&c, "result")[0]
}

#[test]
fn float_to_integer_saturates() {
    assert_eq!(convert("to-int", 3e9), 2_147_483_647.0);
    assert_eq!(convert("to-int", -3e9), -2_147_483_648.0);
    assert_eq!(convert("to-uint", -1.0), 0.0);
    assert_eq!(convert("to-uint", 5e9), 4_294_967_295.0);
}

#[test]
fn nan_converts_to_zero() {
    assert_eq!(convert("to-int", f64::NAN), 0.0);
    assert_eq!(convert("to-uint", f64::NAN), 0.0);
}

fn integer_op(ty: &str, wgsl_type: WgslType, a: f64, b: f64) -> f64 {
    let mut g = TestGraph::new();
    let node = g.add(ty);
    g.force_type(&node, wgsl_type);
    if ty == "sub" {
        g.set(&node, "a", a).set(&node, "b", b);
    } else {
        for x in [a, b] {
            let d = g.add("declare");
            g.force_type(&d, wgsl_type).set(&d, "x", x);
            g.link((&d, "value"), (&node, "operands"));
        }
    }
    g.eval(&node, "result")[0]
}

#[test]
fn integer_arithmetic_wraps_both_ways() {
    let max = f64::from(i32::MAX);
    assert_eq!(integer_op("add", WgslType::I32, max, 1.0), f64::from(i32::MIN));
    assert_eq!(integer_op("sub", WgslType::I32, f64::from(i32::MIN), 1.0), max);
    assert_eq!(integer_op("mul", WgslType::I32, max, 2.0), -2.0);
    assert_eq!(integer_op("add", WgslType::U32, f64::from(u32::MAX), 1.0), 0.0);
    assert_eq!(integer_op("sub", WgslType::U32, 0.0, 1.0), f64::from(u32::MAX));
}

#[test]
fn round_takes_halves_to_even() {
    assert_eq!(convert("round", 2.5), 2.0);
    assert_eq!(convert("round", 3.5), 4.0);
}
