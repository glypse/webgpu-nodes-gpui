//! Group bundles in the app: exporting the selected group to a file, and
//! importing the groups of the files picked.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{App, Context, Entity, Window};
use shader_graph::assets::BundleKind;
use shader_graph::graph::{GroupsBundle, GroupsExport, GroupsFile, read_groups};

use crate::assets::{pick_files, report_problems};
use crate::editor::Editor;
use crate::playback::Playback;
use crate::project::{self, AssetFiles, read_capped};

/// The groups bundle of an export: its JSON and the files of the assets it reads, as the project has them.
pub fn encode(export: &GroupsExport, files: &AssetFiles) -> Result<Vec<u8>> {
    project::pack(BundleKind::Groups, &export.json, &export.assets, files)
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Asks for groups bundles and imports the groups they hold: File > Import groups.
pub fn pick_and_import<T: 'static>(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let (editor, playback) = (editor.clone(), playback.clone());
    pick_files(true, window, cx, read_bundles, move |(bundles, problems), window, cx| {
        import_bundles(&editor, &playback, bundles, problems, window, cx);
    });
}

/// Reads and parses the bundles at `paths`. Those that can't be used are returned as problems.
fn read_bundles(paths: &[PathBuf]) -> (Vec<(String, GroupsBundle)>, Vec<String>) {
    let mut problems = Vec::new();
    let mut bundles = Vec::new();
    for path in paths {
        let name = project::file_name(path);
        let bytes = match read_capped(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        match read_groups(&bytes) {
            Ok(bundle) => {
                if !bundle.altered.is_empty() {
                    let names: Vec<&str> = bundle.altered.iter().map(|a| a.name()).collect();
                    problems.push(format!(
                        "{name}: these files aren't what the bundle says they are, so they were left out: {}.",
                        names.join(", ")
                    ));
                }
                bundles.push((name, bundle));
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    (bundles, problems)
}

/// Imports the groups of the bundles as one undo step, with the files of the assets they bring.
/// Files that can't be used are left out; the others come in together, and what went wrong is reported once.
fn import_bundles(
    editor: &Entity<Editor>,
    playback: &Entity<Playback>,
    mut bundles: Vec<(String, GroupsBundle)>,
    mut problems: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    if bundles.is_empty() {
        report_problems(window, &problems, cx);
        return;
    }

    let groups: Vec<GroupsFile> = bundles.iter().map(|(_, bundle)| bundle.file.clone()).collect();
    let mut result = None;
    editor.update(cx, |e, cx| e.edit(cx, |m| result = Some(m.import_groups(&groups))));
    match result {
        Some(Ok(import)) => {
            // Each asset's file is moved out of its bundle: an added asset is the only one reading that path.
            let files: Vec<(String, Arc<Vec<u8>>)> = import
                .added
                .iter()
                .filter_map(|added| {
                    let (_, bundle) = bundles.get_mut(added.source.bundle)?;
                    let data = bundle.files.remove(&added.source.path)?;
                    Some((added.asset.id().to_owned(), Arc::new(data)))
                })
                .collect();
            if !files.is_empty() {
                playback.update(cx, |p, cx| {
                    let mut all = p.asset_files().clone();
                    all.extend(files);
                    p.set_asset_files(all, window, cx);
                });
            }
            if !import.missing.is_empty() {
                log::warn!("imported groups have missing dependencies: {:?}", import.missing);
                problems.push(format!(
                    "{} imported groups rely on groups that weren't in the bundles.",
                    import.missing.len()
                ));
            }
        }
        Some(Err(e)) => {
            let names: Vec<&str> = bundles.iter().map(|(name, _)| name.as_str()).collect();
            problems.push(format!("{}: {e}", names.join(", ")));
        }
        None => {}
    }
    report_problems(window, &problems, cx);
}

#[cfg(test)]
mod tests {
    use shader_graph::assets::{Asset, AssetFile, asset_hash, pack_bundle};
    use shader_graph::graph::{GraphDocument, GraphModel, GraphNode, GraphProject, GroupRef, NODE_GROUP, Point};

    use super::*;

    fn image(id: &str, bytes: &[u8]) -> Asset {
        Asset::Image(AssetFile {
            id: id.into(),
            name: id.into(),
            file: format!("assets/images/{id}.png"),
            mime: "image/png".into(),
            bytes: bytes.len() as u64,
            hash: asset_hash(bytes),
            width: 1,
            height: 1,
        })
    }

    /// A model with one group that reads an image, exported with that image's file.
    fn exported(bytes: &[u8]) -> (GroupsExport, AssetFiles) {
        let mut node = GraphNode::new("image", Point::default());
        node.data.asset = Some("a1".to_owned());
        let doc = GraphDocument { nodes: vec![node], label: Some("Photo group".into()), ..GraphDocument::default() };
        let mut project = GraphProject::default();
        project.definitions.insert("g".to_owned(), doc);
        project.assets.push(image("a1", bytes));
        let export = GraphModel::new(project).export_group("g").expect("exports");
        let files = AssetFiles::from([("a1".to_owned(), Arc::new(bytes.to_vec()))]);
        (export, files)
    }

    #[test]
    fn an_export_names_its_file_and_carries_its_assets_files() {
        let (export, files) = exported(&[1, 2, 3]);
        assert_eq!(export.name, "photo-group");
        let bytes = encode(&export, &files).expect("encodes");
        let read = read_groups(&bytes).expect("reads");
        assert_eq!(read.file.groups.len(), 1);
        assert_eq!(read.file.assets.len(), 1);
        assert_eq!(read.files.get("assets/images/a1.png").map(Vec::as_slice), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn an_asset_without_its_file_is_exported_without_it() {
        let (export, _) = exported(&[1, 2, 3]);
        let bytes = encode(&export, &AssetFiles::new()).expect("encodes");
        let read = read_groups(&bytes).expect("reads");
        assert!(read.files.is_empty());
        assert_eq!(read.file.assets.len(), 1);
    }

    #[test]
    fn an_exported_group_nesting_another_brings_it_along() {
        let mut project = GraphProject::default();
        project.definitions.insert("inner".to_owned(), GraphDocument { label: Some("Inner".into()), ..GraphDocument::default() });
        let mut node = GraphNode::new(NODE_GROUP, Point::default());
        node.group = Some(GroupRef { definition_ref: "inner".to_owned(), locked: false });
        project.definitions.insert("outer".to_owned(), GraphDocument { nodes: vec![node], ..GraphDocument::default() });
        let export = GraphModel::new(project).export_group("outer").expect("exports");
        let read = read_groups(&pack_bundle(BundleKind::Groups, &export.json, []).expect("packs")).expect("reads");
        let ids: Vec<&str> = read.file.groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["outer", "inner"]);
    }
}
