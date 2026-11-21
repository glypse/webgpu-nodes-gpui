use super::builders::{declare, model_for, node, shader, valid_shader};
use crate::enums::{CASES_FAMILY, case_handle};
use crate::evaluate::evaluate;
use crate::graph::{GraphModel, GraphProject};
use crate::nodes::INDEX_SWITCH;
use crate::types::{InputValue, NodeData, WgslType};

const CASES: [[f64; 3]; 3] = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];

/// An Index Switch of three vec3f cases, its index linked from a Declare holding `index`.
fn graph_at(index: i32) -> GraphModel {
    let mut data = NodeData { cases: Some(CASES.len() as u32), ..Default::default() };
    data.types.insert(CASES_FAMILY.to_owned(), WgslType::Vec3f);
    for (k, v) in CASES.iter().enumerate() {
        data.values.insert(case_handle(k as i64), InputValue::vector(v));
    }
    let mut switch = node("s", INDEX_SWITCH);
    switch.data = data;
    model_for(
        vec![declare("i", WgslType::I32, &[("x", f64::from(index))]), switch],
        &[("i", "value", "s", "index"), ("s", "result", "out", "color")],
    )
}

#[test]
fn picks_the_case_at_its_index_the_nearest_one_out_of_range() {
    let pick = |index| evaluate(&graph_at(index), "s", "result").expect("evaluates");
    assert_eq!(pick(1), CASES[1]);
    assert_eq!(pick(-3), CASES[0]);
    assert_eq!(pick(9), CASES[2]);
}

#[test]
fn compiles_to_a_table_indexed_at_runtime() {
    let code = valid_shader(&graph_at(2));
    assert!(code.contains("array<vec3f, 3>"), "{code}");
}

#[test]
fn saves_and_loads_its_cases() {
    let model = graph_at(0);
    let loaded = GraphProject::from_json(&model.project().to_json()).expect("loads");
    assert_eq!(loaded.root().node("s").and_then(|n| n.data.cases), Some(CASES.len() as u32));
}

#[test]
fn drops_the_links_and_values_of_the_cases_it_loses() {
    let mut switch = node("s", INDEX_SWITCH);
    switch.data.cases = Some(3);
    switch.data.values.insert(case_handle(2), 5.0.into());
    let mut model = model_for(vec![switch, node("t", "time")], &[("t", "seconds", "s", "case-1")]);
    model.set_index_switch_cases("s", 1);
    let node = model.root().node("s").expect("kept");
    assert_eq!(node.data.cases, Some(1));
    assert!(!node.data.values.contains_key(&case_handle(2)));
    assert!(!model.root().edges.iter().any(|e| e.target == "s"));
    let _ = shader(&model);
}
