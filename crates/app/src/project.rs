use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use shader_graph::assets::{Asset, BundleKind, pack_bundle, unpack_bundle, verified_files};
use shader_graph::graph::{GraphProject, MAX_BUNDLE_BYTES};

pub const EXTENSION: &str = BundleKind::Project.extension();
const ZIP_MAGIC: &[u8] = b"PK";

/// The files of a project's assets, by asset id.
pub type AssetFiles = BTreeMap<String, Arc<Vec<u8>>>;

/// A project and the files of its assets.
pub struct Loaded {
    pub project: GraphProject,
    pub files: AssetFiles,
}

/// Reads a file whole, refusing one larger than a bundle can be, before reading it.
pub fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_BUNDLE_BYTES {
        return Err(std::io::Error::other("it is too large to use"));
    }
    fs::read(path)
}

/// Reads a project from a bundle (zip with `project.json` and its assets' files) or from bare JSON.
pub fn read(path: &Path) -> Result<Loaded> {
    let bytes = read_capped(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<Loaded> {
    if !bytes.starts_with(ZIP_MAGIC) {
        let json = std::str::from_utf8(bytes).map_err(|_| anyhow!("it isn't text or a bundle"))?;
        return Ok(Loaded {
            project: GraphProject::from_json(json)?,
            files: AssetFiles::new(),
        });
    }
    let bundle = unpack_bundle(bytes, BundleKind::Project)?;
    let project = GraphProject::from_json(&bundle.json)?;
    let verified = verified_files(bundle, &project.assets);
    for asset in &verified.altered {
        log::warn!(
            "the file of the asset \"{}\" isn't what the project says, so it's left out",
            asset.name()
        );
    }
    let files = verified
        .files
        .into_iter()
        .map(|(id, data)| (id, Arc::new(data)))
        .collect();
    Ok(Loaded { project, files })
}

/// Packs `json` and the files of the `assets` the project holds, as a bundle of `kind`.
pub fn pack(kind: BundleKind, json: &str, assets: &[Asset], files: &AssetFiles) -> Result<Vec<u8>> {
    let entries = assets.iter().filter_map(|asset| {
        let data = files.get(asset.id())?;
        Some((asset.file(), data.as_slice()))
    });
    Ok(pack_bundle(kind, json, entries)?)
}

/// Writes the project and its assets' files as a bundle, replacing the file only once it is whole.
pub fn write(path: &Path, project: &GraphProject, files: &AssetFiles) -> Result<()> {
    let bytes = pack(
        BundleKind::Project,
        &project.to_json(),
        &project.assets,
        files,
    )?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

fn last_project_file() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("webgpu-nodes")
            .join("last-project"),
    )
}

/// Remembers which project to reopen on the next start; `None` forgets it.
pub fn remember(path: Option<&Path>) {
    let Some(file) = last_project_file() else {
        return;
    };
    let result = match path {
        Some(path) => file
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&file, path.to_string_lossy().as_bytes())),
        None => match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    };
    if let Err(e) = result {
        log::warn!("couldn't remember the last project: {e}");
    }
}

pub fn last() -> Option<PathBuf> {
    let text = fs::read_to_string(last_project_file()?).ok()?;
    let path = PathBuf::from(text.trim());
    path.is_file().then_some(path)
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_graph::assets::{Asset, AssetFile, asset_hash};

    /// Loading registers the standard library, so compare against a project that has it.
    fn loaded_default() -> GraphProject {
        GraphProject::from_json(&GraphProject::default().to_json()).expect("default project loads")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wgnodes-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn image(bytes: &[u8]) -> Asset {
        Asset::Image(AssetFile {
            id: "a1".into(),
            name: "Photo".into(),
            file: "assets/images/a1.png".into(),
            mime: "image/png".into(),
            bytes: bytes.len() as u64,
            hash: asset_hash(bytes),
            width: 1,
            height: 1,
        })
    }

    #[test]
    fn a_saved_project_reads_back_the_same() {
        let dir = scratch("roundtrip");
        let path = dir.join("a.wgnodes");
        let project = loaded_default();
        write(&path, &project, &AssetFiles::new()).expect("write");
        assert_eq!(read(&path).expect("read").project, project);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_saved_project_keeps_its_assets_and_their_files() {
        let dir = scratch("assets");
        let path = dir.join("a.wgnodes");
        let bytes = [1u8, 2, 3];
        let mut project = loaded_default();
        project.assets.push(image(&bytes));
        let files = AssetFiles::from([("a1".to_owned(), Arc::new(bytes.to_vec()))]);
        write(&path, &project, &files).expect("write");
        let loaded = read(&path).expect("read");
        assert_eq!(loaded.project, project);
        assert_eq!(
            loaded.files.get("a1").map(|f| f.as_slice()),
            Some(&bytes[..])
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_file_that_isnt_what_its_asset_says_is_left_out() {
        let dir = scratch("altered");
        let path = dir.join("a.wgnodes");
        let mut project = loaded_default();
        project.assets.push(image(&[1, 2, 3]));
        let files = AssetFiles::from([("a1".to_owned(), Arc::new(vec![9, 9, 9]))]);
        write(&path, &project, &files).expect("write");
        let loaded = read(&path).expect("read");
        assert_eq!(loaded.project.assets.len(), 1);
        assert!(loaded.files.is_empty());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bare_json_reads_too() {
        let project = loaded_default();
        assert_eq!(
            parse(project.to_json().as_bytes()).expect("parse").project,
            project
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse(b"not a project").is_err());
        assert!(parse(b"{}").is_err());
        assert!(parse(b"PK garbage").is_err());
        assert!(parse(&[0xff, 0xfe, 0x00]).is_err());
        assert!(read(Path::new("/definitely/not/here.wgnodes")).is_err());
    }

    #[test]
    fn a_bundle_without_its_json_is_an_error() {
        let bytes = pack_bundle(BundleKind::Groups, "{}", []).expect("packs");
        assert!(parse(&bytes).is_err());
    }
}
