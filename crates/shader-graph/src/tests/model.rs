use crate::graph::{Connection, GraphModel, GroupSide, Point};
use crate::testing::TestGraph;
use crate::types::{HandleType, WgslType};

#[test]
fn links_must_not_narrow_or_cycle() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let sin = g.add("sin");
    // vec2f into a scalar-or-vector input is fine for a float family.
    assert!(
        g.model
            .is_valid_connection(&Connection::new(&uv, "uv", &sin, "e"))
    );
    let sub = g.add("sub");
    let sub2 = g.add("sub");
    g.link((&sub, "result"), (&sub2, "a"));
    assert!(
        !g.model
            .is_valid_connection(&Connection::new(&sub2, "result", &sub, "a"))
    );
    // A vec3f can't go into a scalar input.
    let scalar = g.add("declare");
    let v3 = g.add("declare");
    g.force_type(&v3, WgslType::Vec3f);
    assert!(
        !g.model
            .is_valid_connection(&Connection::new(&v3, "value", &scalar, "x"))
    );
}

#[test]
fn a_new_link_replaces_the_one_it_displaces() {
    let mut g = TestGraph::new();
    let a = g.add("time");
    let b = g.add("delta");
    let sin = g.add("sin");
    g.link((&a, "seconds"), (&sin, "e"));
    g.link((&b, "delta"), (&sin, "e"));
    let doc = g.model.root();
    assert_eq!(doc.edges.iter().filter(|e| e.target == sin).count(), 1);
}

#[test]
fn undo_and_redo_restore_the_project() {
    let mut g = TestGraph::new();
    let before = g.model.root().nodes.len();
    let sin = g.add("sin");
    assert_eq!(g.model.root().nodes.len(), before + 1);
    assert!(g.model.undo());
    assert_eq!(g.model.root().nodes.len(), before);
    assert!(g.model.redo());
    assert!(g.model.root().node(&sin).is_some());
}

#[test]
fn deleting_a_zone_node_deletes_its_partner() {
    let mut m = GraphModel::default();
    let input = m
        .create_zone(crate::zones::ZoneKind::Repeat, Point::default())
        .unwrap();
    let nodes_before = m.root().nodes.len();
    assert!(m.delete_nodes(&[input], &[]));
    assert_eq!(m.root().nodes.len(), nodes_before - 2);
}

#[test]
fn copy_and_paste_keeps_links_and_pairs() {
    let mut g = TestGraph::new();
    let a = g.add("time");
    let sin = g.add("sin");
    g.link((&a, "seconds"), (&sin, "e"));
    let clip = g.model.copy_nodes(&[a.clone(), sin.clone()]).unwrap();
    let ids = g.model.paste_nodes(&clip, Some(Point::new(100.0, 100.0)));
    assert_eq!(ids.len(), 2);
    let edges = &g.model.root().edges;
    assert_eq!(edges.len(), 2);
}

#[test]
fn grouping_a_selection_keeps_the_shader_valid() {
    let mut g = TestGraph::new();
    let time = g.add("time");
    let mul = g.add("mul");
    let sin = g.add("sin");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&time, "seconds"), (&mul, "operands"));
    g.link((&mul, "result"), (&sin, "e"));
    g.link((&sin, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    let group = g
        .model
        .group_selection(&[mul.clone(), sin.clone()])
        .expect("groups");
    assert!(g.model.root().node(&group).is_some());
    let wgsl = g.wgsl();
    assert!(wgsl.contains("fn group_"), "{wgsl}");
    let ungrouped = g.model.ungroup_selection(&[group]);
    assert_eq!(ungrouped.len(), 2);
    g.wgsl();
}

#[test]
fn group_interface_edits_keep_links_consistent() {
    let mut g = TestGraph::new();
    let group = g.model.create_group(Point::default()).unwrap();
    let reference = g
        .model
        .node(&group)
        .unwrap()
        .definition_ref()
        .unwrap()
        .to_owned();
    let extra = g
        .model
        .add_handle(
            &reference,
            GroupSide::Input,
            HandleType::Fixed(WgslType::Vec2f),
            None,
            None,
        )
        .unwrap();
    let time = g.add("time");
    g.link((&time, "seconds"), (&group, &extra.id));
    g.model
        .remove_handle(&reference, GroupSide::Input, &extra.id);
    assert!(g.model.root().edges.iter().all(|e| e.target != group));
}

#[test]
fn generic_groups_follow_the_types_linked_to_them() {
    let mut g = TestGraph::new();
    let group = g.model.create_group(Point::default()).unwrap();
    let reference = g
        .model
        .node(&group)
        .unwrap()
        .definition_ref()
        .unwrap()
        .to_owned();
    let variable = g
        .model
        .add_type_variable(&reference, &[WgslType::F32, WgslType::Vec3f])
        .unwrap();
    let def = g.model.project().document(&reference).unwrap();
    let (input, output) = (def.inputs[0].id.clone(), def.outputs[0].id.clone());
    g.model.set_handle_type(
        &reference,
        GroupSide::Input,
        &input,
        HandleType::Variable {
            variable: variable.id.clone(),
        },
    );
    g.model.set_handle_type(
        &reference,
        GroupSide::Output,
        &output,
        HandleType::Variable {
            variable: variable.id.clone(),
        },
    );
    let v3 = g.add("declare");
    g.force_type(&v3, WgslType::Vec3f);
    g.link((&v3, "value"), (&group, &input));
    assert_eq!(
        g.model.resolve_output_handle_type(&group, &output),
        WgslType::Vec3f
    );
    let out = g.output();
    g.link((&group, &output), (&out, "color"));
    g.wgsl();
}

#[test]
fn projects_round_trip_through_json() {
    let mut g = TestGraph::new();
    let a = g.add("time");
    let sin = g.add("sin");
    g.link((&a, "seconds"), (&sin, "e"));
    g.set(&sin, "e", 0.25);
    let json = g.model.project().to_json();
    let loaded = crate::graph::GraphProject::from_json(&json).expect("loads");
    assert_eq!(&loaded, g.model.project());
}

#[test]
fn rejects_projects_with_unknown_nodes() {
    let json = r#"{"definitions":{"__root__":{"nodes":[{"id":"a","type":"nope","data":{},"position":{"x":0,"y":0}}],"edges":[]}}}"#;
    assert!(crate::graph::GraphProject::from_json(json).is_err());
}

fn drag_node(g: &mut TestGraph, node: &str, steps: usize) {
    for i in 0..steps {
        let id = node.to_owned();
        g.model.edit_gesture(i == 0, |project| {
            if let Some(n) = project.root_mut().node_mut(&id) {
                n.position.x = (i + 1) as f32;
            }
        });
    }
}

#[test]
fn a_gesture_is_one_undo_step_with_its_final_state() {
    let mut g = TestGraph::new();
    let node = g.add("sin");
    drag_node(&mut g, &node, 100);
    g.model.end_gesture();
    assert_eq!(
        g.model.root().node(&node).map(|n| n.position.x),
        Some(100.0_f32)
    );
    assert!(g.model.undo());
    assert_eq!(
        g.model.root().node(&node).map(|n| n.position.x),
        Some(0.0_f32)
    );
    assert!(g.model.redo());
    assert_eq!(
        g.model.root().node(&node).map(|n| n.position.x),
        Some(100.0_f32)
    );
}

#[test]
fn unsaved_changes_are_seen_mid_gesture() {
    let mut g = TestGraph::new();
    let node = g.add("sin");
    g.model.mark_saved();
    drag_node(&mut g, &node, 1);
    g.model.end_gesture();
    g.model.mark_saved();
    drag_node(&mut g, &node, 5);
    assert!(g.model.has_unsaved_changes());
    let marker = g.model.saved_marker();
    g.model.mark_saved_as(marker);
    assert!(!g.model.has_unsaved_changes());
}

#[test]
fn undo_mid_gesture_keeps_the_gesture_as_one_step() {
    let mut g = TestGraph::new();
    let node = g.add("sin");
    drag_node(&mut g, &node, 10);
    assert!(g.model.undo());
    assert_eq!(
        g.model.root().node(&node).map(|n| n.position.x),
        Some(0.0_f32)
    );
    assert!(g.model.redo());
    assert_eq!(
        g.model.root().node(&node).map(|n| n.position.x),
        Some(10.0_f32)
    );
}
