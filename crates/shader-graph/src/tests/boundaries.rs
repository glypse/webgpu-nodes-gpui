use std::collections::BTreeMap;

use super::builders::{declare, edges, instance, model_for, node, root_project, shader, valid_shader, values};
use crate::catalog::{EntryKind, add_menu_entries};
use crate::evaluate::evaluate;
use crate::graph::{Connection, GraphDocument, GraphModel, GraphProject};
use crate::types::{GroupHandleDef, HandleType, NodeData, TypeVariable, WgslType};

fn handle(id: &str, label: &str, ty: WgslType) -> GroupHandleDef {
    GroupHandleDef::new(id, label, HandleType::Fixed(ty))
}

/// A group doubling its input `a` into `twice` and tripling it into
/// `thrice`, through two Group Input and two Group Output nodes.
fn multiples() -> GraphProject {
    let mut project = root_project(
        vec![
            super::builders::with_values(node("two", "declare"), &[("x", 2.0.into())]),
            instance("g", "def", NodeData::default()),
            node("sum", "add"),
        ],
        &[
            ("two", "value", "g", "a"),
            ("g", "twice", "sum", "operands"),
            ("g", "thrice", "sum", "operands"),
            ("sum", "result", "out", "color"),
        ],
    );
    let mut in2 = node("in2", "group-input");
    in2.data.hide_unlinked = Some(true);
    let mut double = node("double", "mul");
    double.data.values = values(&[("b", 2.0)]);
    let mut three = node("three", "declare");
    three.data.values = values(&[("x", 3.0)]);
    let def = GraphDocument {
        label: Some("Multiples".to_owned()),
        inputs: vec![handle("a", "A", WgslType::F32)],
        outputs: vec![handle("twice", "Twice", WgslType::F32), handle("thrice", "Thrice", WgslType::F32)],
        nodes: vec![
            node("in1", "group-input"),
            in2,
            double,
            node("triple", "mul"),
            three,
            node("out1", "group-output"),
            node("out2", "group-output"),
        ],
        edges: edges(&[
            ("in1", "a", "double", "operands"),
            ("in1", "a", "double", "operands"),
            ("in2", "a", "triple", "operands"),
            ("three", "value", "triple", "operands"),
            ("double", "result", "out1", "twice"),
            ("triple", "result", "out2", "thrice"),
        ]),
        ..Default::default()
    };
    project.definitions.insert("def".to_owned(), def);
    project.reindex();
    project
}

#[test]
fn several_boundary_nodes_all_show_the_groups_inputs_and_each_output_comes_from_the_node_its_linked_on() {
    let model = GraphModel::new(multiples());
    let outputs = model.types_of("in2", Some(&["g".to_owned()])).map(|_| ());
    assert!(outputs.is_some());
    assert_eq!(evaluate(&model, "g", "twice").expect("evaluates"), [4.0]);
    assert_eq!(evaluate(&model, "g", "thrice").expect("evaluates"), [6.0]);
    valid_shader(&model);
}

#[test]
fn an_output_is_linked_on_one_group_output_node_at_a_time() {
    let mut model = GraphModel::new(multiples());
    model.enter_group("g");
    assert!(model.connect(Connection::new("double", "result", "out2", "twice")));
    let into = |id: &str| -> Vec<String> {
        let mut handles: Vec<String> =
            model.active_document().edges_into(id).map(|e| e.target_handle.clone()).collect();
        handles.sort();
        handles
    };
    assert!(into("out1").is_empty());
    assert_eq!(into("out2"), ["thrice", "twice"]);
}

#[test]
fn boundary_nodes_are_pasted_back_into_their_group_only() {
    let mut model = GraphModel::new(multiples());
    model.enter_group("g");
    let clipboard = model.copy_nodes(&["in1".to_owned(), "double".to_owned()]).expect("copied");
    assert_eq!(model.paste_nodes(&clipboard, None).len(), 2);
    model.exit_group();
    assert_eq!(model.paste_nodes(&clipboard, None).len(), 1);
}

#[test]
fn boundary_nodes_are_offered_in_the_add_menu_inside_groups_only() {
    let mut model = GraphModel::new(multiples());
    let boundaries = |m: &GraphModel| -> Vec<String> {
        add_menu_entries(m)
            .into_iter()
            .filter(|e| matches!(&e.kind, EntryKind::Node { node_type } if node_type == "group-input" || node_type == "group-output"))
            .map(|e| e.label)
            .collect()
    };
    assert!(boundaries(&model).is_empty());
    model.enter_group("g");
    assert_eq!(boundaries(&model), ["Group Input", "Group Output"]);
}

#[test]
fn boundary_nodes_hide_their_unlinked_sockets_all_of_them_or_none() {
    let mut model = GraphModel::new(multiples());
    model.enter_group("g");
    let ids = ["in1".to_owned(), "in2".to_owned()];
    let hidden = |m: &GraphModel| -> Vec<Option<bool>> {
        ids.iter().map(|id| m.active_document().node(id).and_then(|n| n.data.hide_unlinked)).collect()
    };
    model.toggle_unlinked_sockets(&ids);
    assert_eq!(hidden(&model), [Some(true), Some(true)]);
    model.toggle_unlinked_sockets(&ids);
    assert_eq!(hidden(&model), [None, None]);
}

/// A group generic over T (vec3f or vec2f) negating its vector's Z, and an
/// instance per type; `linked` is the one feeding the output.
fn unavailable_project(linked: &'static str) -> GraphProject {
    let mut project = root_project(
        vec![
            instance("g2", "def", NodeData { types: BTreeMap::from([("T".to_owned(), WgslType::Vec2f)]), ..Default::default() }),
            instance("g3", "def", NodeData { types: BTreeMap::from([("T".to_owned(), WgslType::Vec3f)]), ..Default::default() }),
        ],
        &[],
    );
    project.root_mut().edges = edges(&[(linked, "r", "out", "color")]);
    let def = GraphDocument {
        type_variables: vec![TypeVariable {
            id: "T".to_owned(),
            name: "T".to_owned(),
            types: vec![WgslType::Vec3f, WgslType::Vec2f],
        }],
        inputs: vec![GroupHandleDef::new("v", "V", HandleType::Variable { variable: "T".to_owned() })],
        outputs: vec![handle("r", "R", WgslType::F32)],
        nodes: vec![node("in", "group-input"), node("sep", "separate-vector"), node("neg", "negate"), node("go", "group-output")],
        edges: edges(&[("in", "v", "sep", "v"), ("sep", "z", "neg", "e"), ("neg", "result", "go", "r")]),
        ..Default::default()
    };
    project.definitions.insert("def".to_owned(), def);
    project.reindex();
    project
}

const Z_LINK: &str = "sep.z->neg.e";

#[test]
fn links_to_sockets_that_come_and_go_with_types_are_kept_when_edited_through_an_instance_without_them() {
    let mut model = GraphModel::new(unavailable_project("g3"));
    model.enter_group("g2");
    model.update_node_data("sep", |d| d.label = Some("Split".to_owned()));
    assert!(model.project().document("def").is_some_and(|d| d.edges.iter().any(|e| e.id == Z_LINK)));
}

#[test]
fn links_are_unavailable_only_where_their_socket_is_missing() {
    let mut model = GraphModel::new(unavailable_project("g3"));
    model.enter_group("g3");
    assert!(!model.unavailable_links("def").contains(Z_LINK));
    model.exit_group();
    model.enter_group("g2");
    assert!(model.unavailable_links("def").contains(Z_LINK));
}

#[test]
fn links_come_back_with_their_socket() {
    let mut model = model_for(
        vec![declare("d", WgslType::Vec3f, &[]), node("s", "separate-vector")],
        &[("d", "value", "s", "v"), ("s", "z", "out", "color")],
    );
    model.set_node_type("d", Some(WgslType::Vec2f));
    assert!(model.unavailable_links(crate::graph::ROOT_ID).contains("s.z->out.color"));
    model.set_node_type("d", Some(WgslType::Vec3f));
    assert!(model.unavailable_links(crate::graph::ROOT_ID).is_empty());
}

#[test]
fn links_are_left_out_of_the_shader_of_an_instance_without_them() {
    valid_shader(&GraphModel::new(unavailable_project("g2")));
    let _ = shader;
}
