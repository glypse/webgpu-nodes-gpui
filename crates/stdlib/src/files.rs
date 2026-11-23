//! The library's files on disk: reading them, writing built groups, and
//! keeping every ref to a rebuilt group current.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use shader_graph::assets::{BundleError, BundleKind, pack_bundle, unpack_bundle};
use thiserror::Error;

use crate::build::BuiltGroup;
use crate::json::{self, JsonError};

#[derive(Debug, Error)]
pub enum FilesError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: JsonError },
    #[error("{path}: {source}")]
    Bundle { path: PathBuf, source: BundleError },
    #[error("{0} has no SOURCES list to register a library file in")]
    NoSources(PathBuf),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> FilesError + '_ {
    move |source| FilesError::Io {
        path: path.to_owned(),
        source,
    }
}

fn shader_graph_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../shader-graph/src")
}

/// Where the library's JSON files are.
pub fn library_dir() -> PathBuf {
    shader_graph_src().join("library")
}

/// The projects shipped with the app, which reference library groups.
pub fn scenes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/scenes")
}

fn path_of(id: &str) -> PathBuf {
    library_dir().join(format!("{id}.json"))
}

/// The text of the library file `id`, if there is one.
pub fn read_text(id: &str) -> Result<Option<String>, FilesError> {
    let path = path_of(id);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(&path)(e)),
    }
}

/// The library file `id` as a value, if there is one.
pub fn read_value(id: &str) -> Result<Option<Value>, FilesError> {
    read_text(id)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|e| FilesError::Json {
                path: path_of(id),
                source: e.into(),
            })
        })
        .transpose()
}

/// The ref the library file `id` has now.
pub fn current_ref(id: &str) -> Result<Option<String>, FilesError> {
    Ok(read_value(id)?.and_then(|file| {
        let version = shader_graph::library::version_of(file.get("definition")?);
        Some(shader_graph::library::make_ref(id, &version))
    }))
}

/// Whether the library's file for `group` is what it describes (where the nodes are aside).
pub fn is_current(group: &BuiltGroup) -> Result<bool, FilesError> {
    let Some(file) = read_value(group.spec.id)? else {
        return Ok(false);
    };
    let described = json::file_value(group).map_err(|source| FilesError::Json {
        path: path_of(group.spec.id),
        source,
    })?;
    Ok(json::unplaced(described) == json::unplaced(file))
}

/// Write the library file of `group` and list it in the library's sources.
pub fn write_group(group: &BuiltGroup) -> Result<(), FilesError> {
    let path = path_of(group.spec.id);
    let text = json::render(group).map_err(|source| FilesError::Json {
        path: path.clone(),
        source,
    })?;
    fs::write(&path, text).map_err(io_error(&path))?;
    register(group.spec.id)
}

/// Add `id` to the library's embedded sources if it isn't there.
fn register(id: &str) -> Result<(), FilesError> {
    let path = shader_graph_src().join("library.rs");
    let source = fs::read_to_string(&path).map_err(io_error(&path))?;
    let file = format!("library/{id}.json");
    if source.contains(&format!("\"{file}\"")) {
        return Ok(());
    }
    let list = source
        .find("const SOURCES")
        .ok_or_else(|| FilesError::NoSources(path.clone()))?;
    let end = source
        .get(list..)
        .and_then(|rest| rest.find("\n];"))
        .map(|at| list + at + 1)
        .ok_or_else(|| FilesError::NoSources(path.clone()))?;
    let single = format!("    (\"{id}\", include_str!(\"{file}\")),\n");
    let entry = if single.trim_end().len() <= 100 {
        single
    } else {
        format!("    (\n        \"{id}\",\n        include_str!(\"{file}\"),\n    ),\n")
    };
    let mut updated = source.clone();
    updated.insert_str(end, &entry);
    fs::write(&path, updated).map_err(io_error(&path))
}

/// The refs in `text`: `"id@version"` strings whose id is in `ids`.
fn refs_in<'a>(text: &'a str, ids: &BTreeMap<String, String>) -> Vec<&'a str> {
    let mut found = Vec::new();
    for id in ids.keys() {
        let needle = format!("\"{id}@");
        let mut rest = text;
        while let Some(at) = rest.find(&needle) {
            let start = at + 1;
            let Some(after) = rest.get(start..) else {
                break;
            };
            let Some(len) = after.find('"') else {
                break;
            };
            if let Some(reference) = after.get(..len) {
                found.push(reference);
            }
            rest = after.get(len..).unwrap_or("");
        }
    }
    found
}

/// What a rebuild changed elsewhere.
#[derive(Default)]
pub struct Rewritten {
    /// Library files rewritten, by id.
    pub library: Vec<String>,
    /// Scene files rewritten.
    pub scenes: Vec<PathBuf>,
    /// Every old ref and the one that replaces it.
    pub renames: BTreeMap<String, String>,
}

/// Point every ref to an older version of a group at its current one, in the
/// library files of groups that aren't described (which take new versions
/// themselves, so their dependants follow) and in the shipped scenes.
/// `latest` maps the id of each group built to its ref; `old` holds the refs
/// the rebuilt groups had. Files are only written unless `dry`.
pub fn update_refs(
    latest: &BTreeMap<String, String>,
    old: &[String],
    dry: bool,
) -> Result<Rewritten, FilesError> {
    let mut latest = latest.clone();
    let mut result = Rewritten::default();
    // The ref each id had before the rebuild.
    let mut before: BTreeMap<String, String> = BTreeMap::new();
    for reference in old {
        if let Some((id, _)) = shader_graph::library::parse_ref(reference) {
            before.insert(id.to_owned(), reference.clone());
        }
    }
    let described: Vec<String> = latest.keys().cloned().collect();
    let mut others: BTreeMap<String, String> = BTreeMap::new();
    let dir = library_dir();
    for entry in fs::read_dir(&dir).map_err(io_error(&dir))? {
        let path = entry.map_err(io_error(&dir))?.path();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|_| path.extension().is_some_and(|e| e == "json"));
        if let Some(id) = id.filter(|id| !described.iter().any(|d| d == id)) {
            others.insert(
                id.to_owned(),
                fs::read_to_string(&path).map_err(io_error(&path))?,
            );
        }
    }
    for id in others.keys() {
        let current = text_ref(id, others.get(id).map_or("", String::as_str))?;
        before.insert(id.clone(), current.clone());
        latest.insert(id.clone(), current);
    }
    // A file taking a new version changes the refs of the files calling it: go on until none moves.
    loop {
        let mut moved = false;
        let ids: Vec<String> = others.keys().cloned().collect();
        for id in ids {
            let Some(text) = others.get(&id) else {
                continue;
            };
            let mut updated = text.clone();
            for reference in refs_in(text, &latest) {
                let stale = shader_graph::library::parse_ref(reference)
                    .and_then(|(ref_id, _)| latest.get(ref_id))
                    .filter(|new| new.as_str() != reference);
                if let Some(new) = stale {
                    updated = updated.replace(&format!("\"{reference}\""), &format!("\"{new}\""));
                }
            }
            if updated != *text {
                latest.insert(id.clone(), text_ref(&id, &updated)?);
                others.insert(id.clone(), updated);
                if !result.library.contains(&id) {
                    result.library.push(id);
                }
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for (id, new) in &latest {
        if let Some(old) = before.get(id).filter(|old| *old != new) {
            result.renames.insert(old.clone(), new.clone());
        }
    }
    if !dry {
        for id in &result.library {
            if let Some(text) = others.get(id) {
                let path = path_of(id);
                fs::write(&path, text).map_err(io_error(&path))?;
            }
        }
    }
    let scenes = scenes_dir();
    if let Ok(entries) = fs::read_dir(&scenes) {
        for entry in entries.flatten() {
            let path = entry.path();
            let bundle = path
                .extension()
                .is_some_and(|e| e == BundleKind::Project.extension());
            if !bundle && path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let renamed = if bundle {
                rename_in_bundle(&path, &latest)?
            } else {
                let text = fs::read_to_string(&path).map_err(io_error(&path))?;
                let updated = rename_in_text(&text, &latest);
                (updated != text).then(|| updated.into_bytes())
            };
            if let Some(bytes) = renamed {
                if !dry {
                    fs::write(&path, bytes).map_err(io_error(&path))?;
                }
                result.scenes.push(path);
            }
        }
    }
    Ok(result)
}

/// `text` with every ref to an older version of a group in `latest` (id to ref) made current.
fn rename_in_text(text: &str, latest: &BTreeMap<String, String>) -> String {
    let mut updated = text.to_owned();
    for reference in refs_in(text, latest) {
        let current = shader_graph::library::parse_ref(reference)
            .and_then(|(id, _)| latest.get(id))
            .filter(|new| new.as_str() != reference);
        if let Some(new) = current {
            updated = updated.replace(&format!("\"{reference}\""), &format!("\"{new}\""));
        }
    }
    updated
}

/// A project bundle with its refs made current (see `rename_in_text`), `None` if none changed.
pub fn rename_in_bundle(
    path: &Path,
    latest: &BTreeMap<String, String>,
) -> Result<Option<Vec<u8>>, FilesError> {
    let bundle_error = |e: BundleError| FilesError::Bundle {
        path: path.to_owned(),
        source: e,
    };
    let bytes = fs::read(path).map_err(io_error(path))?;
    let bundle = unpack_bundle(&bytes, BundleKind::Project).map_err(bundle_error)?;
    let json = rename_in_text(&bundle.json, latest);
    if json == bundle.json {
        return Ok(None);
    }
    let files = bundle.files.iter().map(|(p, d)| (p.as_str(), d.as_slice()));
    pack_bundle(BundleKind::Project, &json, files)
        .map(Some)
        .map_err(bundle_error)
}

fn text_ref(id: &str, text: &str) -> Result<String, FilesError> {
    let value: Value = serde_json::from_str(text).map_err(|e| FilesError::Json {
        path: path_of(id),
        source: e.into(),
    })?;
    let version = value
        .get("definition")
        .map(shader_graph::library::version_of)
        .unwrap_or_default();
    Ok(shader_graph::library::make_ref(id, &version))
}
