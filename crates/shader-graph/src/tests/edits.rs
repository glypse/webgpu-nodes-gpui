use crate::graph::Point;
use crate::testing::TestGraph;
use crate::types::{HandleType, WgslType};

fn chain() -> (TestGraph, String, String, String) {
    let mut g = TestGraph::new();
    let a = g.add("sin");
    let b = g.add("sin");
    g.link((&a, "result"), (&b, "e"));
    let edge = g.model.root().edges.iter().find(|e| e.source == a).expect("linked").id.clone();
    (g, a, b, edge)
}

#[test]
fn a_link_is_routed_through_a_reroute() {
    let (mut g, a, b, edge) = chain();
    let ids = g.model.reroute_edges(&[(edge.clone(), Point::new(50.0, 60.0))]);
    let [id] = ids.as_slice() else { panic!("one reroute") };
    assert_eq!(g.model.node(id).map(|n| n.position), Some(Point::new(50.0, 60.0)));
    let edges = &g.model.root().edges;
    assert!(edges.iter().any(|e| e.source == a && e.source_handle == "result" && e.target == *id));
    assert!(edges.iter().any(|e| e.source == *id && e.target == b && e.target_handle == "e"));
    assert!(!edges.iter().any(|e| e.id == edge));
}

#[test]
fn links_leaving_the_same_output_share_one_reroute() {
    let mut g = TestGraph::new();
    let (a, b, c) = (g.add("sin"), g.add("sin"), g.add("sin"));
    g.link((&a, "result"), (&b, "e"));
    g.link((&a, "result"), (&c, "e"));
    let edge = |to: &str| g.model.root().edges.iter().find(|e| e.target == to).expect("linked").id.clone();
    let cuts = [(edge(&b), Point::new(0.0, 0.0)), (edge(&c), Point::new(10.0, 20.0))];
    let ids = g.model.reroute_edges(&cuts);
    let [id] = ids.as_slice() else { panic!("one reroute") };
    assert_eq!(g.model.node(id).map(|n| n.position), Some(Point::new(5.0, 10.0)));
    let edges = &g.model.root().edges;
    let targets: Vec<&str> = edges.iter().filter(|e| e.source == *id).map(|e| e.target.as_str()).collect();
    assert_eq!(targets, [b.as_str(), c.as_str()]);
    assert_eq!(edges.iter().filter(|e| e.target == *id).count(), 1);
}

#[test]
fn rerouting_nothing_changes_nothing() {
    let (mut g, ..) = chain();
    assert!(g.model.reroute_edges(&[("missing".to_owned(), Point::default())]).is_empty());
}

#[test]
fn hidden_links_toggle_on_reroutes_only() {
    let mut g = TestGraph::new();
    let r = g.add("reroute");
    let s = g.add("sin");
    let hidden = |g: &TestGraph, id: &str| g.model.node(id).and_then(|n| n.data.hide_links);
    g.model.toggle_hidden_links(&[r.clone(), s.clone()]);
    assert_eq!(hidden(&g, &r), Some(true));
    assert_eq!(hidden(&g, &s), None);
    g.model.toggle_hidden_links(std::slice::from_ref(&r));
    assert_eq!(hidden(&g, &r), None);
}

#[test]
fn a_group_nodes_variable_follows_its_links_until_one_is_forced() {
    let mut g = TestGraph::new();
    let sin = g.add("sin");
    let group = g.model.group_selection(&[sin]).expect("groups");
    let reference = g.model.node(&group).and_then(|n| n.definition_ref().map(str::to_owned)).expect("ref");
    let variable = g
        .model
        .add_type_variable(&reference, &[WgslType::F32, WgslType::Vec2f, WgslType::Vec3f])
        .expect("variable");
    let input = g.model.add_handle(&reference, crate::graph::GroupSide::Input, HandleType::Variable { variable: variable.id.clone() }, None, None);
    assert!(input.is_some());
    let choice = |g: &TestGraph| g.model.variable_choices(&group).into_iter().next().expect("a variable");
    let before = choice(&g);
    assert!(before.inferable);
    assert_eq!(before.forced, None);
    assert_eq!(before.ty, WgslType::F32);
    g.model.set_variable_type(&group, &variable.id, Some(WgslType::Vec3f));
    let forced = choice(&g);
    assert_eq!(forced.forced, Some(WgslType::Vec3f));
    assert_eq!(forced.ty, WgslType::Vec3f);
    assert_eq!(forced.inferred, WgslType::F32);
}
