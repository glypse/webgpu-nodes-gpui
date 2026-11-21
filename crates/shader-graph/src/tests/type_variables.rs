use std::collections::BTreeMap;

use super::builders::{Link, declare, edges, instance, node, root_project, valid_shader};
use crate::evaluate::evaluate;
use crate::graph::{GraphDocument, GraphModel, GraphProject, GroupSide};
use crate::types::{
    GroupHandleDef, HandleType, NodeData, TypeVariable, WgslType, FLOAT_TYPES, NUMERIC_TYPES, ZoneItem,
};

const GROUP: &str = "pairs";
const T: &str = "t-id";
const U: &str = "u-id";

fn variable(id: &str, name: &str, types: &[WgslType]) -> TypeVariable {
    TypeVariable { id: id.to_owned(), name: name.to_owned(), types: types.to_vec() }
}

fn by(id: &str, label: &str, variable: &str) -> GroupHandleDef {
    GroupHandleDef::new(id, label, HandleType::Variable { variable: variable.to_owned() })
}

fn with_source(project: &mut GraphProject, id: &str, ty: WgslType, x: f64, link: Link) {
    let root = project.root_mut();
    root.nodes.push(declare(id, ty, &[("x", x), ("y", x + 1.0), ("z", x + 2.0)]));
    root.edges.extend(edges(&[link]));
}

/// A group adding two `T`s and multiplying two `U`s: two independent pairs.
fn project(a: Option<WgslType>, c: Option<WgslType>) -> GraphProject {
    let mut project = root_project(
        vec![super::builders::instance("g", GROUP, NodeData::default())],
        &[("g", "sum", "out", "color")],
    );
    if let Some(ty) = a {
        with_source(&mut project, "sa", ty, 1.0, ("sa", "value", "g", "a"));
    }
    if let Some(ty) = c {
        with_source(&mut project, "sc", ty, 3.0, ("sc", "value", "g", "c"));
    }
    let def = GraphDocument {
        label: Some("Pairs".to_owned()),
        type_variables: vec![variable(T, "T", FLOAT_TYPES), variable(U, "U", NUMERIC_TYPES)],
        inputs: vec![by("a", "A", T), by("b", "B", T), by("c", "C", U), by("d", "D", U)],
        defaults: BTreeMap::from([("b".to_owned(), 10.0.into()), ("d".to_owned(), 2.0.into())]),
        outputs: vec![by("sum", "Sum", T), by("product", "Product", U)],
        nodes: vec![node("in", "group-input"), node("add", "add"), node("mul", "mul"), node("outs", "group-output")],
        edges: edges(&[
            ("in", "a", "add", "operands"),
            ("in", "b", "add", "operands"),
            ("in", "c", "mul", "operands"),
            ("in", "d", "mul", "operands"),
            ("add", "result", "outs", "sum"),
            ("mul", "result", "outs", "product"),
        ]),
        ..Default::default()
    };
    project.definitions.insert(GROUP.to_owned(), def);
    project.reindex();
    project
}

fn bindings_of(model: &GraphModel) -> BTreeMap<String, WgslType> {
    model.variable_choices("g").into_iter().map(|c| (c.variable.name, c.ty)).collect()
}

fn named(entries: &[(&str, WgslType)]) -> BTreeMap<String, WgslType> {
    entries.iter().map(|(k, t)| ((*k).to_owned(), *t)).collect()
}

#[test]
fn each_variable_is_decided_from_its_own_sockets_links() {
    let model = GraphModel::new(project(Some(WgslType::Vec3f), Some(WgslType::I32)));
    assert_eq!(bindings_of(&model), named(&[("T", WgslType::Vec3f), ("U", WgslType::I32)]));
    assert_eq!(model.resolve_input_type("g", "b"), WgslType::Vec3f);
    assert_eq!(model.resolve_input_type("g", "d"), WgslType::I32);
    assert_eq!(model.resolve_output_handle_type("g", "sum"), WgslType::Vec3f);
    assert_eq!(model.resolve_output_handle_type("g", "product"), WgslType::I32);
}

#[test]
fn variables_take_their_first_type_with_nothing_linked() {
    let model = GraphModel::new(project(None, None));
    assert_eq!(bindings_of(&model), named(&[("T", WgslType::F32), ("U", WgslType::F32)]));
    let choices = model.variable_choices("g");
    assert!(choices[0].inferable);
    assert_eq!(choices[1].forced, None);
}

#[test]
fn a_variable_can_be_forced_on_the_group_node_keeping_what_links_infer_apart() {
    let mut model = GraphModel::new(project(Some(WgslType::Vec2f), None));
    model.set_variable_type("g", U, Some(WgslType::Vec4i));
    assert_eq!(bindings_of(&model), named(&[("T", WgslType::Vec2f), ("U", WgslType::Vec4i)]));
    let u = model.variable_choices("g").into_iter().find(|c| c.variable.id == U).expect("U");
    assert_eq!((u.forced, u.inferred), (Some(WgslType::Vec4i), WgslType::F32));
    model.set_variable_type("g", U, None);
    assert_eq!(bindings_of(&model)["U"], WgslType::F32);
}

#[test]
fn each_group_node_is_computed_with_its_own_types() {
    let model = GraphModel::new(project(Some(WgslType::Vec3f), Some(WgslType::I32)));
    assert_eq!(evaluate(&model, "g", "sum").expect("evaluates"), [11.0, 12.0, 13.0]);
    assert_eq!(evaluate(&model, "g", "product").expect("evaluates"), [6.0]);
    let code = valid_shader(&model);
    assert!(code.contains("t_vec3f_i32("), "{code}");
    assert!(code.contains("struct GroupOutputs_h_pairs__vec3f_i32 {"), "{code}");
}

#[test]
fn one_function_is_emitted_per_combination_of_types() {
    let mut base = project(Some(WgslType::Vec3f), None);
    let mut copy = base.root().node("g").cloned().expect("group node");
    copy.id = "h".to_owned();
    base.root_mut().nodes.push(copy);
    base.root_mut().edges.extend(edges(&[("h", "sum", "out", "color")]));
    base.reindex();
    let code = valid_shader(&GraphModel::new(base));
    assert!(code.contains("fn group_h_pairs__u1__t_vec3f_f32("), "{code}");
    assert!(code.contains("fn group_h_pairs__u1__t_f32_f32("), "{code}");
}

#[test]
fn a_groups_inside_shows_the_types_of_the_group_node_it_was_entered_through() {
    let mut model = GraphModel::new(project(Some(WgslType::Vec3f), None));
    assert_eq!(model.resolve_output_handle_type("add", "result"), WgslType::F32);
    model.enter_group("g");
    assert_eq!(model.resolve_output_handle_type("add", "result"), WgslType::Vec3f);
    assert_eq!(model.resolve_output_handle_type("in", "a"), WgslType::Vec3f);
    model.exit_group();
    assert_eq!(model.resolve_output_handle_type("add", "result"), WgslType::F32);
}

#[test]
fn removed_variables_fix_their_sockets_to_their_first_type() {
    let mut model = GraphModel::new(project(Some(WgslType::Vec3f), None));
    model.remove_type_variable(GROUP, T);
    let handle = model.project().document(GROUP).and_then(|d| d.inputs.iter().find(|h| h.id == "a").cloned());
    assert_eq!(handle.map(|h| h.ty), Some(HandleType::Fixed(WgslType::F32)));
    let names: Vec<String> = model.variable_choices("g").into_iter().map(|c| c.variable.name).collect();
    assert_eq!(names, ["U"]);
}

#[test]
fn a_narrower_set_drops_forced_types_and_links_it_no_longer_allows() {
    let mut model = GraphModel::new(project(None, Some(WgslType::Vec3i)));
    model.set_variable_type("g", U, Some(WgslType::Vec3i));
    model.set_type_variable(GROUP, U, None, Some(&[WgslType::F32]));
    assert_eq!(model.root().node("g").and_then(|n| n.data.types.get(U).copied()), None);
    assert!(!model.root().edges.iter().any(|e| e.source == "sc"));
    assert_eq!(bindings_of(&model)["U"], WgslType::F32);
}

#[test]
fn variables_survive_saving_and_loading() {
    let model = GraphModel::new(project(Some(WgslType::Vec2f), None));
    let loaded = GraphModel::new(GraphProject::from_json(&model.project().to_json()).expect("loads"));
    assert_eq!(bindings_of(&loaded), named(&[("T", WgslType::Vec2f), ("U", WgslType::F32)]));
}

#[test]
fn new_variables_are_named_after_the_next_free_letter() {
    let mut model = GraphModel::new(project(None, None));
    assert_eq!(model.add_type_variable(GROUP, FLOAT_TYPES).map(|v| v.name).as_deref(), Some("V"));
}

fn infer(model: &mut GraphModel) {
    model.set_handle_type(GROUP, GroupSide::Output, "sum", HandleType::Inferred { inferred: true });
}

#[test]
fn inferred_outputs_carry_whatever_reaches_them_inside_per_group_node() {
    let mut model = GraphModel::new(project(Some(WgslType::Vec3f), None));
    infer(&mut model);
    assert_eq!(model.resolve_output_handle_type("g", "sum"), WgslType::Vec3f);
    let code = valid_shader(&model);
    assert!(code.contains("struct GroupOutputs_h_pairs__vec3f_f32 {"), "{code}");
}

#[test]
fn inferred_outputs_follow_edits_inside_the_group() {
    let mut model = GraphModel::new(project(Some(WgslType::Vec2f), None));
    infer(&mut model);
    model.enter_group("g");
    model.set_node_type("add", Some(WgslType::Vec4f));
    model.exit_group();
    assert_eq!(model.resolve_output_handle_type("g", "sum"), WgslType::Vec4f);
}

#[test]
fn inferred_types_are_only_for_outputs() {
    let mut model = GraphModel::new(project(None, None));
    model.set_handle_type(GROUP, GroupSide::Input, "a", HandleType::Inferred { inferred: true });
    let handle = model.project().document(GROUP).and_then(|d| d.inputs.iter().find(|h| h.id == "a").cloned());
    assert_eq!(handle.map(|h| h.ty), Some(HandleType::Variable { variable: T.to_owned() }));
}

const ZONED: &str = "zoned";

/// A group whose zone adds A to an item, starting at A, twice: 3 x A, as wide as A.
fn zoned(source: WgslType) -> GraphProject {
    let mut project = root_project(
        vec![instance("g", ZONED, NodeData::default()), declare("sa", source, &[("x", 1.0), ("y", 2.0), ("z", 3.0)])],
        &[("sa", "value", "g", "a"), ("g", "sum", "out", "color")],
    );
    let mut repeat_input = node("ri", "repeat-input");
    repeat_input.data.paired_output = Some("ro".to_owned());
    repeat_input.data.values = super::builders::values(&[("iterations", 2.0)]);
    let mut repeat_output = node("ro", "repeat-output");
    repeat_output.data.items =
        Some(vec![ZoneItem::new("acc", "Acc", HandleType::Variable { variable: T.to_owned() })]);
    let def = GraphDocument {
        label: Some("Zoned".to_owned()),
        type_variables: vec![variable(T, "T", FLOAT_TYPES)],
        inputs: vec![by("a", "A", T)],
        outputs: vec![by("sum", "Sum", T)],
        nodes: vec![node("in", "group-input"), repeat_input, node("add", "add"), repeat_output, node("outs", "group-output")],
        edges: edges(&[
            ("in", "a", "ri", "acc"),
            ("ri", "acc", "add", "operands"),
            ("in", "a", "add", "operands"),
            ("add", "result", "ro", "acc"),
            ("ro", "acc", "outs", "sum"),
        ]),
        ..Default::default()
    };
    project.definitions.insert(ZONED.to_owned(), def);
    project.reindex();
    project
}

#[test]
fn zone_items_carry_the_type_their_variable_takes_for_each_group_node() {
    let mut model = GraphModel::new(zoned(WgslType::Vec3f));
    assert_eq!(evaluate(&model, "g", "sum").expect("evaluates"), [3.0, 6.0, 9.0]);
    model.enter_group("g");
    assert_eq!(model.resolve_output_handle_type("ri", "acc"), WgslType::Vec3f);
    assert_eq!(model.resolve_output_handle_type("ro", "acc"), WgslType::Vec3f);
    let code = super::builders::shader(&model);
    assert!(code.contains(": vec3f = "), "{code}");
    crate::testing::validate_wgsl(&code);
}

#[test]
fn zone_items_are_fixed_to_their_variables_first_type_when_its_removed() {
    let mut model = GraphModel::new(zoned(WgslType::Vec2f));
    model.remove_type_variable(ZONED, T);
    let ty = model
        .project()
        .document(ZONED)
        .and_then(|d| d.node("ro")?.data.items.as_ref()?.first().map(|i| i.ty.clone()));
    assert_eq!(ty, Some(HandleType::Fixed(WgslType::F32)));
    let _: Vec<Link> = Vec::new();
    let _ = edges;
}
