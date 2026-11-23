//! Noise coordinates: their type decides a noise's dimensions, one per
//! component, and every helper working on them is generic over it.

use shader_graph::types::WgslType;

use crate::builder::{GroupBuilder, GroupSpec, Input, Output, Ref, TypeVar};
use crate::groups::helpers::uv;

pub const NOISE_COORDINATES: &str = "std-noise-coordinates";

/// The types noise coordinates take, one per dimension count: 2D first, the
/// type they have when nothing decides it (unlinked, they're the UV).
const COORDINATE_TYPES: [WgslType; 4] = [
    WgslType::Vec2f,
    WgslType::Vec3f,
    WgslType::Vec4f,
    WgslType::F32,
];

/// A group's coordinates type: its sockets taking it follow the noise's dimensions.
pub fn coordinates_type(g: &GroupBuilder<'_>) -> TypeVar {
    g.type_variable("t", "T", &COORDINATE_TYPES)
}

/// A noise's coordinates input, keyed as `std-noise-coordinates` takes it so
/// it can be passed on, and its type.
pub fn coordinates_input(g: &GroupBuilder<'_>) -> (TypeVar, Ref) {
    let t = coordinates_type(g);
    let vector = g.input("vector", Input::new(&t).label("Vector"));
    (t, vector)
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            NOISE_COORDINATES,
            "Noise Coordinates",
            "texture",
            &["vector"],
            &["coordinates"],
            |g| {
                let (t, vector) = coordinates_input(g);
                let coordinates = g.op("fallback", (vector, uv(g)));
                g.output(
                    "coordinates",
                    coordinates,
                    Output::new(&t).label("Coordinates"),
                );
            },
        )
        .aliases(&["Texture Coordinates", "Noise Vector"])
        .helper(&[]),
    ]
}
