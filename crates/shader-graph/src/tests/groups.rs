use crate::graph::{GraphModel, Point};
use crate::library;

fn library_entry(id: &str) -> &'static library::LibraryEntry {
    library::entries().iter().find(|e| e.id == id).expect("the library has the entry")
}

#[test]
fn cloning_a_locked_color_group_gives_an_editable_copy_without_its_wheel() {
    let entry = library_entry("std-color");
    let mut model = GraphModel::default();
    let node = model.add_group_instance(&entry.reference, Point::default()).expect("adds");
    let widget = |model: &GraphModel| model.resolve_descriptor(model.node(&node).expect("node")).widget.is_some();
    assert!(widget(&model), "the library colour group draws its wheel");

    let copy = model.deep_clone_group(&node).expect("clones");
    assert!(!widget(&model), "a clone is an ordinary group");
    let definition = &model.project().definitions[&copy];
    assert!(!definition.locked);
    assert_eq!(definition.label.as_deref(), Some(format!("{} - Copy", entry.label).as_str()));
    assert_eq!(model.node(&node).and_then(|n| n.definition_ref()), Some(copy.as_str()));
    assert!(model.node(&node).is_some_and(|n| n.group.as_ref().is_some_and(|g| !g.locked)));

    let source_ids: Vec<&str> = entry.definition.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(definition.nodes.iter().all(|n| !source_ids.contains(&n.id.as_str())), "node ids are fresh");
    assert_eq!(model.project().definitions[&entry.reference].label.as_deref(), Some(entry.label.as_str()));
}

#[test]
fn clones_keep_type_variables_and_number_their_copies() {
    let entry = library_entry("std-hash");
    let mut model = GraphModel::default();
    let node = model.add_group_instance(&entry.reference, Point::default()).expect("adds");
    let first = model.deep_clone_group(&node).expect("clones");
    let names = |key: &str| -> Vec<String> {
        model.project().definitions[key].type_variables.iter().map(|v| v.name.clone()).collect()
    };
    assert_eq!(names(&first), names(&entry.reference));
    assert!(!names(&first).is_empty(), "the hash group shares a type");

    let second_node = model.add_group_instance(&entry.reference, Point::new(100.0, 0.0)).expect("adds");
    let second = model.deep_clone_group(&second_node).expect("clones");
    let label = |key: &str| model.project().definitions[key].label.clone();
    assert_eq!(label(&first), Some(format!("{} - Copy", entry.label)));
    assert_eq!(label(&second), Some(format!("{} - Copy 2", entry.label)));
}

#[test]
fn a_group_nested_in_another_group_is_cloned_in_its_own_document() {
    let mut model = GraphModel::default();
    let outer = model.create_group(Point::default()).expect("creates");
    let outer_ref = model.node(&outer).and_then(|n| n.definition_ref()).expect("a group").to_owned();
    model.enter_group(&outer);
    let inner = model.create_group(Point::default()).expect("creates");
    model.exit_group();

    let copy = model.deep_clone_group(&inner).expect("clones a group inside another group");
    let holder = &model.project().definitions[&outer_ref];
    assert_eq!(holder.node(&inner).and_then(|n| n.definition_ref()), Some(copy.as_str()));
}
