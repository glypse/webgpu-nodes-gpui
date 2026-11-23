use super::builders::{model_for, node};
use crate::graph::{GraphModel, Point};
use crate::zones::{ZoneKind, paired_output_id};

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Three Sine nodes in a chain: a -> b -> c, 200 apart.
fn chain() -> GraphModel {
    let mut nodes = vec![node("a", "sin"), node("b", "sin"), node("c", "sin")];
    for (i, n) in nodes.iter_mut().enumerate() {
        n.position = Point::new(i as f32 * 200.0, 0.0);
    }
    model_for(
        nodes,
        &[("a", "result", "b", "e"), ("b", "result", "c", "e")],
    )
}

#[test]
fn pastes_copies_with_their_inner_links_selected_at_the_position_given() {
    let mut model = chain();
    let clipboard = model.copy_nodes(&ids(&["a", "b"])).expect("copied");
    assert_eq!(clipboard.edges.len(), 1);
    let pasted = model.paste_nodes(&clipboard, Some(Point::new(1000.0, 500.0)));
    assert_eq!(pasted.len(), 2);
    let positions: Vec<Option<Point>> = pasted
        .iter()
        .map(|id| model.root().node(id).map(|n| n.position))
        .collect();
    assert_eq!(
        positions,
        [
            Some(Point::new(1000.0, 500.0)),
            Some(Point::new(1200.0, 500.0))
        ]
    );
    assert_eq!(model.selected_node_ids(), pasted);
    assert!(
        model
            .root()
            .edges
            .iter()
            .any(|e| e.source == pasted[0] && e.target == pasted[1])
    );
}

#[test]
fn never_copies_a_documents_output() {
    assert!(chain().copy_nodes(&ids(&["out"])).is_none());
}

#[test]
fn copies_a_zone_whole() {
    let mut model = chain();
    let zone = model
        .create_zone(ZoneKind::Repeat, Point::default())
        .expect("created");
    let clipboard = model
        .copy_nodes(std::slice::from_ref(&zone))
        .expect("copied");
    let mut types: Vec<&str> = clipboard
        .nodes
        .iter()
        .map(|n| n.node_type.as_str())
        .collect();
    types.sort_unstable();
    assert_eq!(types, ["repeat-input", "repeat-output"]);
    let pasted = model.duplicate_nodes(std::slice::from_ref(&zone));
    let input = pasted
        .iter()
        .filter_map(|id| model.root().node(id))
        .find(|n| n.is("repeat-input"))
        .expect("input");
    let partner = pasted.iter().find(|id| **id != input.id);
    assert_eq!(paired_output_id(input), partner.map(String::as_str));
}

#[test]
fn deletes_nodes_with_their_links() {
    let mut model = chain();
    assert!(model.delete_nodes(&ids(&["b"]), &[]));
    assert!(model.root().node("b").is_none());
    assert!(model.root().edges.is_empty());
    assert!(!model.delete_nodes(&ids(&["out"]), &[]));
}

#[test]
fn dissolves_nodes_linking_what_fed_them_to_what_they_fed() {
    let mut model = chain();
    assert!(model.dissolve_nodes(&ids(&["b"])));
    let edges = &model.root().edges;
    assert_eq!(edges.len(), 1);
    let e = &edges[0];
    assert_eq!(
        (
            e.source.as_str(),
            e.source_handle.as_str(),
            e.target.as_str(),
            e.target_handle.as_str()
        ),
        ("a", "result", "c", "e")
    );
}

#[test]
fn dissolves_a_chain_of_nodes_at_once() {
    let mut model = model_for(
        vec![
            node("a", "sin"),
            node("b", "sin"),
            node("c", "sin"),
            node("d", "sin"),
        ],
        &[
            ("a", "result", "b", "e"),
            ("b", "result", "c", "e"),
            ("c", "result", "d", "e"),
        ],
    );
    model.dissolve_nodes(&ids(&["b", "c"]));
    let edges = &model.root().edges;
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (
            edges[0].source.as_str(),
            edges[0].target.as_str(),
            edges[0].target_handle.as_str()
        ),
        ("a", "d", "e")
    );
}

#[test]
fn reset_puts_input_values_back_to_their_defaults() {
    let mut model = chain();
    model.set_input_value("a", "e", Some(3.0.into()));
    model.reset_nodes(&ids(&["a"]));
    let values = model
        .root()
        .node("a")
        .map(|n| n.data.values.clone())
        .unwrap_or_default();
    assert_eq!(values, super::builders::values(&[("e", 0.0)]));
}

#[test]
fn routes_a_link_through_a_reroute() {
    let mut model = chain();
    let ids = model.reroute_edges(&[("a.result->b.e".to_owned(), Point::new(50.0, 60.0))]);
    assert_eq!(ids.len(), 1);
    let id = &ids[0];
    assert_eq!(
        model.root().node(id).map(|n| n.position),
        Some(Point::new(50.0, 60.0))
    );
    let edges = &model.root().edges;
    assert!(
        edges
            .iter()
            .any(|e| e.source == "a" && e.source_handle == "result" && &e.target == id)
    );
    assert!(
        edges
            .iter()
            .any(|e| &e.source == id && e.target == "b" && e.target_handle == "e")
    );
    assert!(!edges.iter().any(|e| e.id == "a.result->b.e"));
}

#[test]
fn shares_one_reroute_between_links_leaving_the_same_output() {
    let mut model = model_for(
        vec![node("a", "sin"), node("b", "sin"), node("c", "sin")],
        &[("a", "result", "b", "e"), ("a", "result", "c", "e")],
    );
    let ids = model.reroute_edges(&[
        ("a.result->b.e".to_owned(), Point::new(0.0, 0.0)),
        ("a.result->c.e".to_owned(), Point::new(10.0, 20.0)),
    ]);
    assert_eq!(ids.len(), 1);
    let id = &ids[0];
    assert_eq!(
        model.root().node(id).map(|n| n.position),
        Some(Point::new(5.0, 10.0))
    );
    let edges = &model.root().edges;
    let targets: Vec<&str> = edges
        .iter()
        .filter(|e| &e.source == id)
        .map(|e| e.target.as_str())
        .collect();
    assert_eq!(targets, ["b", "c"]);
    assert_eq!(edges.iter().filter(|e| &e.target == id).count(), 1);
}
