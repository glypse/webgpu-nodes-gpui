use super::builders::{model_for, node};
use crate::graph::{Connection, GraphModel, GraphNode, Point};
use crate::layout::{InputRow, LayoutOptions, LayoutSpacing, NodeGeometry, NodeRows, estimate_node_geometry};
use crate::zones::ZoneKind;

fn chain() -> GraphModel {
    model_for(
        vec![node("t", "time"), node("a", "sin"), node("b", "sin")],
        &[
            ("t", "seconds", "a", "e"),
            ("a", "result", "b", "e"),
            ("b", "result", "out", "color"),
        ],
    )
}

fn x(model: &GraphModel, id: &str) -> f32 {
    model.root().node(id).map_or(f32::NAN, |n| n.position.x)
}

fn key(model: &GraphModel) -> String {
    model.active_definition_ref()
}

#[test]
fn lays_a_chain_out_left_to_right_keeping_its_top_left_corner() {
    let mut model = chain();
    let k = key(&model);
    assert!(
        model
            .layout_document(&k, None, &LayoutOptions::default())
            .expect("lays out")
    );
    assert!(x(&model, "t") < x(&model, "a"));
    assert!(x(&model, "a") < x(&model, "b"));
    assert!(x(&model, "b") < x(&model, "out"));
    let nodes = &model.root().nodes;
    assert_eq!(nodes.iter().map(|n| n.position.x).fold(f32::INFINITY, f32::min), 0.0);
    assert_eq!(nodes.iter().map(|n| n.position.y).fold(f32::INFINITY, f32::min), 0.0);
}

#[test]
fn uses_the_front_ends_node_sizes() {
    let mut model = model_for(vec![node("a", "sin"), node("b", "sin")], &[("a", "result", "b", "e")]);
    let k = key(&model);
    let measure = |_: &str, n: &GraphNode| {
        let width = if n.id == "a" { 500.0 } else { 100.0 };
        Some(NodeGeometry {
            width,
            height: 50.0,
            inputs: vec![("e".to_owned(), 25.0)],
            outputs: vec![("result".to_owned(), 25.0)],
        })
    };
    let options = LayoutOptions {
        measure: Some(&measure),
        spacing: LayoutSpacing {
            between_layers: 10.0,
            ..Default::default()
        },
    };
    let ids = ["a".to_owned(), "b".to_owned()];
    model.layout_document(&k, Some(&ids), &options).expect("lays out");
    assert_eq!(x(&model, "b") - x(&model, "a"), 510.0);
}

#[test]
fn moves_only_the_given_nodes() {
    let mut model = model_for(vec![node("a", "sin"), node("b", "sin")], &[("a", "result", "b", "e")]);
    let k = key(&model);
    let before: Option<Point> = model.root().node("out").map(|n| n.position);
    let ids = ["a".to_owned(), "b".to_owned()];
    model
        .layout_document(&k, Some(&ids), &LayoutOptions::default())
        .expect("lays out");
    assert_eq!(model.root().node("out").map(|n| n.position), before);
}

#[test]
fn keeps_a_zones_nodes_between_its_boundaries() {
    let mut model = model_for(vec![node("t", "time"), node("body", "sin")], &[]);
    let zone = model
        .create_zone(ZoneKind::Repeat, Point::default())
        .expect("zone created");
    let output = model.zone_nodes(&zone).expect("paired").output.id;
    let item = model.zone_items_of(&zone)[0].id.clone();
    assert!(model.connect(Connection::new(zone.as_str(), item.as_str(), "body", "e")));
    assert!(model.connect(Connection::new("body", "result", output.as_str(), item.as_str())));
    assert!(model.connect(Connection::new("t", "seconds", zone.as_str(), item.as_str())));
    assert_eq!(model.active_zones().node_zone.get("body"), Some(&Some(zone.clone())));

    let k = key(&model);
    model
        .layout_document(&k, None, &LayoutOptions::default())
        .expect("lays out");
    assert!(x(&model, "t") < x(&model, &zone));
    assert!(x(&model, &zone) < x(&model, "body"));
    assert!(x(&model, "body") < x(&model, &output));
}

#[test]
fn splits_each_output_linked_to_several_inputs_at_a_reroute_once() {
    let mut model = model_for(
        vec![node("t", "time"), node("a", "sin"), node("b", "cos"), node("c", "add")],
        &[
            ("t", "seconds", "a", "e"),
            ("t", "seconds", "b", "e"),
            ("a", "result", "c", "operands"),
            ("b", "result", "c", "operands"),
            ("c", "result", "out", "color"),
        ],
    );
    let k = key(&model);
    model
        .layout_document(&k, None, &LayoutOptions::default())
        .expect("lays out");
    let root = model.root();
    let from_time: Vec<&_> = root.edges_from("t").collect();
    assert_eq!(from_time.len(), 1);
    let reroute = from_time[0].target.clone();
    assert_eq!(root.node(&reroute).map(|n| n.node_type.as_str()), Some("reroute"));
    let mut targets: Vec<String> = root.edges_from(&reroute).map(|e| e.target.clone()).collect();
    targets.sort();
    assert_eq!(targets, ["a", "b"]);

    let count = model.root().nodes.len();
    model
        .layout_document(&k, None, &LayoutOptions::default())
        .expect("lays out");
    assert_eq!(model.root().nodes.len(), count);
}

#[test]
fn estimates_a_node_from_the_rows_it_shows() {
    let rows = NodeRows {
        header: 1,
        outputs: vec!["o".to_owned()],
        widget: 2,
        inputs: vec![
            InputRow {
                id: "i".to_owned(),
                editor: true,
            },
            InputRow {
                id: "j".to_owned(),
                editor: false,
            },
        ],
    };
    let geometry = estimate_node_geometry(&rows);
    assert_eq!(geometry.width, 220.0);
    assert_eq!(geometry.height, 40.0 + 30.0 * 7.0);
    assert_eq!(geometry.outputs, [("o".to_owned(), 40.0 + 30.0 * 1.5)]);
    assert_eq!(
        geometry.inputs,
        [("i".to_owned(), 40.0 + 30.0 * 4.5), ("j".to_owned(), 40.0 + 30.0 * 6.5)]
    );
}
