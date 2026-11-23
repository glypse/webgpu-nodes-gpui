use stdlib::build::build_groups;
use stdlib::{files, groups, json};

#[test]
fn every_library_file_matches_its_description() {
    let built = build_groups(&groups::all()).expect("the descriptions build");
    for group in &built {
        let file = files::read_value(group.spec.id)
            .expect("the file reads")
            .unwrap_or_else(|| {
                panic!(
                    "{} has no library file: run `cargo run -p stdlib`",
                    group.spec.id
                )
            });
        let described = json::unplaced(json::file_value(group).expect("a value"));
        assert!(
            described == json::unplaced(file),
            "{} drifted from its description: run `cargo run -p stdlib`",
            group.spec.id
        );
    }
}

#[test]
fn described_groups_are_listed_in_the_library() {
    let ids: Vec<&str> = shader_graph::library::entries()
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    for spec in groups::all() {
        assert!(
            ids.contains(&spec.id),
            "{} isn't in the library's sources",
            spec.id
        );
    }
}
