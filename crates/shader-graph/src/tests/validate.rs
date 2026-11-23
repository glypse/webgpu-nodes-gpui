//! Loading a project: its schema limits, its graph rules and the defaults it's filled in with.

use serde_json::{Value, json};

use crate::graph::{
    GraphIssue, GraphProject, LoadError, MAX_CASES, MAX_CHAIN_LENGTH, MAX_CODE_LENGTH,
    MAX_DEFINITIONS, MAX_GROUP_DEPTH, MAX_ID_LENGTH, MAX_LIST_LENGTH, MAX_NODES_PER_DOCUMENT,
    MAX_NOTE_LENGTH, ROOT_ID, document_issues,
};
use crate::library;
use crate::nodes::default_data_for_type;
use crate::types::{InputValue, NodeData};

fn node(id: &str, node_type: &str) -> Value {
    json!({ "id": id, "type": node_type, "data": {}, "position": { "x": 0, "y": 0 } })
}

fn link(id: &str, source: &str, target: &str) -> Value {
    json!({ "id": id, "source": source, "target": target, "sourceHandle": "result", "targetHandle": "e" })
}

/// A project of one root document holding `nodes` and `edges`.
fn root(nodes: Vec<Value>, edges: Vec<Value>) -> String {
    json!({ "definitions": { ROOT_ID: { "nodes": nodes, "edges": edges } } }).to_string()
}

fn group(id: &str, definition: &str) -> Value {
    json!({ "id": id, "type": "node-group", "data": {}, "position": { "x": 0, "y": 0 }, "group": { "definitionRef": definition } })
}

fn issues_of(text: &str) -> Vec<GraphIssue> {
    match GraphProject::from_json(text) {
        Err(LoadError::InvalidGraph(issues)) => issues,
        other => panic!("expected graph issues, got {other:?}"),
    }
}

fn schema_error(text: &str) -> String {
    match GraphProject::from_json(text) {
        Err(LoadError::InvalidSchema(message)) => message,
        other => panic!("expected a schema error, got {other:?}"),
    }
}

#[test]
fn accepts_what_the_app_ships() {
    assert!(GraphProject::from_json(&GraphProject::default().to_json()).is_ok());
    for entry in library::entries() {
        assert_eq!(
            document_issues(&entry.reference, &entry.definition),
            Vec::new()
        );
    }
}

#[test]
fn rejects_what_isnt_json_or_a_project() {
    assert_eq!(
        GraphProject::from_json("{").unwrap_err(),
        LoadError::InvalidJson
    );
    assert!(matches!(
        GraphProject::from_json(r#"{"definitions": 1}"#),
        Err(LoadError::InvalidSchema(_))
    ));
}

#[test]
fn rejects_a_project_without_a_root() {
    assert_eq!(issues_of(r#"{"definitions":{}}"#), vec![GraphIssue::NoRoot]);
}

#[test]
fn rejects_link_cycles_a_self_link_and_a_cycle_through_a_zone() {
    let cycle = root(
        vec![node("a", "sin"), node("b", "sin")],
        vec![link("l1", "a", "b"), link("l2", "b", "a")],
    );
    assert_eq!(
        issues_of(&cycle),
        vec![GraphIssue::EdgeCycle {
            definition: ROOT_ID.into()
        }]
    );

    let self_link = root(vec![node("a", "sin")], vec![link("l", "a", "a")]);
    assert_eq!(
        issues_of(&self_link),
        vec![GraphIssue::EdgeCycle {
            definition: ROOT_ID.into()
        }]
    );

    let mut input = node("in", "repeat-input");
    input["data"] = json!({ "pairedOutput": "out-zone" });
    let zone = root(
        vec![input, node("out-zone", "repeat-output")],
        vec![link("l", "out-zone", "in")],
    );
    assert_eq!(
        issues_of(&zone),
        vec![GraphIssue::EdgeCycle {
            definition: ROOT_ID.into()
        }]
    );
}

#[test]
fn rejects_ids_used_twice_and_links_to_nothing() {
    let doubled = root(vec![node("a", "sin"), node("a", "sin")], vec![]);
    assert!(
        matches!(issues_of(&doubled).as_slice(), [GraphIssue::DuplicateId { id, .. }] if *id == "a")
    );

    let dangling = root(vec![node("a", "sin")], vec![link("l", "a", "gone")]);
    assert!(
        matches!(issues_of(&dangling).as_slice(), [GraphIssue::DanglingEdge { edge, .. }] if *edge == "l")
    );
}

#[test]
fn keeps_links_to_sockets_it_doesnt_know_which_hidden_inputs_have() {
    let mut edge = link("l", "a", "b");
    edge["targetHandle"] = json!("gone");
    let hidden = root(vec![node("a", "sin"), node("b", "sin")], vec![edge]);
    assert!(GraphProject::from_json(&hidden).is_ok());
}

#[test]
fn rejects_groups_nesting_themselves_or_nesting_the_root() {
    let project = json!({ "definitions": {
        ROOT_ID: { "nodes": [group("g", "a")], "edges": [] },
        "a": { "nodes": [group("n", "b")], "edges": [] },
        "b": { "nodes": [group("n2", "a")], "edges": [] },
    }});
    assert!(issues_of(&project.to_string()).contains(&GraphIssue::GroupCycle));

    let nests_root = json!({ "definitions": {
        ROOT_ID: { "nodes": [], "edges": [] },
        "a": { "nodes": [group("n", ROOT_ID)], "edges": [] },
    }});
    assert!(
        issues_of(&nests_root.to_string()).contains(&GraphIssue::RootInstance {
            definition: "a".into()
        })
    );
}

#[test]
fn rejects_groups_nested_too_deep_or_too_many_definitions() {
    let mut definitions = serde_json::Map::new();
    definitions.insert(ROOT_ID.into(), json!({ "nodes": [], "edges": [] }));
    for i in 0..=MAX_GROUP_DEPTH + 1 {
        let nodes = if i <= MAX_GROUP_DEPTH {
            vec![group("n", &format!("d{}", i + 1))]
        } else {
            vec![]
        };
        definitions.insert(format!("d{i}"), json!({ "nodes": nodes, "edges": [] }));
    }
    let deep = json!({ "definitions": definitions });
    assert!(issues_of(&deep.to_string()).contains(&GraphIssue::GroupsTooDeep));

    let mut many = serde_json::Map::new();
    many.insert(ROOT_ID.into(), json!({ "nodes": [], "edges": [] }));
    for i in 0..MAX_DEFINITIONS {
        many.insert(format!("d{i}"), json!({ "nodes": [], "edges": [] }));
    }
    assert!(schema_error(&json!({ "definitions": many }).to_string()).contains("definitions"));
}

#[test]
fn rejects_definitions_posing_as_the_standard_librarys() {
    let entry = &library::entries()[0];
    let spoof = json!({ "definitions": {
        ROOT_ID: { "nodes": [], "edges": [] },
        format!("{}@deadbeef", entry.id): { "nodes": [], "edges": [] },
    }});
    assert!(
        issues_of(&spoof.to_string()).contains(&GraphIssue::ReservedDefinition {
            definition: format!("{}@deadbeef", entry.id)
        })
    );
}

#[test]
fn rejects_counts_that_would_hang_codegen() {
    let switches = |cases: Value| {
        let mut switch = node("s", "index-switch");
        switch["data"] = json!({ "cases": cases });
        root(vec![switch], vec![])
    };
    assert!(schema_error(&switches(json!(1_000_000_000))).contains("cases"));
    assert!(schema_error(&switches(json!(0))).contains("cases"));
    schema_error(&switches(json!(0.5)));
    assert!(GraphProject::from_json(&switches(json!(MAX_CASES))).is_ok());
}

#[test]
fn rejects_numbers_json_cant_write_back() {
    let project = root(vec![node("a", "sin")], vec![]).replace(r#""x":0"#, r#""x":1e999"#);
    assert!(GraphProject::from_json(&project).is_err());
}

#[test]
fn rejects_a_camera_without_a_zoom() {
    let mut project: Value = serde_json::from_str(&root(vec![], vec![])).unwrap();
    project["definitions"][ROOT_ID]["viewport"] = json!({ "x": 0, "y": 0, "zoom": 0 });
    assert!(schema_error(&project.to_string()).contains("zoom"));
}

#[test]
fn rejects_ids_labels_notes_and_code_past_their_limits() {
    let long_id = "i".repeat(MAX_ID_LENGTH + 1);
    assert!(schema_error(&root(vec![node(&long_id, "sin")], vec![])).contains("too long"));

    let mut note = node("a", "sin");
    note["data"] = json!({ "note": "n".repeat(MAX_NOTE_LENGTH + 1) });
    assert!(schema_error(&root(vec![note], vec![])).contains("note: too long"));

    let mut code = node("a", "custom-wgsl");
    code["data"] = json!({ "code": "c".repeat(MAX_CODE_LENGTH + 1) });
    assert!(schema_error(&root(vec![code], vec![])).contains("code: too long"));
}

#[test]
fn rejects_more_nodes_or_items_than_a_document_or_node_may_hold() {
    let nodes: Vec<Value> = (0..=MAX_NODES_PER_DOCUMENT)
        .map(|i| node(&format!("n{i}"), "sin"))
        .collect();
    assert!(schema_error(&root(nodes, vec![])).contains("nodes"));

    let mut zone = node("out", "simulation-output");
    let items: Vec<Value> = (0..=MAX_LIST_LENGTH)
        .map(|i| json!({ "id": format!("i{i}"), "label": "x", "type": "f32" }))
        .collect();
    zone["data"] = json!({ "items": items });
    assert!(schema_error(&root(vec![zone], vec![])).contains("items"));
}

#[test]
fn rejects_ramp_stops_sharing_an_id() {
    let mut ramp = node("r", "color-ramp");
    let stop = json!({ "id": "a", "position": 0.5 });
    ramp["data"] = json!({ "ramp": [stop.clone(), stop] });
    assert!(schema_error(&root(vec![ramp], vec![])).contains("ramp stop"));
}

#[test]
fn rejects_chains_too_long_to_follow() {
    let count = MAX_CHAIN_LENGTH + 2;
    let nodes: Vec<Value> = (0..count).map(|i| node(&format!("n{i}"), "sin")).collect();
    let edges: Vec<Value> = (1..count)
        .map(|i| link(&format!("l{i}"), &format!("n{}", i - 1), &format!("n{i}")))
        .collect();
    assert!(
        issues_of(&root(nodes, edges)).contains(&GraphIssue::ChainTooLong {
            definition: ROOT_ID.into()
        })
    );
}

#[test]
fn fills_in_the_defaults_of_nodes_saved_without_them() {
    let loaded = GraphProject::from_json(&root(vec![node("c", "curve")], vec![])).expect("loads");
    let defaults = default_data_for_type("curve");
    assert_ne!(
        defaults,
        NodeData::default(),
        "the curve node has defaults to fill in"
    );
    assert_eq!(loaded.resolve_node("c").map(|n| &n.data), Some(&defaults));
}

#[test]
fn keeps_the_values_a_node_saved_over_the_defaults() {
    let mut saved = node("s", "sin");
    saved["data"] = json!({ "values": { "e": 2.5 } });
    let loaded = GraphProject::from_json(&root(vec![saved], vec![])).expect("loads");
    let data = &loaded.resolve_node("s").expect("kept").data;
    assert_eq!(data.values.get("e"), Some(&InputValue::from(2.5)));
}

#[test]
fn one_node_id_in_two_definitions_is_refused() {
    let project = json!({ "definitions": {
        ROOT_ID: { "nodes": [node("a", "sin")], "edges": [] },
        "other": { "nodes": [node("a", "sin")], "edges": [] },
    } });
    assert!(matches!(
        issues_of(&project.to_string()).as_slice(),
        [GraphIssue::NodeIdInTwoDefinitions { id, .. }] if id == "a"
    ));
}

#[test]
fn rejects_an_asset_that_isnt_what_an_import_makes() {
    let asset = json!({
        "kind": "image", "id": "a1", "name": "Photo", "file": "project.json",
        "mime": "image/png", "bytes": 1, "hash": "0".repeat(64), "width": 1, "height": 1,
    });
    let project = json!({ "definitions": { ROOT_ID: { "nodes": [], "edges": [] } }, "assets": [asset] });
    assert!(schema_error(&project.to_string()).contains("isn't a valid asset"));
}
