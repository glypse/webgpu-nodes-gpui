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

pub(crate) fn load() -> Result<Vec<LibraryEntry>, LibraryError> {
    let mut entries = Vec::with_capacity(SOURCES.len());
    for &(id, source) in SOURCES {
        let invalid = |source| LibraryError::Invalid { id, source };
        let raw: Value = serde_json::from_str(source).map_err(invalid)?;
        let version = raw.get("definition").map_or_else(String::new, version_of);
        let file: LibraryFile = serde_json::from_value(raw).map_err(invalid)?;
        let mut definition = file.definition;
        definition.label = Some(file.name.clone());
        definition.locked = true;
        definition.widget = file.widget;
        entries.push(LibraryEntry {
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
        });
    }
    Ok(entries)
}

static LIBRARY: LazyLock<Result<Vec<LibraryEntry>, LibraryError>> =
    LazyLock::new(|| load().inspect_err(|e| log::error!("the standard library isn't loaded: {e}")));

/// The library's entries in menu order; empty if the embedded files are broken.
pub fn entries() -> &'static [LibraryEntry] {
    LIBRARY.as_deref().unwrap_or_default()
}

#[cfg(test)]
pub fn load_error() -> Option<&'static LibraryError> {
    LIBRARY.as_ref().err()
}

/// The entry a ref of any of its versions stands for: the latest.
pub fn entry_for_ref(reference: &str) -> Option<&'static LibraryEntry> {
    let (id, _) = parse_ref(reference)?;
    entries().iter().find(|e| e.id == id)
}

/// Gives `project` a definition for every library entry, and for every
/// library version its group nodes reference (the latest stands in for an
/// older one).
pub fn register(project: &mut GraphProject) {
    for entry in entries() {
        project
            .definitions
            .entry(entry.reference.clone())
            .or_insert_with(|| entry.definition.clone());
    }
    loop {
        let missing: Vec<(String, &'static LibraryEntry)> = project
            .definitions
            .values()
            .flat_map(GraphDocument::group_refs)
            .filter(|r| !project.definitions.contains_key(*r))
            .filter_map(|r| Some((r.to_owned(), entry_for_ref(r)?)))
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
