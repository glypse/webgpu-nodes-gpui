//! OpenSimplex2's variants and lattice orientations: how coordinates are
//! turned into lattice space, so the slices one looks at avoid its axes.

use shader_graph::types::{InputValue, WgslType};

use crate::builder::{
    EnumRef, Frame, GroupBuilder, GroupSpec, Input, Output, Ref, TypeVar, Variant,
};
use crate::groups::noise::coordinates::coordinates_type;

pub const SIMPLEX_ORIENTATION: &str = "std-opensimplex-orientation";

pub const FAST: i64 = 0;
pub const SMOOTH: i64 = 1;

const STANDARD: i64 = 0;
const Y_VERTICAL: i64 = 1;
const XY_PLANE: i64 = 2;
const XZ_PLANE: i64 = 3;
const XYZ_VOLUME: i64 = 4;
const XY_ZW_PLANES: i64 = 5;
const CLASSIC: i64 = 6;

const COORDINATE_TYPES: [WgslType; 3] = [WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f];

/// Variant, on a group whose coordinates are `t`: shown for 2D and up, as
/// 1D noise has a single lattice.
pub fn variant_input(g: &GroupBuilder<'_>, t: Option<&TypeVar>) -> EnumRef {
    let mut input = Input::enumeration(
        [Variant::new(FAST, "Fast"), Variant::new(SMOOTH, "Smooth")],
        SMOOTH,
    )
    .label("Variant");
    if let Some(t) = t {
        input = input.type_owner(t, &COORDINATE_TYPES);
    }
    g.enum_input("variant", input)
}

/// Orientation, on a group whose coordinates are `t`: offering those of
/// their dimension count, as the slices worth favouring differ (and 1D has
/// none, nor the input).
pub fn orientation_input(g: &GroupBuilder<'_>, t: &TypeVar) -> EnumRef {
    let orientations = [
        Variant::new(STANDARD, "Standard").types(&[WgslType::Vec2f]),
        Variant::new(Y_VERTICAL, "Y Vertical").types(&[WgslType::Vec2f]),
        Variant::new(XY_PLANE, "XY Plane").types(&[WgslType::Vec3f, WgslType::Vec4f]),
        Variant::new(XZ_PLANE, "XZ Plane").types(&[WgslType::Vec3f, WgslType::Vec4f]),
        Variant::new(XYZ_VOLUME, "XYZ Volume").types(&[WgslType::Vec4f]),
        Variant::new(XY_ZW_PLANES, "XY and ZW Planes").types(&[WgslType::Vec4f]),
        Variant::new(CLASSIC, "Classic").types(&[WgslType::Vec3f, WgslType::Vec4f]),
    ];
    g.enum_input(
        "orientation",
        Input::enumeration(orientations, XY_PLANE)
            .label("Orientation")
            .type_owner(t, &COORDINATE_TYPES)
            .variants_by(t),
    )
}

const SKEW_2D: f64 = 0.366025403784439;
const ROOT2OVER2: f64 = 0.7071067811865476;
const ROOT3OVER3: f64 = 0.577350269189626;
const ORTHOGONALIZER: f64 = -0.21132486540518713;

/// The 4D lattice maps of one variant, by orientation.
#[derive(Clone, Copy)]
enum HyperMap {
    XyPlane,
    XzPlane,
    XyzVolume,
    XyZwPlanes,
    Classic,
}

/// A linear map taking coordinates to a lattice's space. The OpenSimplex2
/// reference maps (OpenSimplex2.java, OpenSimplex2S.java) are the source.
#[derive(Clone, Copy)]
enum Map {
    PlaneStandard,
    PlaneImproveX,
    SpaceImproveXY,
    SpaceImproveXZ,
    SpaceClassic,
    Hyper { fast: bool, map: HyperMap },
}

fn at(v: &[f64], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

/// Z up the cubes' main diagonal, XY far from their faces.
fn space_improve_xy(x: f64, y: f64, z: f64) -> [f64; 3] {
    let s2 = (x + y) * ORTHOGONALIZER;
    let zz = z * ROOT3OVER3;
    [x + s2 + zz, y + s2 + zz, (x + y) * -ROOT3OVER3 + zz]
}

fn improve_xyz_xy(x: f64, y: f64, z: f64, w: f64, wscale: f64) -> [f64; 4] {
    let s2 = (x + y) * ORTHOGONALIZER;
    let zz = z * 0.2886751345948129;
    let ww = w * wscale;
    [
        x + zz + ww + s2,
        y + zz + ww + s2,
        (x + y) * -0.577350269189626 + zz + ww,
        z * -0.866025403784439 + ww,
    ]
}

fn improve_xyz(x: f64, y: f64, z: f64, w: f64, wscale: f64) -> [f64; 4] {
    let xyz = x + y + z;
    let ww = w * wscale;
    let s2 = xyz * (-1.0 / 6.0) + ww;
    [x + s2, y + s2, z + s2, -0.5 * xyz + ww]
}

fn improve_xy_zw(x: f64, y: f64, z: f64, w: f64, [a, b, c, d]: [f64; 4]) -> [f64; 4] {
    let s2 = (x + y) * a + (z + w) * b;
    let t2 = (z + w) * c + (x + y) * d;
    [x + s2, y + s2, z + t2, w + t2]
}

fn evaluate(map: Map, v: &[f64]) -> Vec<f64> {
    let (x, y, z) = (at(v, 0), at(v, 1), at(v, 2));
    match map {
        Map::PlaneStandard => {
            let s = SKEW_2D * (x + y);
            vec![x + s, y + s]
        }
        Map::PlaneImproveX => {
            let xx = x * ROOT2OVER2;
            let yy = y * ROOT2OVER2 * (1.0 + 2.0 * SKEW_2D);
            vec![yy + xx, yy - xx]
        }
        Map::SpaceImproveXY => space_improve_xy(x, y, z).to_vec(),
        Map::SpaceImproveXZ => {
            let [xr, zr, yr] = space_improve_xy(x, z, y);
            vec![xr, yr, zr]
        }
        Map::SpaceClassic => {
            let r = (2.0 / 3.0) * (x + y + z);
            vec![r - x, r - y, r - z]
        }
        Map::Hyper { fast, map } => {
            let w = at(v, 3);
            let wscale = if fast {
                0.2236067977499788
            } else {
                1.118033988749894
            };
            match map {
                HyperMap::XyPlane => improve_xyz_xy(x, y, z, w, wscale).to_vec(),
                HyperMap::XzPlane => {
                    let [xr, zr, yr, wr] = improve_xyz_xy(x, z, y, w, wscale);
                    vec![xr, yr, zr, wr]
                }
                HyperMap::XyzVolume => improve_xyz(x, y, z, w, wscale).to_vec(),
                HyperMap::XyZwPlanes => {
                    let coefficients = if fast {
                        [
                            -0.17827565795139938,
                            0.21562339328884284,
                            -0.4039497625802071,
                            -0.3751990830100753,
                        ]
                    } else {
                        [
                            -0.2852251398743488,
                            0.8389706547061143,
                            0.21939749883706436,
                            -0.48214856493302477,
                        ]
                    };
                    improve_xy_zw(x, y, z, w, coefficients).to_vec()
                }
                HyperMap::Classic => {
                    let skew = if fast {
                        -0.138196601125011
                    } else {
                        0.309016994374947
                    };
                    let s = skew * v.iter().fold(0.0, |sum, n| sum + n);
                    v.iter().map(|n| n + s).collect()
                }
            }
        }
    }
}

fn hyper(fast: bool) -> [(i64, Map); 5] {
    [
        (
            XY_PLANE,
            Map::Hyper {
                fast,
                map: HyperMap::XyPlane,
            },
        ),
        (
            XZ_PLANE,
            Map::Hyper {
                fast,
                map: HyperMap::XzPlane,
            },
        ),
        (
            XYZ_VOLUME,
            Map::Hyper {
                fast,
                map: HyperMap::XyzVolume,
            },
        ),
        (
            XY_ZW_PLANES,
            Map::Hyper {
                fast,
                map: HyperMap::XyZwPlanes,
            },
        ),
        (
            CLASSIC,
            Map::Hyper {
                fast,
                map: HyperMap::Classic,
            },
        ),
    ]
}

fn orientation_body(g: &GroupBuilder<'_>) {
    let t = coordinates_type(g);
    let vector = g.input("vector", Input::new(&t).label("Vector"));
    let variant = variant_input(g, Some(&t));
    let orientation = orientation_input(g, &t);
    // Split padded so every width finds its components on the same node.
    let padded = g
        .call(crate::groups::vector::PAD_VECTOR)
        .input("vector", vector)
        .add()
        .out("padded");
    let components = g.op("separate-vector", padded);
    let apply = |map: Map, n: usize| -> Ref {
        let columns: Vec<Ref> = (0..n)
            .map(|i| {
                let unit: Vec<f64> = (0..n).map(|j| f64::from(u8::from(i == j))).collect();
                let column = evaluate(map, &unit);
                let width = match n {
                    2 => WgslType::Vec2f,
                    3 => WgslType::Vec3f,
                    _ => WgslType::Vec4f,
                };
                let scale = g.constant(width, InputValue::vector(&column));
                let axis = ["x", "y", "z", "w"].get(i).copied().unwrap_or("x");
                g.op("mul", (components.out(axis), scale)).first()
            })
            .collect();
        g.op("add", columns).first()
    };
    let pick = |maps: &[(i64, Map)], n: usize| -> Ref {
        let mut switch = g.match_on(&orientation).partial();
        for (value, map) in maps {
            switch = switch.case(*value, apply(*map, n));
        }
        switch.add().first()
    };
    let plane = g.frame(
        Frame::new("2D").note(
            "Skewed onto the triangular lattice; Y Vertical turns it so Y runs\nalong a diagonal, for side views where Y is up.",
        ),
        || {
            pick(
                &[(STANDARD, Map::PlaneStandard), (Y_VERTICAL, Map::PlaneImproveX)],
                2,
            )
        },
    );
    let space = g.frame(
        Frame::new("3D").note(
            "Rotated, never skewed. XY Plane puts Z up the cubes' diagonal,\nso XY slices (Z as time or height) look their best; XZ Plane\ndoes it for Y; Classic is the diagonal rotation, more biased.",
        ),
        || {
            pick(
                &[
                    (XY_PLANE, Map::SpaceImproveXY),
                    (XZ_PLANE, Map::SpaceImproveXZ),
                    (CLASSIC, Map::SpaceClassic),
                ],
                3,
            )
        },
    );
    let four_d = g.frame(
        Frame::new("4D").note(
            "XYZ oriented as in 3D with W apart (XY Plane, XZ Plane, XYZ Volume),\nor XY and ZW as two planes (for noise(x, y, cos t, sin t));\nthe variants' lattices differ, and so do their maps.",
        ),
        || {
            g.match_on(&variant)
                .case(FAST, pick(&hyper(true), 4))
                .case(SMOOTH, pick(&hyper(false), 4))
                .add()
                .first()
        },
    );
    let lattice = g.width_switch(vector, (vector, plane, space, four_d));
    g.output(
        "lattice",
        lattice,
        Output::new(&t).label("Lattice Position"),
    );
}

pub fn all() -> Vec<GroupSpec> {
    vec![
        GroupSpec::new(
            SIMPLEX_ORIENTATION,
            "OpenSimplex Orientation",
            "texture",
            &["vector", "variant", "orientation"],
            &["lattice"],
            orientation_body,
        )
        .aliases(&[
            "Lattice Orientation",
            "Simplex Skew",
            "Lattice Space",
            "Slice Orientation",
        ])
        .helper(&["OpenSimplex"]),
    ]
}
