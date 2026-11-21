use crate::graph::{GraphProject, LoadError};
use crate::nodes::{custom_sockets, is_socket_name};
use crate::testing::{TestGraph, validate_wgsl};
use crate::types::{CustomSockets, HandleDef, NodeData, WgslType};

fn sockets(inputs: &[(&str, &str, WgslType)], outputs: &[(&str, &str, WgslType)]) -> CustomSockets {
    let handles = |list: &[(&str, &str, WgslType)]| {
        list.iter().map(|(id, label, ty)| HandleDef::new(*id, *label, *ty)).collect()
    };
    CustomSockets { inputs: handles(inputs), outputs: handles(outputs) }
}

fn two_outputs() -> (String, CustomSockets) {
    (
        "sum = a + b;\ndiff = vec2f(a - b);".to_owned(),
        sockets(
            &[("a", "a", WgslType::F32), ("b", "b", WgslType::F32)],
            &[("sum", "sum", WgslType::F32), ("diff", "diff", WgslType::Vec2f)],
        ),
    )
}

fn custom(g: &mut TestGraph, code: Option<String>, sockets: Option<CustomSockets>) -> String {
    let id = g.add("custom-wgsl");
    g.model.update_node_data(&id, |d: &mut NodeData| {
        if code.is_some() {
            d.code = code;
        }
        if sockets.is_some() {
            d.sockets = sockets;
        }
    });
    id
}

#[test]
fn compiles_its_code_into_a_function_returning_its_outputs() {
    let mut g = TestGraph::new();
    let (code, s) = two_outputs();
    let c = custom(&mut g, Some(code), Some(s));
    let out = g.output();
    g.link((&c, "sum"), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(wgsl.contains("var sum: f32;") && wgsl.contains("var diff: vec2f;"), "{wgsl}");
    assert!(wgsl.contains("(a: f32, b: f32) -> custom_"), "{wgsl}");
}

#[test]
fn validates_with_the_default_code_and_sockets() {
    let mut g = TestGraph::new();
    let c = g.add("custom-wgsl");
    let out = g.output();
    g.link((&c, "color"), (&out, "color"));
    g.wgsl();
}

#[test]
fn leaves_invalid_code_to_the_shader_compiler() {
    let mut g = TestGraph::new();
    let (_, s) = two_outputs();
    let c = custom(&mut g, Some("sum = undefined_name;".to_owned()), Some(s));
    let out = g.output();
    g.link((&c, "sum"), (&out, "color"));
    let code = g.program().program.code;
    assert!(std::panic::catch_unwind(|| validate_wgsl(&code)).is_err());
}

#[test]
fn keeps_its_code_and_sockets_through_save_and_load() {
    let mut g = TestGraph::new();
    let (code, s) = two_outputs();
    let c = custom(&mut g, Some(code.clone()), Some(s.clone()));
    let loaded = GraphProject::from_json(&g.model.project().to_json()).expect("loads");
    let node = loaded.root().node(&c).expect("kept");
    assert_eq!(node.data.code.as_deref(), Some(code.as_str()));
    assert_eq!(custom_sockets(&node.data), s);
}

fn load_with_label(label: &str) -> Result<GraphProject, LoadError> {
    let mut g = TestGraph::new();
    let s = sockets(&[("a", "taken", WgslType::F32)], &[("b", label, WgslType::F32)]);
    custom(&mut g, None, Some(s));
    GraphProject::from_json(&g.model.project().to_json())
}

#[test]
fn refuses_socket_names_from_a_file_that_arent_wgsl_names_of_their_own() {
    assert!(load_with_label("free").is_ok());
    assert!(load_with_label("taken").is_err());
    assert!(load_with_label("x: f32) { evil(); } fn y(").is_err());
}

#[test]
fn keeps_socket_ids_out_of_its_shaders_syntax() {
    let hostile = "b; evil(); let c";
    let mut g = TestGraph::new();
    let s = sockets(
        &[],
        &[(hostile, "one", WgslType::F32), ("other", "two", WgslType::F32)],
    );
    let c = custom(&mut g, Some("one = 1.0;\ntwo = 2.0;".to_owned()), Some(s));
    let out = g.output();
    g.link((&c, hostile), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(!wgsl.contains("evil();"), "{wgsl}");
}

#[test]
fn takes_only_wgsl_identifiers_as_socket_names() {
    assert!(is_socket_name("color") && is_socket_name("_tmp2"));
    for name in ["2x", "a b", "let", "vec3f", "mat4x4", "__x", "_", "", "vec2", "mat2x3h"] {
        assert!(!is_socket_name(name), "{name}");
    }
    assert!(is_socket_name("vec5") && is_socket_name("matrix"));
}

#[test]
fn renames_sockets_to_free_valid_names_only() {
    let mut g = TestGraph::new();
    let (code, s) = two_outputs();
    let c = custom(&mut g, Some(code), Some(s));
    assert!(!g.model.rename_custom_socket(&c, true, "a", "b"));
    assert!(!g.model.rename_custom_socket(&c, true, "a", "fn"));
    assert!(g.model.rename_custom_socket(&c, true, "a", " x "));
    let node = g.model.node(&c).expect("node");
    assert_eq!(custom_sockets(&node.data).inputs[0].label, "x");
}

#[test]
fn drops_links_to_removed_sockets_and_to_sockets_that_no_longer_fit() {
    let mut g = TestGraph::new();
    let (code, s) = two_outputs();
    let c = custom(&mut g, Some(code), Some(s));
    let v = g.add("declare");
    g.force_type(&v, WgslType::Vec4f);
    g.model.set_custom_socket_type(&c, true, "a", WgslType::Vec4f);
    let out = g.output();
    g.link((&v, "value"), (&c, "a"));
    g.link((&c, "diff"), (&out, "color"));
    assert_eq!(g.model.root().edges.len(), 2);
    g.model.remove_custom_socket(&c, false, "diff");
    let targets: Vec<&str> = g.model.root().edges.iter().map(|e| e.target_handle.as_str()).collect();
    assert_eq!(targets, ["a"]);
    g.model.set_custom_socket_type(&c, true, "a", WgslType::Bool);
    assert!(g.model.root().edges.is_empty());
}

#[test]
fn moves_sockets_within_their_side() {
    let mut g = TestGraph::new();
    let (code, s) = two_outputs();
    let c = custom(&mut g, Some(code), Some(s));
    g.model.move_custom_socket(&c, true, "a", 1);
    let node = g.model.node(&c).expect("node");
    let labels: Vec<String> = custom_sockets(&node.data).inputs.into_iter().map(|h| h.label).collect();
    assert_eq!(labels, ["b", "a"]);
}

#[test]
fn adds_sockets_with_free_names() {
    let mut g = TestGraph::new();
    let c = g.add("custom-wgsl");
    let first = g.model.add_custom_socket(&c, true, WgslType::F32).expect("added");
    let second = g.model.add_custom_socket(&c, true, WgslType::F32).expect("added");
    assert_eq!((first.label.as_str(), second.label.as_str()), ("input", "input2"));
}
