//! OpenSimplex Noise in any dimension count, and Fractal Noise: its octaves
//! summed in Blender's fractal types (cycles/kernel/svm/fractal_noise.h).

use shader_graph::types::WgslType;

use super::lattices::{
    GRADIENT_NOISE_1D, OPENSIMPLEX_2D, OPENSIMPLEX_3D, OPENSIMPLEX_3D_HALF, OPENSIMPLEX_4D,
};
use super::orientation::{SIMPLEX_ORIENTATION, orientation_input, variant_input};
use crate::builder::{
    EnumRef, Frame, GroupBuilder, GroupSpec, Input, NodeBuilder, Operand, Output, Ref, RepeatBody,
    RepeatItem, TypeVar, Variant,
};
use crate::groups::helpers::{clamp_range, compare};
use crate::groups::noise::coordinates::{NOISE_COORDINATES, coordinates_input, coordinates_type};
use crate::groups::noise::fractal::{FractalInputs, Octave, fractal, fractal_inputs};
use crate::groups::noise::lattice::HASH;
use crate::groups::vector::PAD_VECTOR;

pub const OPENSIMPLEX_NOISE: &str = "std-opensimplex-noise";
pub const NOISE_DISTORTION: &str = "std-noise-distortion";
pub const OCTAVE_SHIFT: &str = "std-octave-shift";
pub const FRACTAL_NOISE_OCTAVES: &str = "std-fractal-noise-octaves";
pub const FRACTAL_NOISE: &str = "std-fractal-noise";

/// XORed into the seed for Color's second and third components.
const COLOR_SALTS: [f64; 2] = [1540483477.0, 668265263.0];
/// XORed into the seed for each axis of the distortion.
const DISTORTION_SALT: i64 = 1103515245;
/// How far apart, at most, the octaves are moved, in their own scale.
const OCTAVE_SPREAD: f64 = 128.0;

const FBM: i64 = 0;
const MULTIFRACTAL: i64 = 1;
const HYBRID: i64 = 2;
const HETERO: i64 = 3;
const RIDGED: i64 = 4;

/// The options every noise takes, on a group whose coordinates are some type.
#[derive(Clone)]
struct NoiseOptions {
    variant: EnumRef,
    orientation: EnumRef,
    seed: Ref,
}

fn noise_options(g: &GroupBuilder<'_>, t: &TypeVar) -> NoiseOptions {
    NoiseOptions {
        variant: variant_input(g, Some(t)),
        orientation: orientation_input(g, t),
        seed: g.input("seed", Input::new(WgslType::I32).label("Seed").default(0)),
    }
}

/// The OpenSimplex Noise group's value for `vector` under `options`.
fn noise(g: &GroupBuilder<'_>, vector: impl Into<Operand>, options: &NoiseOptions) -> Ref {
    g.call(OPENSIMPLEX_NOISE)
        .input("vector", vector)
        .input("variant", options.variant.value())
        .input("orientation", options.orientation.value())
        .input("seed", options.seed)
        .add()
        .out("value")
}

/// A match on `selector` with the given case for each of its variants.
fn pick(g: &GroupBuilder<'_>, selector: &EnumRef, cases: Vec<(i64, Operand)>) -> Ref {
    let mut switch = g.match_on(selector);
    for (value, operand) in cases {
        switch = switch.case(value, operand);
    }
    switch.add().first()
}

fn opensimplex_noise_body(g: &GroupBuilder<'_>) {
    let (t, vector) = coordinates_input(g);
    let options = noise_options(g, &t);
    let coordinates = g
        .call(NOISE_COORDINATES)
        .input("vector", vector)
        .add()
        .out("coordinates");
    let lattice = g
        .call(SIMPLEX_ORIENTATION)
        .input("vector", coordinates)
        .input("variant", options.variant.value())
        .input("orientation", options.orientation.value())
        .add()
        .out("lattice");
    let value = g.frame(
        Frame::new("Lattices").note(
            "OpenSimplex2 in as many dimensions as Vector has components,\nplain gradient noise for a single one. In [-1, 1].",
        ),
        || {
            let line = g
                .call(GRADIENT_NOISE_1D)
                .input("value", coordinates)
                .input("seed", options.seed)
                .add()
                .out("value");
            let lattice_value = |group: &str| {
                g.call(group)
                    .input("lattice", lattice)
                    .input("variant", options.variant.value())
                    .input("seed", options.seed)
                    .add()
                    .out("value")
            };
            g.width_switch(
                coordinates,
                (
                    line,
                    lattice_value(OPENSIMPLEX_2D),
                    lattice_value(OPENSIMPLEX_3D),
                    lattice_value(OPENSIMPLEX_4D),
                ),
            )
        },
    );
    g.output("value", value, Output::new(WgslType::F32).label("Value"));
}

fn noise_distortion_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let vector = g.input("vector", Input::new(&t).label("Vector"));
    let amount = g.input(
        "amount",
        Input::new(WgslType::F32).label("Amount").default(0.0),
    );
    let options = noise_options(g, &t);
    let padded = g
        .call(PAD_VECTOR)
        .input("vector", vector)
        .add()
        .out("padded");
    // Without distortion the zone runs no iterations, so its noises cost nothing.
    let iterations = g.op(
        "switch",
        (
            compare(g, amount, 0.0, "!="),
            g.constant(WgslType::I32, 0.0),
            g.constant(WgslType::I32, 1.0),
        ),
    );
    let distorted = g.repeat(
        iterations,
        vec![RepeatItem::new("Distorted", WgslType::Vec4f, padded)],
        |body| {
            let next = g.frame(
                Frame::new("Distortion").note(
                    "Moved by a noise of its own along each axis, times Amount: run once,\nor not at all without distortion, so its noises cost nothing then.",
                ),
                || {
                    let axis = |i: i64| {
                        // Wrapping to i32 matches the salt product's `| 0` in the WGSL the
                        // seed is computed in.
                        let salt = (DISTORTION_SALT * (i + 1)) as i32;
                        let seed = g
                            .node("bitwise")
                            .args((options.seed, g.constant(WgslType::I32, f64::from(salt))))
                            .setting("op", "^")
                            .add()
                            .first();
                        noise(
                            g,
                            vector,
                            &NoiseOptions {
                                seed,
                                ..options.clone()
                            },
                        )
                    };
                    let (x, y, z, w) = (axis(0), axis(1), axis(2), axis(3));
                    let offset = g.width_switch(
                        vector,
                        (
                            g.vec(WgslType::Vec4f, (x, 0.0, 0.0, 0.0)),
                            g.vec(WgslType::Vec4f, (x, y, 0.0, 0.0)),
                            g.vec(WgslType::Vec4f, (x, y, z, 0.0)),
                            g.vec(WgslType::Vec4f, (x, y, z, w)),
                        ),
                    );
                    g.op("add", (padded, g.op("mul", (offset, amount))))
                        .first()
                },
            );
            body.set("Distorted", next);
        },
    );
    g.output(
        "distorted",
        distorted.get("Distorted"),
        Output::new(&t).label("Distorted"),
    );
}

fn octave_shift_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let vector = g.input("vector", Input::new(&t).label("Vector"));
    let octave = g.input(
        "octave",
        Input::new(WgslType::I32).label("Octave").default(0),
    );
    let seed = g.input("seed", Input::new(WgslType::I32).label("Seed").default(0));
    g.frame(
        Frame::new("Shift").note(
            "Moved by a random amount per octave, so the octaves' lattices\ndon't line up at the origin, nor along their axes.",
        ),
        || {
            let random = g
                .call(HASH)
                .input("value", octave)
                .input("seed", seed)
                .add()
                .out("hash");
            let spread = g.op("mul", (random, OCTAVE_SPREAD));
            g.output(
                "shifted",
                g.op("add", (vector, spread)),
                Output::new(&t).label("Shifted"),
            );
        },
    );
}

/// Fractal Noise's type and the inputs its octaves read. Each type has an
/// Offset and Gain of its own, where it uses them: Blender's shared defaults
/// (0 and 1) leave Hybrid and Heterogeneous weighting their octaves by the
/// noise itself, and Ridged at the square of it, near black.
#[derive(Clone)]
struct OctaveInputs {
    selector: EnumRef,
    fractal: FractalInputs,
    normalize: Ref,
    hybrid_offset: Ref,
    hybrid_gain: Ref,
    hetero_offset: Ref,
    ridged_offset: Ref,
    ridged_gain: Ref,
}

fn octave_inputs(g: &GroupBuilder<'_>) -> OctaveInputs {
    let selector = g.enum_input(
        "type",
        Input::enumeration(
            [
                Variant::new(FBM, "fBm"),
                Variant::new(MULTIFRACTAL, "Multifractal"),
                Variant::new(HYBRID, "Hybrid Multifractal"),
                Variant::new(HETERO, "Heterogeneous Terrain"),
                Variant::new(RIDGED, "Ridged Multifractal"),
            ],
            FBM,
        )
        .label("Type"),
    );
    let fractal = fractal_inputs(g);
    let normalize = g.input(
        "normalize",
        Input::new(WgslType::Bool)
            .label("Normalize")
            .default(true)
            .owner(&selector, FBM),
    );
    let owned = |key: &str, label: &str, value: f64, owner: i64| {
        g.input(
            key,
            Input::new(WgslType::F32)
                .label(label)
                .default(value)
                .owner(&selector, owner),
        )
    };
    OctaveInputs {
        selector: selector.clone(),
        fractal,
        normalize,
        hybrid_offset: owned("hybrid_offset", "Offset", 0.5, HYBRID),
        hybrid_gain: owned("hybrid_gain", "Gain", 1.0, HYBRID),
        hetero_offset: owned("hetero_offset", "Offset", 0.5, HETERO),
        ridged_offset: owned("ridged_offset", "Offset", 1.0, RIDGED),
        ridged_gain: owned("ridged_gain", "Gain", 2.0, RIDGED),
    }
}

/// Feeds the octave inputs to a call of `std-fractal-noise-octaves`.
fn feed_octaves<'g>(call: NodeBuilder<'g>, octaves: &OctaveInputs) -> NodeBuilder<'g> {
    call.input("type", octaves.selector.value())
        .input("detail", octaves.fractal.detail)
        .input("roughness", octaves.fractal.roughness)
        .input("lacunarity", octaves.fractal.lacunarity)
        .input("normalize", octaves.normalize)
        .input("hybrid_offset", octaves.hybrid_offset)
        .input("hybrid_gain", octaves.hybrid_gain)
        .input("hetero_offset", octaves.hetero_offset)
        .input("ridged_offset", octaves.ridged_offset)
        .input("ridged_gain", octaves.ridged_gain)
}

/// One octave of the fractal sum under the chosen type: the octave's noise,
/// and the values each type carries from octave to octave.
fn fractal_octave(
    g: &GroupBuilder<'_>,
    body: &RepeatBody<'_>,
    octave: &Octave,
    octaves: &OctaveInputs,
    options: &NoiseOptions,
    coordinates: Ref,
) {
    let shifted = g
        .call(OCTAVE_SHIFT)
        .input("vector", g.op("mul", (coordinates, octave.scale)))
        .input("octave", octave.iteration)
        .input("seed", options.seed)
        .add()
        .out("shifted");
    let n = g.wireless("Octave", noise(g, shifted, options)).first();
    let value = body.state("Value");
    let weight = body.state("Weight");
    let total = body.state("Total Amplitude");
    let first = compare(g, octave.iteration, g.constant(WgslType::I32, 0.0), "==");
    let fbm = g.frame(Frame::new("fBm").note("The octaves' sum."), || {
        g.op("add", (value, g.op("mul", (n, octave.amplitude))))
            .first()
    });
    let multifractal = g.frame(
        Frame::new("Multifractal")
            .note("Their product: each scales what's there by 1 ± its weight."),
        || {
            g.op(
                "mul",
                (
                    value,
                    g.op("add", (g.op("mul", (n, octave.amplitude)), 1.0)),
                ),
            )
            .first()
        },
    );
    let (hybrid_value, hybrid_weight) = g.frame(
        Frame::new("Hybrid Multifractal").note(
            "Each octave weighted by the ones before (times Gain, at most 1),\nstopping once that weight is negligible.",
        ),
        || {
            let active = compare(g, weight, 0.001, ">");
            let capped = g.op("min", (weight, 1.0)).first();
            let raised = g.op("add", (n, octaves.hybrid_offset)).first();
            let signal = g.op("mul", (raised, octave.power)).first();
            let next_value = g
                .op(
                    "switch",
                    (
                        active,
                        value,
                        g.op("add", (value, g.op("mul", (capped, raised, octave.amplitude)))),
                    ),
                )
                .first();
            let next_weight = g
                .op(
                    "switch",
                    (
                        active,
                        weight,
                        g.op("mul", (capped, octaves.hybrid_gain, signal)),
                    ),
                )
                .first();
            (next_value, next_weight)
        },
    );
    let hetero = g.frame(
        Frame::new("Heterogeneous Terrain").note(
            "The first octave plus Offset, then each octave scaled by the\nheight so far: smooth lowlands, rough peaks.",
        ),
        || {
            let raised = g.op("add", (n, octaves.hetero_offset)).first();
            g.op(
                "switch",
                (
                    first,
                    g.op("add", (value, g.op("mul", (raised, octave.amplitude, value)))),
                    raised,
                ),
            )
            .first()
        },
    );
    let (ridged_value, ridged_weight) = g.frame(
        Frame::new("Ridged Multifractal").note(
            "Ridges where the noise crosses zero, (Offset − |noise|)², each octave\nweighted by the last one's ridge (times Gain, within [0, 1]).",
        ),
        || {
            let ridge = g.op("sub", (octaves.ridged_offset, g.op("abs", n))).first();
            let squared = g.op("mul", (ridge, ridge)).first();
            let gained = g.op("mul", (weight, octaves.ridged_gain));
            let signal = g
                .op("mul", (squared, clamp_range(g, gained, 0.0, 1.0)))
                .first();
            let next_value = g
                .op(
                    "switch",
                    (
                        first,
                        g.op("add", (value, g.op("mul", (signal, octave.amplitude)))),
                        squared,
                    ),
                )
                .first();
            let next_weight = g.op("switch", (first, signal, squared)).first();
            (next_value, next_weight)
        },
    );
    let selector = &octaves.selector;
    body.set(
        "Value",
        pick(
            g,
            selector,
            vec![
                (FBM, fbm.into()),
                (MULTIFRACTAL, multifractal.into()),
                (HYBRID, hybrid_value.into()),
                (HETERO, hetero.into()),
                (RIDGED, ridged_value.into()),
            ],
        ),
    );
    body.set(
        "Weight",
        pick(
            g,
            selector,
            vec![
                (FBM, weight.into()),
                (MULTIFRACTAL, weight.into()),
                (HYBRID, hybrid_weight.into()),
                (HETERO, weight.into()),
                (RIDGED, ridged_weight.into()),
            ],
        ),
    );
    body.set("Total Amplitude", g.op("add", (total, octave.amplitude)));
}

fn fractal_noise_octaves_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let coordinates = g.input("coordinates", Input::new(&t).label("Coordinates"));
    let octaves = octave_inputs(g);
    let options = noise_options(g, &t);
    let selector = &octaves.selector;
    let sum = g.frame(
        Frame::new("Octaves").note(
            "OpenSimplex Noise again at each octave, its scale growing by\nLacunarity and its weight shrinking by Roughness, combined\nas the type does.",
        ),
        || {
            let items = vec![
                RepeatItem::new(
                    "Value",
                    WgslType::F32,
                    pick(
                        g,
                        selector,
                        vec![
                            (FBM, 0.0.into()),
                            (MULTIFRACTAL, 1.0.into()),
                            (HYBRID, 0.0.into()),
                            (HETERO, 0.0.into()),
                            (RIDGED, 0.0.into()),
                        ],
                    ),
                ),
                RepeatItem::new("Weight", WgslType::F32, 1.0),
                RepeatItem::new("Total Amplitude", WgslType::F32, 0.0),
            ];
            fractal(g, octaves.fractal, items, |body, octave| {
                fractal_octave(g, body, &octave, &octaves, &options, coordinates);
            })
        },
    );
    g.frame(
        Frame::new("Normalize")
            .note("fBm's sum into [0, 1] by the weights' total, when normalized."),
        || {
            let value = sum.get("Value");
            let scaled = g.op("div", (value, sum.get("Total Amplitude")));
            let normalized = g.op("add", (g.op("mul", (scaled, 0.5)), 0.5)).first();
            let fbm = g
                .op("switch", (octaves.normalize, value, normalized))
                .first();
            let result = pick(
                g,
                selector,
                vec![
                    (FBM, fbm.into()),
                    (MULTIFRACTAL, value.into()),
                    (HYBRID, value.into()),
                    (HETERO, value.into()),
                    (RIDGED, value.into()),
                ],
            );
            g.output("value", result, Output::new(WgslType::F32).label("Value"));
        },
    );
}

fn fractal_noise_body(g: &GroupBuilder<'_>) {
    let (t, vector) = coordinates_input(g);
    let scale = g.input(
        "scale",
        Input::new(WgslType::F32).label("Scale").default(5.0),
    );
    let octaves = octave_inputs(g);
    let distortion = g.input(
        "distortion",
        Input::new(WgslType::F32).label("Distortion").default(0.0),
    );
    let options = noise_options(g, &t);
    let distorted = g.frame(
        Frame::new("Coordinates").note(
            "Vector (the UV when unlinked) times Scale, moved by a noise\nof its own along each axis, times Distortion.",
        ),
        || {
            let coordinates = g
                .call(NOISE_COORDINATES)
                .input("vector", vector)
                .add()
                .out("coordinates");
            let scaled = g.op("mul", (coordinates, scale));
            let moved = g
                .call(NOISE_DISTORTION)
                .input("vector", scaled)
                .input("amount", distortion)
                .input("variant", options.variant.value())
                .input("orientation", options.orientation.value())
                .input("seed", options.seed)
                .add()
                .out("distorted");
            g.wireless("Coordinates", moved).first()
        },
    );
    let sum = |seed: Ref| -> Ref {
        let call = g
            .call(FRACTAL_NOISE_OCTAVES)
            .input("coordinates", distorted)
            .input("variant", options.variant.value())
            .input("orientation", options.orientation.value());
        feed_octaves(call, &octaves)
            .input("seed", seed)
            .add()
            .out("value")
    };
    let fac = g.frame(
        Frame::new("Fac").note("The octaves, combined as Type does."),
        || {
            let fac = sum(options.seed);
            g.output("fac", fac, Output::new(WgslType::F32).label("Fac"));
            fac
        },
    );
    g.frame(
        Frame::new("Color").note("Fac and two more fractal noises, seeded apart."),
        || {
            let [green, blue] = COLOR_SALTS.map(|salt| {
                let seed = g
                    .node("bitwise")
                    .args((options.seed, g.constant(WgslType::I32, salt)))
                    .setting("op", "^")
                    .add()
                    .first();
                sum(seed)
            });
            let color = g.vec(WgslType::Vec3f, (fac, green, blue));
            g.output("color", color, Output::new(WgslType::Vec3f).label("Color"));
        },
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            OPENSIMPLEX_NOISE,
            "OpenSimplex Noise",
            "texture",
            &["vector", "variant", "orientation", "seed"],
            &["value"],
            opensimplex_noise_body,
        )
        .aliases(&[
            "Simplex Noise",
            "OpenSimplex2",
            "SuperSimplex",
            "Gradient Noise",
            "Perlin",
        ])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            NOISE_DISTORTION,
            "Noise Distortion",
            "texture",
            &["vector", "amount", "variant", "orientation", "seed"],
            &["distorted"],
            noise_distortion_body,
        )
        .aliases(&["Domain Warp", "Warp", "Distort Coordinates"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            OCTAVE_SHIFT,
            "Octave Shift",
            "texture",
            &["vector", "octave", "seed"],
            &["shifted"],
            octave_shift_body,
        )
        .aliases(&["Octave Offset", "Decorrelate Octaves"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            FRACTAL_NOISE_OCTAVES,
            "Fractal Noise Octaves",
            "texture",
            &[
                "coordinates",
                "type",
                "detail",
                "roughness",
                "lacunarity",
                "normalize",
                "hybrid_offset",
                "hybrid_gain",
                "hetero_offset",
                "ridged_offset",
                "ridged_gain",
                "variant",
                "orientation",
                "seed",
            ],
            &["value"],
            fractal_noise_octaves_body,
        )
        .aliases(&["fBm Octaves", "Fractal Sum", "Multifractal"])
        .helper(&["OpenSimplex"]),
        GroupSpec::new(
            FRACTAL_NOISE,
            "Fractal Noise",
            "texture",
            &[
                "vector",
                "scale",
                "type",
                "detail",
                "roughness",
                "lacunarity",
                "normalize",
                "hybrid_offset",
                "hybrid_gain",
                "hetero_offset",
                "ridged_offset",
                "ridged_gain",
                "distortion",
                "variant",
                "orientation",
                "seed",
            ],
            &["fac", "color"],
            fractal_noise_body,
        )
        .aliases(&[
            "Noise Texture",
            "fBm",
            "FBM",
            "Multifractal",
            "Ridged Noise",
            "Simplex fBm",
            "Perlin Noise",
            "Clouds",
        ]),
    ]
}
