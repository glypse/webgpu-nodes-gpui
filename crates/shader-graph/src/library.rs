//! The standard library: locked group definitions shipped with the app.

use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::graph::{GraphDocument, GraphProject, GroupWidget};

/// Each library file's id and text.
const SOURCES: &[(&str, &str)] = &[
    ("std-aspect-uv", include_str!("library/std-aspect-uv.json")),
    ("std-remap", include_str!("library/std-remap.json")),
    ("std-color", include_str!("library/std-color.json")),
    (
        "std-linear-rgb-to-xyz",
        include_str!("library/std-linear-rgb-to-xyz.json"),
    ),
    (
        "std-xyz-to-linear-rgb",
        include_str!("library/std-xyz-to-linear-rgb.json"),
    ),
    (
        "std-xyz-to-oklab",
        include_str!("library/std-xyz-to-oklab.json"),
    ),
    (
        "std-oklab-to-xyz",
        include_str!("library/std-oklab-to-xyz.json"),
    ),
    (
        "std-linear-rgb-to-oklab",
        include_str!("library/std-linear-rgb-to-oklab.json"),
    ),
    (
        "std-oklab-to-linear-rgb",
        include_str!("library/std-oklab-to-linear-rgb.json"),
    ),
    (
        "std-oklab-to-oklch",
        include_str!("library/std-oklab-to-oklch.json"),
    ),
    (
        "std-oklch-to-oklab",
        include_str!("library/std-oklch-to-oklab.json"),
    ),
    (
        "std-linear-rgb-to-oklch",
        include_str!("library/std-linear-rgb-to-oklch.json"),
    ),
    (
        "std-oklch-to-linear-rgb",
        include_str!("library/std-oklch-to-linear-rgb.json"),
    ),
    (
        "std-in-display-gamut",
        include_str!("library/std-in-display-gamut.json"),
    ),
    (
        "std-oklch-gamut-map",
        include_str!("library/std-oklch-gamut-map.json"),
    ),
    (
        "std-noise-coordinates",
        include_str!("library/std-noise-coordinates.json"),
    ),
    ("std-hash", include_str!("library/std-hash.json")),
    (
        "std-lattice-neighbor",
        include_str!("library/std-lattice-neighbor.json"),
    ),
    (
        "std-voronoi-point",
        include_str!("library/std-voronoi-point.json"),
    ),
    (
        "std-voronoi-distance",
        include_str!("library/std-voronoi-distance.json"),
    ),
    (
        "std-voronoi-texture",
        include_str!("library/std-voronoi-texture.json"),
    ),
    (
        "std-fractal-octave",
        include_str!("library/std-fractal-octave.json"),
    ),
    (
        "std-fractal-voronoi",
        include_str!("library/std-fractal-voronoi.json"),
    ),
    (
        "std-voronoi-nearest",
        include_str!("library/std-voronoi-nearest.json"),
    ),
    (
        "std-voronoi-edges",
        include_str!("library/std-voronoi-edges.json"),
    ),
    (
        "std-voronoi-radius",
        include_str!("library/std-voronoi-radius.json"),
    ),
    (
        "std-voronoi-smooth",
        include_str!("library/std-voronoi-smooth.json"),
    ),
    (
        "std-voronoi-bisector",
        include_str!("library/std-voronoi-bisector.json"),
    ),
    (
        "std-pad-vector",
        include_str!("library/std-pad-vector.json"),
    ),
    (
        "std-component-sum",
        include_str!("library/std-component-sum.json"),
    ),
    (
        "std-max-component",
        include_str!("library/std-max-component.json"),
    ),
    (
        "std-lattice-gradient",
        include_str!("library/std-lattice-gradient.json"),
    ),
    (
        "std-simplex-kernel",
        include_str!("library/std-simplex-kernel.json"),
    ),
    (
        "std-opensimplex-orientation",
        include_str!("library/std-opensimplex-orientation.json"),
    ),
    (
        "std-gradient-noise-1d",
        include_str!("library/std-gradient-noise-1d.json"),
    ),
    (
        "std-opensimplex-2d",
        include_str!("library/std-opensimplex-2d.json"),
    ),
    (
        "std-opensimplex-3d-half",
        include_str!("library/std-opensimplex-3d-half.json"),
    ),
    (
        "std-opensimplex-3d",
        include_str!("library/std-opensimplex-3d.json"),
    ),
    (
        "std-opensimplex-4d",
        include_str!("library/std-opensimplex-4d.json"),
    ),
    (
        "std-opensimplex-noise",
        include_str!("library/std-opensimplex-noise.json"),
    ),
    (
        "std-noise-distortion",
        include_str!("library/std-noise-distortion.json"),
    ),
    (
        "std-octave-shift",
        include_str!("library/std-octave-shift.json"),
    ),
    (
        "std-fractal-noise-octaves",
        include_str!("library/std-fractal-noise-octaves.json"),
    ),
    (
        "std-fractal-noise",
        include_str!("library/std-fractal-noise.json"),
    ),
    ("std-fit-uv", include_str!("library/std-fit-uv.json")),
    (
        "std-gaussian-blur-1d",
        include_str!("library/std-gaussian-blur-1d.json"),
    ),
    (
        "std-box-blur-1d",
        include_str!("library/std-box-blur-1d.json"),
    ),
    (
        "std-gaussian-blur",
        include_str!("library/std-gaussian-blur.json"),
    ),
    ("std-box-blur", include_str!("library/std-box-blur.json")),
    (
        "std-box-blur-once",
        include_str!("library/std-box-blur-once.json"),
    ),
    (
        "std-gaussian-blur-2d",
        include_str!("library/std-gaussian-blur-2d.json"),
    ),
    (
        "std-box-blur-2d",
        include_str!("library/std-box-blur-2d.json"),
    ),
];

/// Older stored versions of library entries, resolvable by ref only. Empty
/// while every entry has shipped in one version.
const OLDER_SOURCES: &[(&str, &str)] = &[];

/// Separates a library id from its version in a definition ref.
const VERSION_SEPARATOR: char = '@';
/// Bytes of the version digest kept: 64 bits, as hex.
const VERSION_BYTES: usize = 8;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("library file {id}.json is not valid: {source}")]
    Invalid {
        id: &'static str,
        source: serde_json::Error,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryFile {
    id: String,
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    category: String,
    #[serde(default)]
    section: Vec<String>,
    search_section: Option<String>,
    dependencies: Vec<String>,
    definition: GraphDocument,
    widget: Option<GroupWidget>,
}

/// A library group at its latest version.
#[derive(Clone, Debug)]
pub struct LibraryEntry {
    pub id: String,
    pub version: String,
    /// What group nodes reference: the id and version.
    pub reference: String,
    pub label: String,
    pub aliases: Vec<String>,
    /// A node category's id, or the name of a menu heading of its own.
    pub category: String,
    /// Nested menu sections under its heading, outermost first.
    pub section: Vec<String>,
    /// The section of search results it is listed in.
    pub search_section: Option<String>,
    pub dependencies: Vec<String>,
    /// Locked, labelled and carrying the entry's widget.
    pub definition: GraphDocument,
}

/// The id and version a library ref points at; `None` for other refs.
pub fn parse_ref(reference: &str) -> Option<(&str, &str)> {
    let at = reference.rfind(VERSION_SEPARATOR).filter(|at| *at > 0)?;
    Some((&reference[..at], &reference[at + 1..]))
}

/// Whether `reference` is any version of a library entry.
pub fn is_library_ref(reference: &str) -> bool {
    let Some((id, _)) = parse_ref(reference) else {
        return false;
    };
    entries().iter().any(|e| e.id == id)
}

/// The standard library's stored versions.
pub struct Library {
    /// The current version of each entry, in menu order.
    pub(crate) entries: Vec<LibraryEntry>,
    /// Older versions, kept so saved projects that reference them still load.
    pub(crate) older: Vec<LibraryEntry>,
}

fn load_source(id: &'static str, source: &str) -> Result<LibraryEntry, LibraryError> {
    let invalid = |source| LibraryError::Invalid { id, source };
    let raw: Value = serde_json::from_str(source).map_err(invalid)?;
    let version = raw.get("definition").map_or_else(String::new, version_of);
    let file: LibraryFile = serde_json::from_value(raw).map_err(invalid)?;
    let mut definition = file.definition;
    definition.label = Some(file.name.clone());
    definition.locked = true;
    definition.widget = file.widget;
    Ok(LibraryEntry {
        reference: format!("{}{VERSION_SEPARATOR}{version}", file.id),
        id: file.id,
        version,
        label: file.name,
        aliases: file.aliases,
        category: file.category,
        section: file.section,
        search_section: file.search_section,
        dependencies: file.dependencies,
        definition,
    })
}

pub(crate) fn load() -> Result<Library, LibraryError> {
    let entries = SOURCES
        .iter()
        .map(|&(id, source)| load_source(id, source))
        .collect::<Result<_, _>>()?;
    let older = OLDER_SOURCES
        .iter()
        .map(|&(id, source)| load_source(id, source))
        .collect::<Result<_, _>>()?;
    Ok(Library { entries, older })
}

static LIBRARY: LazyLock<Result<Library, LibraryError>> =
    LazyLock::new(|| load().inspect_err(|e| log::error!("the standard library isn't loaded: {e}")));

fn loaded() -> Option<&'static Library> {
    (*LIBRARY).as_ref().ok()
}

/// The library's current entries in menu order; empty if the embedded files are broken.
pub fn entries() -> &'static [LibraryEntry] {
    loaded().map_or(&[], |l| l.entries.as_slice())
}

#[cfg(test)]
pub fn load_error() -> Option<&'static LibraryError> {
    (*LIBRARY).as_ref().err()
}

fn find_in<'a>(library: &'a Library, reference: &str) -> Option<&'a LibraryEntry> {
    library
        .entries
        .iter()
        .chain(&library.older)
        .find(|e| e.reference == reference)
}

/// The stored entry whose id and version are exactly `reference`.
pub fn entry_for_ref(reference: &str) -> Option<&'static LibraryEntry> {
    find_in(loaded()?, reference)
}

/// Gives `project` a definition for every current library entry, and for each
/// stored version its group nodes reference. A ref to a version that isn't
/// stored stays unresolved, which validation reports.
pub fn register(project: &mut GraphProject) {
    if let Some(library) = loaded() {
        register_in(project, library);
    }
}

fn register_in(project: &mut GraphProject, library: &Library) {
    for entry in &library.entries {
        project
            .definitions
            .entry(entry.reference.clone())
            .or_insert_with(|| entry.definition.clone());
    }
    loop {
        let missing: Vec<(String, &LibraryEntry)> = project
            .definitions
            .values()
            .flat_map(GraphDocument::group_refs)
            .filter(|r| !project.definitions.contains_key(*r))
            .filter_map(|r| Some((r.to_owned(), find_in(library, r)?)))
            .collect();
        if missing.is_empty() {
            return;
        }
        for (reference, entry) in missing {
            project
                .definitions
                .insert(reference, entry.definition.clone());
        }
    }
}

/// The label of the entry a stored ref names when its id has a newer version
/// stored; `None` for the current version or a ref that isn't stored.
pub fn outdated_label(reference: &str) -> Option<String> {
    newer_version_label_in(loaded()?, reference)
}

fn newer_version_label_in(library: &Library, reference: &str) -> Option<String> {
    find_in(library, reference)?;
    let (id, _) = parse_ref(reference)?;
    let latest = library.entries.iter().find(|e| e.id == id)?;
    (latest.reference != reference).then(|| latest.label.clone())
}

/// The library with an extra stored older version of `id`, whose definition
/// differs from the current one's. Returns the library and the older ref.
#[cfg(test)]
pub fn library_with_older_version(id: &str) -> (Library, String) {
    let entries: Vec<LibraryEntry> = loaded().expect("the library loads").entries.clone();
    let mut older = entries
        .iter()
        .find(|e| e.id == id)
        .expect("the entry is in the library")
        .clone();
    older.version = "0000000000000a".to_owned();
    older.reference = format!("{}{VERSION_SEPARATOR}{}", older.id, older.version);
    older.definition.label = Some("Older test version".to_owned());
    let reference = older.reference.clone();
    (
        Library {
            entries,
            older: vec![older],
        },
        reference,
    )
}

#[cfg(test)]
pub fn register_with(project: &mut GraphProject, library: &Library) {
    register_in(project, library);
}

#[cfg(test)]
pub fn newer_version_label_with(library: &Library, reference: &str) -> Option<String> {
    newer_version_label_in(library, reference)
}

#[cfg(test)]
pub fn resolve_with<'a>(library: &'a Library, reference: &str) -> Option<&'a LibraryEntry> {
    find_in(library, reference)
}

/// A version id derived from what a definition computes and exposes: node
/// positions and the camera don't count.
fn version_of(definition: &Value) -> String {
    let mut hashed = Map::new();
    if let Some(Value::Array(nodes)) = definition.get("nodes") {
        let nodes = nodes
            .iter()
            .map(|node| {
                let mut kept = Map::new();
                for key in ["id", "type", "data", "group"] {
                    if let Some(value) = node.get(key) {
                        kept.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(kept)
            })
            .collect();
        hashed.insert("nodes".to_owned(), Value::Array(nodes));
    }
    for key in ["edges", "typeVariables", "inputs", "outputs", "defaults"] {
        if let Some(value) = definition.get(key) {
            hashed.insert(key.to_owned(), value.clone());
        }
    }
    let text = sorted(Value::Object(hashed)).to_string();
    Sha256::digest(text.as_bytes())
        .iter()
        .take(VERSION_BYTES)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `value` with every object's keys sorted, so its serialisation is canonical.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<_> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}
