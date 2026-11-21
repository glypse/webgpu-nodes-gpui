//! The standard library: locked group definitions shipped with the app.

use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::graph::{GraphDocument, GraphProject, GroupWidget};

const SOURCES: &[&str] = &[
    include_str!("library/std-aspect-uv.json"),
    include_str!("library/std-remap.json"),
    include_str!("library/std-color.json"),
    include_str!("library/std-linear-rgb-to-xyz.json"),
    include_str!("library/std-xyz-to-linear-rgb.json"),
    include_str!("library/std-xyz-to-oklab.json"),
    include_str!("library/std-oklab-to-xyz.json"),
    include_str!("library/std-linear-rgb-to-oklab.json"),
    include_str!("library/std-oklab-to-linear-rgb.json"),
    include_str!("library/std-oklab-to-oklch.json"),
    include_str!("library/std-oklch-to-oklab.json"),
    include_str!("library/std-linear-rgb-to-oklch.json"),
    include_str!("library/std-oklch-to-linear-rgb.json"),
    include_str!("library/std-in-display-gamut.json"),
    include_str!("library/std-oklch-gamut-map.json"),
    include_str!("library/std-noise-coordinates.json"),
    include_str!("library/std-hash.json"),
    include_str!("library/std-lattice-neighbor.json"),
    include_str!("library/std-voronoi-point.json"),
    include_str!("library/std-voronoi-distance.json"),
    include_str!("library/std-voronoi-texture.json"),
    include_str!("library/std-fractal-octave.json"),
    include_str!("library/std-fractal-voronoi.json"),
    include_str!("library/std-voronoi-nearest.json"),
    include_str!("library/std-voronoi-edges.json"),
    include_str!("library/std-voronoi-radius.json"),
    include_str!("library/std-voronoi-smooth.json"),
    include_str!("library/std-voronoi-bisector.json"),
    include_str!("library/std-pad-vector.json"),
    include_str!("library/std-component-sum.json"),
    include_str!("library/std-max-component.json"),
    include_str!("library/std-lattice-gradient.json"),
    include_str!("library/std-simplex-kernel.json"),
    include_str!("library/std-opensimplex-orientation.json"),
    include_str!("library/std-gradient-noise-1d.json"),
    include_str!("library/std-opensimplex-2d.json"),
    include_str!("library/std-opensimplex-3d-half.json"),
    include_str!("library/std-opensimplex-3d.json"),
    include_str!("library/std-opensimplex-4d.json"),
    include_str!("library/std-opensimplex-noise.json"),
    include_str!("library/std-noise-distortion.json"),
    include_str!("library/std-octave-shift.json"),
    include_str!("library/std-fractal-noise-octaves.json"),
    include_str!("library/std-fractal-noise.json"),
    include_str!("library/std-fit-uv.json"),
    include_str!("library/std-gaussian-blur-1d.json"),
    include_str!("library/std-box-blur-1d.json"),
    include_str!("library/std-gaussian-blur.json"),
    include_str!("library/std-box-blur.json"),
    include_str!("library/std-box-blur-once.json"),
    include_str!("library/std-gaussian-blur-2d.json"),
    include_str!("library/std-box-blur-2d.json"),
];

/// Separates a library id from its version in a definition ref.
const VERSION_SEPARATOR: char = '@';

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("library file {index} is not valid: {source}")]
    Invalid { index: usize, source: serde_json::Error },
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
    let Some((id, _)) = parse_ref(reference) else { return false };
    entries().iter().any(|e| e.id == id)
}

fn load() -> Result<Vec<LibraryEntry>, LibraryError> {
    let mut entries = Vec::with_capacity(SOURCES.len());
    for (index, source) in SOURCES.iter().enumerate() {
        let invalid = |source| LibraryError::Invalid { index, source };
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

static LIBRARY: LazyLock<Result<Vec<LibraryEntry>, LibraryError>> = LazyLock::new(load);

/// The library's entries in menu order; empty if the embedded files are
/// broken, which `load_error` reports.
pub fn entries() -> &'static [LibraryEntry] {
    LIBRARY.as_deref().unwrap_or_default()
}

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
        project.definitions.entry(entry.reference.clone()).or_insert_with(|| entry.definition.clone());
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
            project.definitions.insert(reference, entry.definition.clone());
        }
    }
}

/// A version id derived from what a definition computes and exposes: node
/// positions and the camera don't count. Stays equal to the id the original
/// app derives, so saved projects' refs keep pointing at the same entries.
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
    hash_text(&text)
}

/// `value` with every object's keys in order, as `JSON.stringify` of sorted entries gives.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<_> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        Value::Number(n) => match n.as_f64() {
            // JS prints 1.0 as 1.
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => Value::from(f as i64),
            _ => Value::Number(n),
        },
        other => other,
    }
}

/// cyrb53, as 14 hex digits.
fn hash_text(text: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for unit in text.encode_utf16() {
        let c = u32::from(unit);
        h1 = (h1 ^ c).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ c).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    let value = 2_097_152 * u64::from(h2) + u64::from(h1);
    format!("{value:014x}")
}
