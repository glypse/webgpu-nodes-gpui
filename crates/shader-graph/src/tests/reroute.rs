use super::builders::{model_for, node, with_data};
use crate::catalog::{AddMenuEntry, EntryKind, NAMED_REROUTES, Pending, add_entry, add_entry_connected, add_menu_entries, connectable_entries};
use crate::graph::{GraphModel, Point};
use crate::types::NodeData;

fn graph() -> GraphModel {
    let named = NodeData { label: Some("Scale".to_owned()), hide_links: Some(true), ..Default::default() };
    model_for(
        vec![node("t", "time"), with_data(node("scale", "reroute"), named), node("plain", "reroute"), node("s", "sin")],
        &[("t", "seconds", "scale", "in")],
    )
}

fn entry_of(model: &GraphModel) -> AddMenuEntry {
    add_menu_entries(model)
        .into_iter()
        .find(|e| matches!(&e.kind, EntryKind::Link { source } if source == "scale"))
        .expect("an entry for the named reroute")
}

#[test]
fn named_reroutes_are_listed_in_the_add_menu_by_name_other_reroutes_arent() {
    let links: Vec<AddMenuEntry> =
        add_menu_entries(&graph()).into_iter().filter(|e| matches!(e.kind, EntryKind::Link { .. })).collect();
    let labels: Vec<&str> = links.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["Scale"]);
    assert_eq!(links[0].section, [NAMED_REROUTES]);
}

#[test]
fn named_reroutes_are_linked_to_an_input_a_link_is_dragged_from_adding_nothing() {
    let mut model = graph();
    let entry = entry_of(&model);
    let pending = Pending { node: "s".to_owned(), handle: "e".to_owned(), from_output: false };
    assert_eq!(connectable_entries(&model, std::slice::from_ref(&entry), &pending).len(), 1);
    let count = model.root().nodes.len();
    add_entry_connected(&mut model, &entry, Point::default(), &pending);
    assert_eq!(model.root().nodes.len(), count);
    assert!(model.root().edges_into("s").any(|e| e.source == "scale"));
}

#[test]
fn named_reroutes_feed_a_new_reroute_when_added_on_their_own() {
    let mut model = graph();
    let entry = entry_of(&model);
    let id = add_entry(&mut model, &entry, Point::default()).expect("added");
    assert!(model.root().node(&id).is_some_and(|n| n.is("reroute")));
    assert!(model.root().edges_into(&id).any(|e| e.source == "scale"));
}
