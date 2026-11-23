use super::builders::{model_for, node};
use crate::graph::{GraphEdge, GraphModel, Point};
use crate::zones::ZoneKind;

fn links_into_output(model: &GraphModel) -> Vec<GraphEdge> {
    model
        .root()
        .edges
        .iter()
        .filter(|e| e.target == "out" && e.target_handle == "color")
        .cloned()
        .collect()
}

/// The group node made of `a` and `b`, with `c` outside it.
fn grouped() -> (GraphModel, String, String) {
    let mut model = model_for(
        vec![node("a", "sin"), node("b", "sin"), node("c", "sin")],
        &[("a", "result", "b", "e"), ("b", "result", "c", "e")],
    );
    model
        .group_selection(&["a".to_owned(), "b".to_owned()])
        .expect("grouped");
    let group = model
        .root()
        .nodes
        .iter()
        .find(|n| n.is("node-group"))
        .expect("a group node");
    let reference = group.definition_ref().expect("a definition").to_owned();
    (model.clone(), group.id.clone(), reference)
}

fn viewers_of(model: &GraphModel, reference: &str) -> Vec<String> {
    model
        .project()
        .document(reference)
        .map(|d| {
            d.outputs
                .iter()
                .filter(|h| h.viewer == Some(true))
                .map(|h| h.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn links_a_node_straight_to_the_output_replacing_its_link() {
    let mut model = model_for(vec![node("a", "sin"), node("b", "sin")], &[]);
    assert!(model.preview_node("a"));
    assert!(model.preview_node("b"));
    let links = links_into_output(&model);
    assert_eq!(links.len(), 1);
    assert_eq!(
        (links[0].source.as_str(), links[0].source_handle.as_str()),
        ("b", "result")
    );
}

#[test]
fn goes_to_the_next_output_when_the_node_is_already_previewed() {
    let mut model = model_for(vec![], &[]);
    let zone = model
        .create_zone(ZoneKind::Repeat, Point::default())
        .expect("created");
    let outputs: Vec<String> = model
        .resolve_node_outputs(&zone)
        .into_iter()
        .map(|o| o.id)
        .collect();
    assert!(outputs.len() > 1);
    let previewed = |m: &GraphModel| {
        links_into_output(m)
            .first()
            .map(|e| e.source_handle.clone())
    };
    model.preview_node(&zone);
    assert_eq!(previewed(&model), Some(outputs[0].clone()));
    model.preview_node(&zone);
    assert_eq!(previewed(&model), Some(outputs[1].clone()));
}

#[test]
fn carries_a_preview_out_of_a_group_through_a_viewer_output() {
    let (mut model, group, reference) = grouped();
    model.enter_group(&group);
    assert!(model.preview_node("a"));
    let viewers = viewers_of(&model, &reference);
    assert_eq!(viewers.len(), 1);
    let viewer = &viewers[0];
    let inside = model.project().document(&reference).expect("definition");
    assert!(
        inside
            .edges
            .iter()
            .any(|e| e.source == "a" && &e.target_handle == viewer)
    );
    let links = links_into_output(&model);
    assert_eq!(links.len(), 1);
    assert_eq!(
        (links[0].source.as_str(), &links[0].source_handle),
        (group.as_str(), viewer)
    );

    // Another node of the group reuses the viewer output.
    model.preview_node("b");
    assert_eq!(viewers_of(&model, &reference), viewers);
}

#[test]
fn removes_viewer_outputs_a_preview_no_longer_goes_through() {
    let (mut model, group, reference) = grouped();
    model.enter_group(&group);
    model.preview_node("a");
    model.exit_group();
    model.preview_node("c");
    assert!(viewers_of(&model, &reference).is_empty());
    assert!(
        !model
            .root()
            .edges
            .iter()
            .any(|e| e.source == group && e.target == "out")
    );
    let links = links_into_output(&model);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].source, "c");
}

#[test]
fn leaves_nodes_without_outputs_alone() {
    assert!(!model_for(vec![], &[]).preview_node("out"));
}
