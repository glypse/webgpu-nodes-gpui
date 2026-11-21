use crate::assets::{Asset, AssetFile, AssetKind, VideoEnd, VideoFile, asset_file};
use crate::graph::{GraphModel, GraphProject, PlaybackChange};
use crate::testing::TestGraph;

fn image() -> Asset {
    Asset::Image(AssetFile {
        id: "a1".into(),
        name: "Photo".into(),
        file: asset_file(AssetKind::Image, "a1", "Photo.JPG"),
        mime: "image/jpeg".into(),
        bytes: 3,
        hash: "a".repeat(64),
        width: 4,
        height: 2,
    })
}

fn video() -> Asset {
    Asset::Video(VideoFile {
        file: AssetFile {
            id: "v1".into(),
            name: "Clip".into(),
            file: asset_file(AssetKind::Video, "v1", "clip.mp4"),
            mime: "video/mp4".into(),
            bytes: 3,
            hash: "b".repeat(64),
            width: 4,
            height: 2,
        },
        duration: 1.0,
        frame_rate: 30.0,
    })
}

#[test]
fn assets_round_trip_through_a_saved_project() {
    let mut model = GraphModel::default();
    assert!(model.add_asset(image()));
    let loaded = GraphProject::from_json(&model.project().to_json()).expect("loads");
    assert_eq!(loaded.assets, [image()]);
    assert!(!GraphModel::new(loaded).has_unsaved_changes());
}

#[test]
fn an_asset_is_added_once_and_only_if_it_is_what_an_import_makes() {
    let mut model = GraphModel::default();
    assert!(model.add_asset(image()));
    assert!(!model.add_asset(image()), "its id is taken");
    let mut bad = image();
    bad.file_info_mut().file = "project.json".into();
    bad.file_info_mut().id = "other".into();
    assert!(!model.add_asset(bad));
    assert_eq!(model.assets().len(), 1);
    assert_eq!(model.asset_by_hash(&"a".repeat(64)).map(Asset::id), Some("a1"));
}

#[test]
fn adding_renaming_and_removing_assets_undo() {
    let mut model = GraphModel::default();
    model.add_asset(image());
    model.rename_asset("a1", "  Holiday ");
    assert_eq!(model.asset("a1").map(Asset::name), Some("Holiday"));
    model.rename_asset("a1", "   ");
    assert_eq!(model.asset("a1").map(Asset::name), Some("Holiday"));
    model.remove_asset("a1");
    assert!(model.assets().is_empty());

    model.undo();
    assert_eq!(model.asset("a1").map(Asset::name), Some("Holiday"));
    model.undo();
    assert_eq!(model.asset("a1").map(Asset::name), Some("Photo"));
    model.undo();
    assert!(model.assets().is_empty());
    assert!(!model.has_unsaved_changes());
}

#[test]
fn nodes_read_assets_of_their_own_kind() {
    let mut g = TestGraph::new();
    g.model.add_asset(image());
    g.model.add_asset(video());
    let (img, vid) = (g.add("image"), g.add("video"));
    g.model.set_node_asset(&img, Some("v1"));
    assert_eq!(g.model.node(&img).and_then(|n| n.data.asset.clone()), None, "a video isn't an image");
    g.model.set_node_asset(&img, Some("a1"));
    g.model.set_node_asset(&vid, Some("v1"));
    assert_eq!(g.model.asset_usages().get("a1"), Some(&1));
    assert_eq!(g.model.asset_usages().get("v1"), Some(&1));
    assert!(!g.model.missing_asset(&img, |_| true));
    assert!(g.model.missing_asset(&img, |_| false));
    g.model.set_node_asset(&img, None);
    assert_eq!(g.model.asset_usages().get("a1"), None);
}

#[test]
fn a_video_plays_as_its_node_says() {
    let mut g = TestGraph::new();
    let vid = g.add("video");
    g.model.set_video_playback(&vid, PlaybackChange { speed: Some(2.0), ..Default::default() });
    g.model.set_video_playback(&vid, PlaybackChange { end: Some(VideoEnd::Hold), ..Default::default() });
    let playback = g.model.node(&vid).and_then(|n| n.data.playback).expect("set");
    assert_eq!((playback.start, playback.speed, playback.end), (0.0, 2.0, VideoEnd::Hold));
}

#[test]
fn an_image_node_generates_its_size_and_a_missing_asset_reads_nothing() {
    let mut g = TestGraph::new();
    g.model.add_asset(image());
    let img = g.add("image");
    let out = g.output();
    g.link((&img, "size"), (&out, "color"));
    g.model.set_node_asset(&img, Some("a1"));
    let plan = g.wgsl();
    assert!(plan.contains("textureDimensions"), "{plan}");
    let plan = g.program().plan;
    assert!(plan.resources.iter().any(|r| r.id() == "media:a1"));
    g.model.remove_asset("a1");
    let plan = g.program().plan;
    assert!(!plan.resources.iter().any(|r| r.id() == "media:a1"));
}
