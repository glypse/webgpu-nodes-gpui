//! WGSL value types: a scalar kind and a component count.
//!
//! Scalar kinds always convert implicitly (int to float is exact, float to int
//! truncates toward zero, number to bool is `!= 0`); component counts may only
//! grow when linking, and the missing components are zero.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScalarKind {
    Bool,
    U32,
    I32,
    F32,
}

impl ScalarKind {

    /// The scalar type's WGSL spelling.
    pub fn name(self) -> &'static str {
        match self {
            ScalarKind::F32 => "f32",
            ScalarKind::I32 => "i32",
            ScalarKind::U32 => "u32",
            ScalarKind::Bool => "bool",
        }
    }

    /// Orders kinds for picking a common one: float absorbs ints, ints absorb bools.
    fn rank(self) -> u8 {
        self as u8
    }

    fn zero(self) -> &'static str {
        match self {
            ScalarKind::F32 => "0.0",
            ScalarKind::I32 => "0i",
            ScalarKind::U32 => "0u",
            ScalarKind::Bool => "false",
        }
    }
}

macro_rules! wgsl_types {
    ($($variant:ident = $name:literal, $kind:ident, $n:literal;)*) => {
        /// A concrete WGSL value type. `F32` comes first: the default of untyped values.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum WgslType {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl WgslType {
            pub const ALL: [WgslType; 16] = [$(WgslType::$variant,)*];

            /// The type's identifier, as saved files spell it.
            pub const fn id(self) -> &'static str {
                match self { $(WgslType::$variant => $name,)* }
            }

            pub const fn kind(self) -> ScalarKind {
                match self { $(WgslType::$variant => ScalarKind::$kind,)* }
            }

            pub const fn components(self) -> usize {
                match self { $(WgslType::$variant => $n,)* }
            }

            pub fn from_id(id: &str) -> Option<WgslType> {
                match id { $($name => Some(WgslType::$variant),)* _ => None }
            }
        }
    };
}

wgsl_types! {
    F32 = "f32", F32, 1;
    Vec2f = "vec2f", F32, 2;
    Vec3f = "vec3f", F32, 3;
    Vec4f = "vec4f", F32, 4;
    I32 = "i32", I32, 1;
    Vec2i = "vec2i", I32, 2;
    Vec3i = "vec3i", I32, 3;
    Vec4i = "vec4i", I32, 4;
    U32 = "u32", U32, 1;
    Vec2u = "vec2u", U32, 2;
    Vec3u = "vec3u", U32, 3;
    Vec4u = "vec4u", U32, 4;
    Bool = "bool", Bool, 1;
    Vec2b = "vec2b", Bool, 2;
    Vec3b = "vec3b", Bool, 3;
    Vec4b = "vec4b", Bool, 4;
}

impl fmt::Display for WgslType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

use WgslType::*;

pub const FLOAT_TYPES: &[WgslType] = &[F32, Vec2f, Vec3f, Vec4f];
pub const INT_TYPES: &[WgslType] = &[I32, Vec2i, Vec3i, Vec4i];
pub const UINT_TYPES: &[WgslType] = &[U32, Vec2u, Vec3u, Vec4u];
pub const BOOL_TYPES: &[WgslType] = &[Bool, Vec2b, Vec3b, Vec4b];
pub const FLOAT_VEC_TYPES: &[WgslType] = &[Vec2f, Vec3f, Vec4f];
pub const VEC_TYPES: &[WgslType] = &[
    Vec2f, Vec3f, Vec4f, Vec2i, Vec3i, Vec4i, Vec2u, Vec3u, Vec4u, Vec2b, Vec3b, Vec4b,
];
/// Types supporting arithmetic. Float first so unconnected inputs default to f32.
pub const NUMERIC_TYPES: &[WgslType] = &[
    F32, Vec2f, Vec3f, Vec4f, I32, Vec2i, Vec3i, Vec4i, U32, Vec2u, Vec3u, Vec4u,
];
pub const SIGNED_TYPES: &[WgslType] = &[F32, Vec2f, Vec3f, Vec4f, I32, Vec2i, Vec3i, Vec4i];
pub const INTEGER_TYPES: &[WgslType] = &[I32, Vec2i, Vec3i, Vec4i, U32, Vec2u, Vec3u, Vec4u];
pub const SCALAR_TYPES: &[WgslType] = &[F32, I32, U32, Bool];

/// Named sets of types, e.g. for what a group's type variable may take.
pub struct TypeSet {
    pub id: &'static str,
    pub label: &'static str,
    pub types: &'static [WgslType],
}

pub const TYPE_SETS: &[TypeSet] = &[
    TypeSet { id: "float", label: "Float", types: FLOAT_TYPES },
    TypeSet { id: "int", label: "Integer", types: INT_TYPES },
    TypeSet { id: "uint", label: "Unsigned", types: UINT_TYPES },
    TypeSet { id: "bool", label: "Boolean", types: BOOL_TYPES },
    TypeSet { id: "signed", label: "Signed number", types: SIGNED_TYPES },
    TypeSet { id: "number", label: "Any number", types: NUMERIC_TYPES },
    TypeSet { id: "any", label: "Any", types: &WgslType::ALL },
];

/// The named set `types` make up, in whatever order.
pub fn type_set_of(types: &[WgslType]) -> Option<&'static TypeSet> {
    TYPE_SETS
        .iter()
        .find(|set| set.types.len() == types.len() && set.types.iter().all(|t| types.contains(t)))
}

/// `types` with `default`, when among them, first: the one taken when nothing decides.
pub fn with_default_type(types: &[WgslType], default: Option<WgslType>) -> Vec<WgslType> {
    match default {
        Some(d) if types.contains(&d) => std::iter::once(d)
            .chain(types.iter().copied().filter(|t| *t != d))
            .collect(),
        _ => types.to_vec(),
    }
}

/// The type with the given kind and component count (clamped to 1..=4).
pub fn make_type(kind: ScalarKind, components: usize) -> WgslType {
    let n = components.clamp(1, 4);
    WgslType::ALL
        .into_iter()
        .find(|t| t.kind() == kind && t.components() == n)
        .expect("every kind has every width")
}

impl WgslType {
    /// The spelling in WGSL source. Bool vectors have no predeclared alias.
    pub fn wgsl_name(self) -> String {
        if self.kind() == ScalarKind::Bool && self.components() > 1 {
            format!("vec{}<bool>", self.components())
        } else {
            self.id().to_owned()
        }
    }

    /// The zero value, e.g. `vec3f()`.
    pub fn zero(self) -> String {
        format!("{}()", self.wgsl_name())
    }

    pub fn is_scalar(self) -> bool {
        self.components() == 1
    }
}

/// Whether a value of `source` type may be linked to an input of `target` type.
pub fn is_connection_compatible(source: WgslType, target: WgslType) -> bool {
    source.components() <= target.components()
}

const SWIZZLE: [&str; 4] = ["x", "y", "z", "w"];

/// An expression converting `expr` (of `source` type) for use where `target`
/// is expected: kind changes use constructors, scalars splat, narrower
/// vectors are padded with zeros and wider ones truncated by a swizzle.
pub fn convert_var(expr: &str, source: WgslType, target: WgslType) -> String {
    if source == target {
        return expr.to_owned();
    }
    let (from_kind, from_n) = (source.kind(), source.components());
    let (to_kind, to_n) = (target.kind(), target.components());
    let target_name = target.wgsl_name();

    if from_n == 1 {
        let scalar = if from_kind == to_kind {
            expr.to_owned()
        } else {
            format!("{}({expr})", to_kind.name())
        };
        return if to_n == 1 { scalar } else { format!("{target_name}({scalar})") };
    }

    if from_n > to_n {
        let narrowed = format!("{expr}.{}", SWIZZLE[..to_n].concat());
        return convert_var(&narrowed, make_type(from_kind, to_n), target);
    }

    let same_width = make_type(to_kind, from_n);
    let converted = if from_kind == to_kind {
        expr.to_owned()
    } else {
        format!("{}({expr})", same_width.wgsl_name())
    };
    if from_n == to_n {
        return converted;
    }
    let mut parts = vec![converted];
    parts.extend((0..to_n - from_n).map(|_| to_kind.zero().to_owned()));
    format!("{target_name}({})", parts.join(", "))
}

/// The smallest type able to hold both: the widest kind, the most components.
pub fn join_types(a: WgslType, b: WgslType) -> WgslType {
    let kind = if a.kind().rank() >= b.kind().rank() { a.kind() } else { b.kind() };
    make_type(kind, a.components().max(b.components()))
}

/// Fit a wanted type into a family of allowed types: keeps the wanted kind
/// when the family has it, then picks the smallest width able to hold it.
pub fn fit_type(wanted: WgslType, allowed: &[WgslType]) -> WgslType {
    if allowed.is_empty() || allowed.contains(&wanted) {
        return wanted;
    }
    let fallback = allowed[0].kind();
    let kind = if allowed.iter().any(|t| t.kind() == wanted.kind()) { wanted.kind() } else { fallback };
    let mut same_kind: Vec<_> = allowed.iter().copied().filter(|t| t.kind() == kind).collect();
    same_kind.sort_by_key(|t| t.components());
    same_kind
        .iter()
        .copied()
        .find(|t| t.components() >= wanted.components())
        .or(same_kind.last().copied())
        .unwrap_or(allowed[0])
}

/// The type a family resolves to given its linked source types; the family's
/// first type when there are none.
pub fn highest_type(candidates: &[WgslType], allowed: &[WgslType]) -> WgslType {
    let fallback = allowed.first().copied().unwrap_or(F32);
    match candidates.iter().copied().reduce(join_types) {
        None => fallback,
        Some(joined) if allowed.is_empty() => joined,
        Some(joined) => fit_type(joined, allowed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_types() {
        assert_eq!(convert_var("x", F32, Vec3f), "vec3f(x)");
        assert_eq!(convert_var("x", I32, F32), "f32(x)");
        assert_eq!(convert_var("v", Vec2f, Vec4f), "vec4f(v, 0.0, 0.0)");
        assert_eq!(convert_var("v", Vec2i, Vec3f), "vec3f(vec2f(v), 0.0)");
        assert_eq!(convert_var("v", Vec4f, F32), "v.x");
        assert_eq!(convert_var("b", Bool, Vec2b), "vec2<bool>(b)");
    }

    #[test]
    fn joins_and_fits() {
        assert_eq!(join_types(I32, Vec2f), Vec2f);
        assert_eq!(join_types(Bool, U32), U32);
        assert_eq!(fit_type(Vec2i, FLOAT_TYPES), Vec2f);
        assert_eq!(fit_type(Vec4f, &[Vec3f]), Vec3f);
        assert_eq!(highest_type(&[], NUMERIC_TYPES), F32);
        assert_eq!(highest_type(&[I32, I32], NUMERIC_TYPES), I32);
    }

    #[test]
    fn type_ids_round_trip() {
        for t in WgslType::ALL {
            assert_eq!(WgslType::from_id(t.id()), Some(t));
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{}\"", t.id()));
        }
    }
}
