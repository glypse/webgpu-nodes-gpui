use std::collections::BTreeMap;

use super::*;
use crate::graph::{GraphModel, GraphProject};
use crate::ramp::{RAMP_HSV, RAMP_HUES, RAMP_INTERPOLATIONS, RAMP_OKLCH, RAMP_RGB, RAMP_SPACES};
use crate::testing::TestGraph;
use crate::types::{InputValue, WgslType};

fn vector(color: [f64; 4]) -> InputValue {
    InputValue::vector(&color)
}

fn stops(positions: &[(&str, f64)]) -> Vec<RampStop> {
    positions
        .iter()
        .map(|(id, position)| RampStop {
            id: (*id).into(),
            position: *position,
        })
        .collect()
}

fn three_stops() -> Vec<RampStop> {
    stops(&[("stop-0", 0.0), ("stop-1", 0.3), ("stop-2", 1.0)])
}

/// A graph with a three-stop ramp linked to the output.
fn ramp_graph(values: &[(&str, f64)]) -> (TestGraph, String) {
    let mut g = TestGraph::new();
    let ramp = g.add(COLOR_RAMP);
    let out = g.output();
    g.link((&ramp, "color"), (&out, "color"));
    let colors = BTreeMap::from([
        ("stop-0".to_owned(), vector([1.0, 0.0, 0.0, 1.0])),
        ("stop-1".to_owned(), vector([0.0, 1.0, 0.0, 1.0])),
        ("stop-2".to_owned(), vector([0.0, 0.0, 1.0, 1.0])),
    ]);
    g.model.set_color_ramp(&ramp, &three_stops(), &colors);
    for (input, value) in values {
        g.set(&ramp, input, *value);
    }
    (g, ramp)
}

#[test]
fn compiles_every_interpolation_in_every_space() {
    for (interpolation, _) in RAMP_INTERPOLATIONS {
        for (space, _) in RAMP_SPACES {
            for (hue, _) in RAMP_HUES {
                let (g, _) = ramp_graph(&[
                    (RAMP_INTERPOLATION, interpolation as f64),
                    (RAMP_SPACE, space as f64),
                    (RAMP_HUE, hue as f64),
                ]);
                g.wgsl();
            }
        }
    }
}

#[test]
fn switches_per_pixel_on_enums_only_known_there() {
    let (mut g, ramp) = ramp_graph(&[]);
    let declare = g.add("declare");
    g.force_type(&declare, WgslType::U32);
    g.set(&declare, "x", 1.0);
    for input in [RAMP_INTERPOLATION, RAMP_SPACE, RAMP_HUE] {
        g.link((&declare, "value"), (&ramp, input));
    }
    let wgsl = g.wgsl();
    assert!(wgsl.contains("color_ramp_any_any_any"));
    assert!(wgsl.contains("switch (interpolation)"));
}

#[test]
fn only_compiles_what_its_enums_pick() {
    let (g, _) = ramp_graph(&[]);
    let wgsl = g.wgsl();
    assert!(!wgsl.contains("color_ramp_to_oklch"));
    assert!(!wgsl.contains("switch"));
    let (g, _) = ramp_graph(&[(RAMP_SPACE, RAMP_OKLCH as f64)]);
    let wgsl = g.wgsl();
    assert!(wgsl.contains("color_ramp_to_oklch"));
    assert!(!wgsl.contains("color_ramp_to_hsv"));
}

#[test]
fn passes_positions_as_uniforms_so_moving_a_stop_doesnt_recompile() {
    let (mut g, ramp) = ramp_graph(&[]);
    let before = g.program().program.code;
    g.model.set_color_ramp(
        &ramp,
        &stops(&[("stop-0", 0.0), ("stop-1", 0.6), ("stop-2", 1.0)]),
        &BTreeMap::new(),
    );
    let after = g.program();
    assert_eq!(after.program.code, before);
    assert!(after.uniforms.iter().any(|(_, v)| v.0 == [0.6]));
}

#[test]
fn gives_rgba_once_a_stop_is_a_vec4() {
    let (mut g, ramp) = ramp_graph(&[]);
    assert!(!g.wgsl().contains("_rgba;"));
    let color = g.add("declare");
    g.force_type(&color, WgslType::Vec4f);
    g.link((&color, "value"), (&ramp, "stop-2"));
    assert_eq!(g.model.resolve_node_output_type(&ramp), WgslType::Vec4f);
    let wgsl = g.wgsl();
    assert!(wgsl.contains("_rgba;"), "{wgsl}");
}

#[test]
fn drops_the_links_and_colours_of_the_stops_it_loses_keeping_the_others() {
    let (mut g, ramp) = ramp_graph(&[]);
    let time = g.add("time");
    g.link((&time, "seconds"), (&ramp, "stop-1"));
    g.link((&time, "seconds"), (&ramp, "stop-2"));
    g.model.set_color_ramp(
        &ramp,
        &stops(&[("stop-2", 0.1), ("stop-0", 0.5)]),
        &BTreeMap::new(),
    );
    let node = g.model.node(&ramp).expect("the ramp exists");
    assert!(!node.data.values.contains_key("stop-1"));
    assert!(node.data.values.contains_key("stop-2"));
    let targets: Vec<&str> = g
        .model
        .root()
        .edges
        .iter()
        .filter(|e| e.target == ramp)
        .map(|e| e.target_handle.as_str())
        .collect();
    assert_eq!(targets, ["stop-2"]);
    let inputs = g.model.resolve_node_inputs(&ramp);
    let last: Vec<&str> = inputs[inputs.len() - 2..]
        .iter()
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(last, ["stop-2", "stop-0"]);
}

#[test]
fn hides_its_hue_interpolation_while_in_rgb() {
    let (mut g, ramp) = ramp_graph(&[]);
    assert!(g.model.hidden_inputs(&ramp).contains(RAMP_HUE));
    g.model
        .set_input_value(&ramp, RAMP_SPACE, Some((RAMP_HSV as f64).into()));
    assert!(!g.model.hidden_inputs(&ramp).contains(RAMP_HUE));
    let (g, ramp) = ramp_graph(&[(RAMP_SPACE, RAMP_RGB as f64)]);
    assert!(g.model.hidden_inputs(&ramp).contains(RAMP_HUE));
}

#[test]
fn saves_and_loads_its_stops() {
    let (g, ramp) = ramp_graph(&[]);
    let json = serde_json::to_string(g.model.project()).expect("serialises");
    let loaded: GraphProject = serde_json::from_str(&json).expect("loads");
    let model = GraphModel::new(loaded);
    let node = model.node(&ramp).expect("the ramp exists");
    assert_eq!(node.data.ramp, Some(three_stops()));
}

#[test]
fn a_new_ramp_goes_from_black_to_white() {
    let mut g = TestGraph::new();
    let ramp = g.add(COLOR_RAMP);
    let node = g.model.node(&ramp).expect("the ramp exists");
    let ramp_stops = node.data.ramp.clone().expect("has stops");
    assert_eq!(ramp_stops.len(), 2);
    assert_eq!(node.data.values["stop-0"], vector([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(node.data.values["stop-1"], vector([1.0, 1.0, 1.0, 1.0]));
    let inputs = g.model.resolve_node_inputs(&ramp);
    let ids: Vec<&str> = inputs.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            RAMP_FACTOR,
            RAMP_INTERPOLATION,
            RAMP_SPACE,
            RAMP_HUE,
            "stop-0",
            "stop-1"
        ]
    );
    let out = g.output();
    g.link((&ramp, "color"), (&out, "color"));
    g.wgsl();
}

#[test]
fn is_listed_in_the_add_menu_under_colour() {
    let model = GraphModel::default();
    let menu = crate::catalog::add_menu_entries(&model);
    let entry = menu
        .iter()
        .find(|e| e.label == "Color Ramp")
        .expect("Color Ramp is listed");
    assert_eq!(entry.heading, "Color");
    assert!(crate::nodes::is_known_node_type(COLOR_RAMP));
}
