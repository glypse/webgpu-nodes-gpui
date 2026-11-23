use std::collections::BTreeMap;

use shader_graph::assets::{BundleKind, pack_bundle, unpack_bundle};
use stdlib::files::rename_in_bundle;

#[test]
fn refs_inside_a_bundle_follow_the_latest_versions() {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join("scene.wgnodes");
    let json =
        r#"{"definitions":{"a":{"nodes":[{"group":{"definitionRef":"std-x@0000000000000000"}}]}}}"#;
    let bytes = pack_bundle(BundleKind::Project, json, []).expect("packs");
    std::fs::write(&path, bytes).expect("writes");
    let latest = BTreeMap::from([("std-x".to_owned(), "std-x@1111111111111111".to_owned())]);

    let renamed = rename_in_bundle(&path, &latest)
        .expect("reads")
        .expect("changed");
    let bundle = unpack_bundle(&renamed, BundleKind::Project).expect("unpacks");
    assert!(bundle.json.contains("std-x@1111111111111111"));
    assert!(!bundle.json.contains("std-x@0000000000000000"));

    std::fs::write(&path, renamed).expect("writes");
    assert!(rename_in_bundle(&path, &latest).expect("reads").is_none());
}
