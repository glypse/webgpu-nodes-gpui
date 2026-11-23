use crate::graph::{GraphModel, LAYOUT_FRAME};
use crate::testing::TestGraph;

fn frame_in(g: &mut TestGraph, parent: Option<&str>) -> String {
    let id = g.add(LAYOUT_FRAME);
    set_frame(g, &id, parent);
    id
}

fn set_frame(g: &mut TestGraph, node: &str, frame: Option<&str>) {
    let node = node.to_owned();
    let frame = frame.map(str::to_owned);
    g.model.edit(|p| {
        if let Some(n) = p.root_mut().node_mut(&node) {
            n.frame = frame;
        }
    });
}

fn frame_of(model: &GraphModel, node: &str) -> Option<String> {
    model.root().node(node).and_then(|n| n.frame.clone())
}

#[test]
fn frames_are_left_out_of_the_shader() {
    let mut plain = TestGraph::new();
    let t = plain.add("time");
    let out = plain.output();
    plain.link((&t, "seconds"), (&out, "color"));

    let mut framed = TestGraph::new();
    let f = framed.add(LAYOUT_FRAME);
    let t = framed.add("time");
    set_frame(&mut framed, &t, Some(&f));
    let out = framed.output();
    framed.link((&t, "seconds"), (&out, "color"));
    let normalised = |code: String| code.lines().map(|l| l.len()).collect::<Vec<_>>();
    assert_eq!(normalised(framed.wgsl()), normalised(plain.wgsl()));
}

#[test]
fn nodes_are_framed_in_the_frame_they_share_and_leave_one_frame_at_a_time() {
    let mut g = TestGraph::new();
    let outer = g.add(LAYOUT_FRAME);
    let (a, b) = (g.add("sin"), g.add("sin"));
    set_frame(&mut g, &a, Some(&outer));
    set_frame(&mut g, &b, Some(&outer));
    let id = g
        .model
        .frame_nodes(&[a.clone(), b.clone()])
        .expect("frames");
    let of = |g: &TestGraph, n: &str| g.model.active_frames().frame_of.get(n).cloned();
    assert_eq!(of(&g, &id), Some(outer.clone()));
    assert_eq!(of(&g, &a), Some(id.clone()));
    assert_eq!(g.model.selected_node_ids(), [id]);
    assert!(g.model.remove_from_frame(std::slice::from_ref(&a)));
    assert_eq!(of(&g, &a), Some(outer));
    assert!(g.model.remove_from_frame(std::slice::from_ref(&a)));
    assert_eq!(of(&g, &a), None);
    assert!(!g.model.remove_from_frame(&[a]));
}

#[test]
fn a_frame_never_goes_in_itself_or_a_frame_it_holds() {
    let mut g = TestGraph::new();
    let outer = frame_in(&mut g, None);
    let inner = frame_in(&mut g, Some(&outer));
    let a = g.add("sin");
    assert!(
        !g.model
            .set_nodes_frame(std::slice::from_ref(&outer), Some(&inner))
    );
    assert!(
        !g.model
            .set_nodes_frame(std::slice::from_ref(&outer), Some(&outer))
    );
    assert!(g.model.set_nodes_frame(&[a.clone(), outer], Some(&inner)));
    assert_eq!(frame_of(&g.model, &a), Some(inner));
    assert!(g.model.set_nodes_frame(std::slice::from_ref(&a), None));
    assert_eq!(frame_of(&g.model, &a), None);
    assert!(!g.model.set_nodes_frame(std::slice::from_ref(&a), Some(&a)));
}

#[test]
fn a_deleted_frame_keeps_what_it_held_in_the_frame_around_it() {
    let mut g = TestGraph::new();
    let outer = frame_in(&mut g, None);
    let inner = frame_in(&mut g, Some(&outer));
    let a = g.add("sin");
    set_frame(&mut g, &a, Some(&inner));
    g.model.delete_nodes(&[inner], &[]);
    assert_eq!(frame_of(&g.model, &a), Some(outer));
}

#[test]
fn a_frame_is_copied_with_what_it_holds() {
    let mut g = TestGraph::new();
    let f = g.add(LAYOUT_FRAME);
    let a = g.add("sin");
    g.add("sin");
    set_frame(&mut g, &a, Some(&f));
    let copied = g.model.duplicate_nodes(&[f]);
    assert_eq!(copied.len(), 2);
    let nodes: Vec<_> = copied.iter().filter_map(|id| g.model.node(id)).collect();
    let frame_copy = nodes
        .iter()
        .find(|n| n.is(LAYOUT_FRAME))
        .map(|n| n.id.clone());
    let held = nodes
        .iter()
        .find(|n| n.is("sin"))
        .and_then(|n| n.frame.clone());
    assert!(frame_copy.is_some());
    assert_eq!(held, frame_copy);
}

#[test]
fn a_frame_is_grouped_with_what_it_holds_the_group_staying_in_the_frame_around_them() {
    let mut g = TestGraph::new();
    let outer = frame_in(&mut g, None);
    let f = frame_in(&mut g, Some(&outer));
    let (a, b) = (g.add("sin"), g.add("sin"));
    set_frame(&mut g, &a, Some(&f));
    set_frame(&mut g, &b, Some(&outer));
    g.link((&a, "result"), (&b, "e"));
    let group = g
        .model
        .group_selection(std::slice::from_ref(&f))
        .expect("groups");
    let node = g.model.node(&group).expect("group node");
    assert_eq!(node.frame.as_deref(), Some(outer.as_str()));
    let definition = g
        .model
        .project()
        .document(node.definition_ref().expect("ref"))
        .expect("definition");
    assert_eq!(
        definition.node(&a).and_then(|n| n.frame.clone()),
        Some(f.clone())
    );
    assert_eq!(definition.node(&f).and_then(|n| n.frame.clone()), None);
}

#[test]
fn ungrouping_puts_unframed_nodes_in_the_group_nodes_frame() {
    let mut g = TestGraph::new();
    let outer = frame_in(&mut g, None);
    let a = g.add("sin");
    let group = g
        .model
        .group_selection(std::slice::from_ref(&a))
        .expect("groups");
    set_frame(&mut g, &group, Some(&outer));
    let fresh = g.model.ungroup_selection(&[group]);
    assert_eq!(fresh.len(), 1);
    assert_eq!(frame_of(&g.model, &fresh[0]), Some(outer));
}

#[test]
fn frame_data_survives_a_round_trip() {
    let mut g = TestGraph::new();
    let f = g.add(LAYOUT_FRAME);
    let a = g.add("sin");
    set_frame(&mut g, &a, Some(&f));
    g.model.update_node_data(&f, |d| {
        crate::graph::set_frame_tint(d, Some(crate::graph::FrameTint::Teal));
        crate::graph::set_frame_note(d, Some("what it does"));
    });
    let loaded =
        crate::graph::GraphProject::from_json(&g.model.project().to_json()).expect("loads");
    let node = loaded.root().node(&f).expect("frame");
    assert_eq!(
        crate::graph::frame_tint(&node.data),
        Some(crate::graph::FrameTint::Teal)
    );
    assert_eq!(crate::graph::frame_note(&node.data), Some("what it does"));
    assert_eq!(
        loaded.root().node(&a).and_then(|n| n.frame.clone()),
        Some(f)
    );
}
