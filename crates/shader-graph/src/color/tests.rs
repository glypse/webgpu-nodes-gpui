use crate::graph::{Connection, FRAG_OUTPUT, GraphModel, GroupWidget, Point};
use crate::library::{entries, entry_for_ref};
use crate::testing::validate_wgsl;
use crate::types::{InputControl, InputValue, NodeWidget};

use super::{ColorMode, color_to_linear};

const COLOR: &str = "std-color";

fn entry(id: &str) -> &'static crate::library::LibraryEntry {
    entries()
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("{id} is in the library"))
}

fn input_id(id: &str, label: &str, nth: usize) -> String {
    entry(id)
        .definition
        .inputs
        .iter()
        .filter(|h| h.label == label)
        .nth(nth)
        .map(|h| h.id.clone())
        .unwrap_or_else(|| panic!("{id} has input {label}"))
}

fn output_id(id: &str, label: &str) -> String {
    entry(id)
        .definition
        .outputs
        .iter()
        .find(|h| h.label == label)
        .map(|h| h.id.clone())
        .unwrap_or_else(|| panic!("{id} has output {label}"))
}

fn components_of(mode: ColorMode) -> [String; 3] {
    let Some(GroupWidget::Color { modes, .. }) = &entry(COLOR).definition.widget else {
        panic!("Color has a widget")
    };
    let value = ColorMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    modes[&value.to_string()].components.clone()
}

fn mode_input() -> String {
    input_id(COLOR, "Mode", 0)
}

fn color_node(model: &mut GraphModel) -> String {
    model
        .add_group_instance(&entry(COLOR).reference, Point::default())
        .expect("adds the Color group")
}

fn set_mode(model: &mut GraphModel, node: &str, mode: ColorMode) {
    let value = ColorMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    model.set_input_value(node, &mode_input(), Some(InputValue::from(value as i32)));
}

fn set_components(model: &mut GraphModel, node: &str, mode: ColorMode, values: [f64; 3]) {
    set_mode(model, node, mode);
    for (id, value) in components_of(mode).iter().zip(values) {
        model.set_input_value(node, id, Some(value.into()));
    }
}

fn values_of(model: &GraphModel, node: &str, mode: ColorMode) -> [f64; 3] {
    let data = &model.node(node).expect("the node exists").data;
    let [x, y, z] = components_of(mode);
    let read = |id: &String| data.values.get(id).map_or(f64::NAN, |v| v.component(0));
    [read(&x), read(&y), read(&z)]
}

fn shown_labels(model: &GraphModel, node: &str) -> Vec<String> {
    let hidden = model.hidden_inputs(node);
    model
        .resolve_node_inputs(node)
        .into_iter()
        .filter(|i| !hidden.contains(&i.id))
        .map(|i| i.label)
        .collect()
}

#[test]
fn shows_the_inputs_of_its_mode_only() {
    let mut model = GraphModel::default();
    let node = color_node(&mut model);
    assert_eq!(
        shown_labels(&model, &node),
        ["Mode", "Hue", "Saturation", "Value", "Alpha"]
    );
    set_mode(&mut model, &node, ColorMode::Oklch);
    assert_eq!(
        shown_labels(&model, &node),
        ["Mode", "Lightness", "Chroma", "Hue", "Alpha"]
    );
    set_mode(&mut model, &node, ColorMode::Rgb);
    assert_eq!(
        shown_labels(&model, &node),
        ["Mode", "Red", "Green", "Blue", "Alpha"]
    );
}

#[test]
fn keeps_its_components_in_range_while_edited() {
    let mut model = GraphModel::default();
    let node = color_node(&mut model);
    let inputs = model.resolve_node_inputs(&node);
    let range = |id: &String| match inputs
        .iter()
        .find(|i| i.id == *id)
        .and_then(|i| i.control.clone())
    {
        Some(InputControl::Number { min, max, .. }) => (min, max),
        None => (None, None),
        other => panic!("{other:?}"),
    };
    let oklch = components_of(ColorMode::Oklch);
    assert_eq!(
        oklch.each_ref().map(range),
        [
            (Some(0.0), Some(1.0)),
            (Some(0.0), Some(1.0)),
            (Some(0.0), Some(360.0))
        ]
    );
    let rgb = components_of(ColorMode::Rgb);
    assert_eq!(rgb.each_ref().map(range), [(None, None); 3]);
}

#[test]
fn draws_the_colour_wheel() {
    let mut model = GraphModel::default();
    let node = color_node(&mut model);
    let descriptor = model.resolve_descriptor(model.node(&node).expect("the node exists"));
    let Some(NodeWidget::Color(GroupWidget::Color { .. })) = &descriptor.widget else {
        panic!("the Color group has a colour widget")
    };
}

#[test]
fn converts_the_components_when_the_mode_changes_keeping_the_colour() {
    let mut model = GraphModel::default();
    let node = color_node(&mut model);
    set_components(&mut model, &node, ColorMode::Hsv, [0.6, 0.5, 0.7]);
    let original = color_to_linear(ColorMode::Hsv, [0.6, 0.5, 0.7]);
    for mode in [ColorMode::Rgb, ColorMode::Oklch, ColorMode::Hsv] {
        set_mode(&mut model, &node, mode);
        let linear = color_to_linear(mode, values_of(&model, &node, mode));
        assert!((linear.r - original.r).abs() < 1e-5, "{mode:?}");
        assert!((linear.g - original.g).abs() < 1e-5, "{mode:?}");
        assert!((linear.b - original.b).abs() < 1e-5, "{mode:?}");
    }
    assert!((values_of(&model, &node, ColorMode::Hsv)[0] - 0.6).abs() < 1e-5);
}

#[test]
fn keeps_the_hue_of_a_grey_through_a_mode_change() {
    let mut model = GraphModel::default();
    let node = color_node(&mut model);
    set_components(&mut model, &node, ColorMode::Hsv, [0.25, 0.0, 0.5]);
    set_mode(&mut model, &node, ColorMode::Oklch);
    assert!((values_of(&model, &node, ColorMode::Oklch)[2] - 90.0).abs() < 1e-5);
}

#[test]
fn keeps_the_numbers_while_a_component_is_linked() {
    let mut model = GraphModel::default();
    let time = model.add_node("time", Point::default()).expect("adds");
    let node = color_node(&mut model);
    set_components(&mut model, &node, ColorMode::Hsv, [0.6, 0.5, 0.7]);
    let [hue, ..] = components_of(ColorMode::Hsv);
    assert!(model.connect(Connection::new(&time, "seconds", &node, &hue)));
    set_mode(&mut model, &node, ColorMode::Rgb);
    let [red, ..] = components_of(ColorMode::Rgb);
    let stored = model
        .node(&node)
        .expect("the node exists")
        .data
        .values
        .get(&red)
        .map(|v| v.component(0));
    // Still the group's default, not a conversion.
    assert_eq!(stored, None);
}

fn shader_of(model: &mut GraphModel, node: &str, output: &str) -> String {
    let out = model
        .root()
        .nodes
        .iter()
        .find(|n| n.is(FRAG_OUTPUT))
        .expect("an output")
        .id
        .clone();
    assert!(model.connect(Connection::new(node, output, &out, "color")));
    let code = model.generate().expect("generates").program.code;
    validate_wgsl(&code);
    code
}

#[test]
fn only_compiles_the_conversion_of_its_mode() {
    let output = output_id(COLOR, "RGBA");
    let gamut_map = entry("std-oklch-gamut-map")
        .reference
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    let shader = |mode| {
        let mut model = GraphModel::default();
        let node = color_node(&mut model);
        set_components(&mut model, &node, mode, [0.1, 0.2, 0.3]);
        shader_of(&mut model, &node, &output)
    };
    assert!(!shader(ColorMode::Rgb).contains("pow("));
    assert!(!shader(ColorMode::Hsv).contains(&gamut_map));
    assert!(shader(ColorMode::Oklch).contains(&gamut_map));
}

#[test]
fn compiles_each_converter() {
    for id in [
        "std-linear-rgb-to-xyz",
        "std-xyz-to-linear-rgb",
        "std-xyz-to-oklab",
        "std-oklab-to-xyz",
        "std-linear-rgb-to-oklab",
        "std-oklab-to-linear-rgb",
        "std-oklab-to-oklch",
        "std-oklch-to-oklab",
        "std-linear-rgb-to-oklch",
        "std-oklch-to-linear-rgb",
        "std-oklch-gamut-map",
        "std-in-display-gamut",
    ] {
        let Some(out) = entry(id).definition.outputs.first().map(|h| h.id.clone()) else {
            panic!("{id} has an output")
        };
        let mut model = GraphModel::default();
        let node = model.add_group_instance(
            &entry_for_ref(&entry(id).reference)
                .expect("entry")
                .reference,
            Point::default(),
        );
        shader_of(&mut model, &node.expect("adds"), &out);
    }
}
