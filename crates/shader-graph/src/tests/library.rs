use std::collections::HashSet;

use crate::catalog::{EntryKind, add_menu_entries};
use crate::graph::{GraphIssue, GraphModel, GraphProject, LoadError, Point, ROOT_ID};
use crate::library::{self, entries, is_library_ref};
use crate::testing::TestGraph;
use crate::types::{HandleType, WgslType};

#[test]
fn the_library_loads() {
    assert!(
        library::load_error().is_none(),
        "{:?}",
        library::load_error()
    );
    assert!(entries().len() >= 40);
    let ids: HashSet<&str> = entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), entries().len(), "library ids are unique");
    let (first, second) = (
        library::load().expect("loads").entries,
        library::load().expect("loads").entries,
    );
    let refs = |loaded: &[library::LibraryEntry]| {
        loaded
            .iter()
            .map(|e| e.reference.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(refs(&first), refs(&second), "library versions are stable");
}

#[test]
fn nested_groups_are_in_the_library() {
    for entry in entries() {
        for reference in entry
            .definition
            .group_refs()
            .chain(entry.dependencies.iter().map(String::as_str))
        {
            assert!(
                library::entry_for_ref(reference).is_some(),
                "{} nests {reference}, which isn't in the library",
                entry.id
            );
        }
    }
}

#[test]
fn entries_are_listed_in_the_add_menu_under_their_headings() {
    let model = GraphModel::default();
    let menu = add_menu_entries(&model);
    for entry in entries() {
        let listed = menu
            .iter()
            .find(|m| m.key == format!("group:{}", entry.id))
            .unwrap_or_else(|| panic!("{} isn't in the menu", entry.id));
        assert_eq!(
            listed.kind,
            EntryKind::Group {
                definition_ref: entry.reference.clone()
            }
        );
        assert_eq!(listed.section, entry.section);
        assert!(!listed.deletable);
    }
    let remap = menu
        .iter()
        .find(|m| m.label == "Remap")
        .expect("remap is listed");
    assert_eq!(remap.heading, "Math");
    let noise = menu
        .iter()
        .find(|m| m.label == "Voronoi Distance")
        .expect("voronoi is listed");
    assert_eq!(noise.heading, "Texture");
    assert_eq!(noise.section, ["Helpers", "Voronoi"]);
}

#[test]
fn library_definitions_are_locked_and_never_saved() {
    let mut model = GraphModel::default();
    let entry = library::entries().first().expect("an entry");
    let node = model
        .add_group_instance(&entry.reference, Point::default())
        .expect("adds");
    assert!(model.project().definitions[&entry.reference].locked);
    model.enter_group(&node);
    assert!(model.is_locked());
    assert!(model.add_node("add", Point::default()).is_none());
    model.exit_group();
    let json = serde_json::to_string(model.project()).expect("serialises");
    let saved: serde_json::Value = serde_json::from_str(&json).expect("is JSON");
    let keys: Vec<&String> = saved["definitions"]
        .as_object()
        .expect("definitions")
        .keys()
        .collect();
    assert_eq!(keys, [ROOT_ID]);
    assert!(model.prunable_refs().iter().all(|r| !is_library_ref(r)));

    let reloaded: GraphProject = serde_json::from_str(&json).expect("loads");
    let model = GraphModel::new(reloaded);
    assert!(model.project().definitions.contains_key(&entry.reference));
    assert!(model.project().definitions.contains_key(ROOT_ID));
}

#[test]
fn an_exact_stored_older_version_resolves_to_its_own_definition() {
    let entry = library::entries().first().expect("an entry");
    let (lib, old) = library::library_with_older_version(&entry.id);
    let mut model = GraphModel::default();
    let node = model
        .add_group_instance(&entry.reference, Point::default())
        .expect("adds");
    model.edit(|project| {
        if let Some(group) = project
            .root_mut()
            .node_mut(&node)
            .and_then(|n| n.group.as_mut())
        {
            group.definition_ref = old.clone();
        }
    });
    let mut project = model.project().clone();
    project.definitions.remove(&old);
    library::register_with(&mut project, &lib);
    assert_eq!(
        project.definitions[&old].label.as_deref(),
        Some("Older test version")
    );
    assert_eq!(
        project.definitions[&entry.reference].label,
        entry.definition.label
    );
}

#[test]
fn an_unknown_library_version_is_not_resolved() {
    let entry = library::entries().first().expect("an entry");
    let (lib, _) = library::library_with_older_version(&entry.id);
    let unknown = format!("{}@0000000000000b", entry.id);
    assert!(library::resolve_with(&lib, &unknown).is_none());
    assert!(library::entry_for_ref(&unknown).is_none());
}

#[test]
fn an_unknown_library_version_fails_to_load() {
    let entry = library::entries().first().expect("an entry");
    let mut model = GraphModel::default();
    model
        .add_group_instance(&entry.reference, Point::default())
        .expect("adds");
    let json = model.project().to_json();
    let old = format!("{}@0000000000000a", entry.id);
    let stale = json.replace(&entry.reference, &old);
    assert_ne!(stale, json, "the saved project names the library group");
    match GraphProject::from_json(&stale) {
        Err(LoadError::InvalidGraph(issues)) => assert!(
            issues.iter().any(|issue| matches!(
                issue,
                GraphIssue::UnknownLibraryVersion { reference, .. } if *reference == old
            )),
            "{issues:?}"
        ),
        other => panic!("expected an unknown library version, got {other:?}"),
    }
    assert!(GraphProject::from_json(&json).is_ok());
}

/// An instance of `reference` with every output linked, summed into the
/// fragment output, generates valid WGSL.
fn check(reference: &str, bindings: &[(String, WgslType)]) {
    let mut g = TestGraph::new();
    let node = g
        .model
        .add_group_instance(reference, Point::default())
        .expect("adds");
    for (variable, ty) in bindings {
        let (variable, ty) = (variable.clone(), *ty);
        g.model.update_node_data(&node, |d| {
            d.types.insert(variable, ty);
        });
    }
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec4f);
    for output in g.model.resolve_node_outputs(&node) {
        let float = g.add("to-float");
        g.link((&node, &output.id), (&float, "e"));
        g.link((&float, "result"), (&sum, "operands"));
    }
    let out = g.output();
    g.link((&sum, "result"), (&out, "color"));
    let code = g.program().program.code;
    let result = std::panic::catch_unwind(|| crate::testing::validate_wgsl(&code));
    assert!(
        result.is_ok(),
        "{reference} with {bindings:?} generated invalid WGSL"
    );
}

#[test]
fn every_library_group_generates_valid_wgsl() {
    for entry in entries() {
        check(&entry.reference, &[]);
        for variable in &entry.definition.type_variables {
            for ty in &variable.types {
                check(&entry.reference, &[(variable.id.clone(), *ty)]);
            }
        }
    }
}

#[test]
fn library_inputs_and_outputs_resolve_to_types() {
    for entry in entries() {
        for handle in entry
            .definition
            .inputs
            .iter()
            .chain(&entry.definition.outputs)
        {
            if let HandleType::Variable { variable } = &handle.ty {
                assert!(
                    entry
                        .definition
                        .type_variables
                        .iter()
                        .any(|v| &v.id == variable),
                    "{} has a socket of an unknown type variable",
                    entry.id
                );
            }
        }
    }
}

#[test]
fn helper_groups_are_searched_after_the_main_results() {
    let model = GraphModel::default();
    let menu = add_menu_entries(&model);
    let results = crate::catalog::search_entries(&menu, "voronoi", &|_| true);
    let sections = crate::catalog::group_by_search_section(&results);
    assert_eq!(sections.first().map(|(s, _)| s.clone()), Some(None));
    assert!(
        sections
            .iter()
            .any(|(s, _)| s.as_deref() == Some("Helpers"))
    );
}

#[test]
fn the_texture_groups_are_in_the_library() {
    for id in [
        "std-fit-uv",
        "std-gaussian-blur",
        "std-gaussian-blur-1d",
        "std-gaussian-blur-2d",
        "std-box-blur",
        "std-box-blur-1d",
        "std-box-blur-2d",
        "std-box-blur-once",
    ] {
        assert!(
            entries().iter().any(|e| e.id == id),
            "{id} is in the library"
        );
    }
}

#[test]
fn a_blur_takes_and_gives_a_texture() {
    let entry = entries()
        .iter()
        .find(|e| e.id == "std-gaussian-blur")
        .expect("the blur");
    let texture = HandleType::Fixed(WgslType::Texture);
    assert!(entry.definition.inputs.iter().any(|h| h.ty == texture));
    assert_eq!(
        entry
            .definition
            .outputs
            .iter()
            .map(|h| &h.ty)
            .collect::<Vec<_>>(),
        [&texture]
    );
}
