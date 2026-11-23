//! Bit-exact CPU evaluation of described groups. Each group is evaluated on a
//! fixed set of samples and its outputs compared with `tests/golden/<id>.txt`,
//! so an optimisation that changes any output bit fails here. The golden files
//! are written with `STDLIB_WRITE_GOLDEN=1 cargo test -p stdlib --test cpu`.

use std::path::PathBuf;

use serde_json::{Map, Value, json};
use shader_graph::evaluate::evaluate;
use shader_graph::graph::{GraphModel, GraphProject, ROOT_ID};
use shader_graph::library::{make_ref, version_of};

/// Groups whose outputs are evaluated on the CPU and guarded.
const GROUPS: &[&str] = &[
    "std-remap",
    "std-component-sum",
    "std-max-component",
    "std-pad-vector",
    "std-linear-rgb-to-xyz",
    "std-xyz-to-linear-rgb",
    "std-linear-rgb-to-oklab",
    "std-oklab-to-linear-rgb",
    "std-oklab-to-oklch",
    "std-oklch-to-oklab",
    "std-linear-rgb-to-oklch",
    "std-oklch-to-linear-rgb",
    "std-in-display-gamut",
    "std-oklch-gamut-map",
    "std-color",
    "std-aspect-uv",
    "std-hash",
    "std-lattice-neighbor",
    "std-noise-coordinates",
    "std-lattice-gradient",
    "std-simplex-kernel",
    "std-gradient-noise-1d",
    "std-opensimplex-2d",
    "std-opensimplex-3d-half",
    "std-opensimplex-3d",
    "std-opensimplex-4d",
    "std-opensimplex-orientation",
    "std-opensimplex-noise",
    "std-octave-shift",
    "std-fractal-octave",
    "std-fractal-noise-octaves",
    "std-noise-distortion",
    "std-voronoi-point",
    "std-voronoi-distance",
    "std-voronoi-bisector",
    "std-voronoi-nearest",
    "std-voronoi-radius",
    "std-voronoi-smooth",
    "std-voronoi-edges",
    "std-voronoi-texture",
];

fn library_path(id: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../shader-graph/src/library")
        .join(format!("{id}.json"))
}

fn library_file(id: &str) -> Value {
    let text = std::fs::read_to_string(library_path(id)).expect("library file");
    serde_json::from_str(&text).expect("library json")
}

/// A small deterministic generator, so the samples never change.
struct Samples(u64);

impl Samples {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A value in `[lo, hi]`, now and then one of the values where branches meet.
    fn float(&mut self, lo: f64, hi: f64) -> f64 {
        let specials: [f64; 6] = [0.0, 1.0, -1.0, 0.5, 1e-6, 2.0];
        let r = self.next();
        if r < 0.15 {
            let s = specials[(self.next() * specials.len() as f64) as usize % specials.len()];
            s.clamp(lo, hi)
        } else {
            lo + (hi - lo) * self.next()
        }
    }
}

fn component_names(width: usize) -> &'static [&'static str] {
    &["x", "y", "z", "w"][..width]
}

fn width(ty: &str) -> Option<usize> {
    match ty {
        "f32" | "i32" | "u32" | "bool" => Some(1),
        "vec2f" | "vec2i" => Some(2),
        "vec3f" | "vec3i" => Some(3),
        "vec4f" | "vec4i" => Some(4),
        _ => None,
    }
}

/// One sample for each input of the group: its value and, for a type
/// variable, the type it takes.
fn sample_inputs(
    definition: &Value,
    samples: &mut Samples,
) -> Option<(Map<String, Value>, Map<String, Value>)> {
    let mut values = Map::new();
    let mut types = Map::new();
    let type_variables = definition["definition"]["typeVariables"].as_array();
    let tv_types = |id: &str| -> Vec<String> {
        type_variables
            .into_iter()
            .flatten()
            .find(|tv| tv["id"] == id)
            .map(|tv| {
                tv["types"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    for input in definition["definition"]["inputs"].as_array()? {
        let id = input["id"].as_str()?.to_owned();
        if let Some(variants) = input["variants"].as_array() {
            let pick = variants[(samples.next() * variants.len() as f64) as usize % variants.len()]
                ["value"]
                .clone();
            values.insert(id, pick);
            continue;
        }
        let ty = match &input["type"] {
            Value::String(ty) => ty.clone(),
            Value::Object(variable) => {
                let tv = variable["variable"].as_str()?;
                let choices = tv_types(tv);
                let pick = choices
                    [(samples.next() * choices.len() as f64) as usize % choices.len()]
                .clone();
                types.insert(tv.to_owned(), json!(pick));
                pick
            }
            _ => return None,
        };
        let n = width(&ty)?;
        let (lo, hi) = (
            input["min"].as_f64().unwrap_or(-2.0),
            input["max"].as_f64().unwrap_or(2.0),
        );
        let value = match ty.as_str() {
            "i32" | "u32" => json!((samples.next() * 8.0).floor() as i64 - 3),
            "bool" => json!((samples.next() < 0.5) as i64),
            _ if n == 1 => json!(samples.float(lo, hi)),
            _ => {
                let mut object = Map::new();
                for name in component_names(n) {
                    object.insert((*name).to_owned(), json!(samples.float(lo, hi)));
                }
                Value::Object(object)
            }
        };
        values.insert(id, value);
    }
    Some((values, types))
}

fn model(group_ref: &str, values: Map<String, Value>, types: Map<String, Value>) -> GraphModel {
    let json = json!({
        "definitions": {
            ROOT_ID: {
                "nodes": [{
                    "id": "g",
                    "type": "node-group",
                    "data": { "values": values, "types": types },
                    "position": { "x": 0, "y": 0 },
                    "group": { "definitionRef": group_ref, "locked": true },
                }],
                "edges": [],
            }
        }
    });
    let mut project: GraphProject = serde_json::from_value(json).expect("project");
    shader_graph::library::register(&mut project);
    GraphModel::new(project)
}

/// The output bits of every sample, one line per sample.
fn evaluate_group(id: &str) -> String {
    let file = library_file(id);
    let group_ref = make_ref(id, &version_of(&file["definition"]));
    let outputs: Vec<String> = file["definition"]["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .map(|o| o["id"].as_str().expect("output id").to_owned())
        .collect();
    let mut samples = Samples(0x5EED ^ id.len() as u64);
    let mut lines = String::new();
    for _ in 0..sample_count(id) {
        let Some((values, types)) = sample_inputs(&file, &mut samples) else {
            panic!("{id}: cannot sample its inputs");
        };
        let graph = model(&group_ref, values, types);
        let mut fields = Vec::new();
        for output in &outputs {
            match evaluate(&graph, "g", output) {
                Ok(components) => fields.push(
                    components
                        .iter()
                        .map(|c| format!("{:016x}", c.to_bits()))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                Err(error) => fields.push(format!("ERR {}", error.0)),
            }
        }
        lines.push_str(&fields.join(" | "));
        lines.push('\n');
    }
    lines
}

/// Voronoi groups evaluate their nested repeats for every output, so they get fewer samples.
/// Fractal Voronoi is left out: each sample evaluates a Voronoi texture per octave, which is
/// too slow to run as a test; its outputs are built from the guarded Voronoi groups.
fn sample_count(id: &str) -> usize {
    if id.starts_with("std-voronoi") {
        16
    } else {
        120
    }
}

#[test]
fn cpu_outputs_are_bit_identical_to_the_golden_files() {
    let writing = std::env::var_os("STDLIB_WRITE_GOLDEN").is_some();
    let mut failures = Vec::new();
    for id in GROUPS {
        let actual = evaluate_group(id);
        let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(format!("{id}.txt"));
        if writing {
            std::fs::write(&golden, &actual).expect("write golden");
            continue;
        }
        let expected = std::fs::read_to_string(&golden).unwrap_or_default();
        if expected != actual {
            let line = expected
                .lines()
                .zip(actual.lines())
                .position(|(e, a)| e != a)
                .unwrap_or(expected.lines().count().min(actual.lines().count()));
            failures.push(format!("{id}: first difference at sample {line}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
