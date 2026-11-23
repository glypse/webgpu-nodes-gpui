use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::values::{ScalarKind, WgslType};

/// One component of an input value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    Bool(bool),
    Num(f64),
}

impl ScalarValue {
    pub fn as_f64(self) -> f64 {
        match self {
            ScalarValue::Bool(b) => f64::from(u8::from(b)),
            ScalarValue::Num(n) => n,
        }
    }
}

/// Per-component values of a vector input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorValue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<ScalarValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<ScalarValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<ScalarValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<ScalarValue>,
}

impl VectorValue {
    pub fn component(&self, index: usize) -> Option<ScalarValue> {
        [self.x, self.y, self.z, self.w].get(index).copied().flatten()
    }

    pub fn set(&mut self, index: usize, value: ScalarValue) {
        match index {
            0 => self.x = Some(value),
            1 => self.y = Some(value),
            2 => self.z = Some(value),
            _ => self.w = Some(value),
        }
    }
}

/// The value an input holds while unlinked: a scalar (which fills every
/// component of a vector) or per-component values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    Scalar(ScalarValue),
    Vector(VectorValue),
}

impl From<f64> for InputValue {
    fn from(n: f64) -> Self {
        InputValue::Scalar(ScalarValue::Num(n))
    }
}

impl From<i32> for InputValue {
    fn from(n: i32) -> Self {
        InputValue::Scalar(ScalarValue::Num(f64::from(n)))
    }
}

impl From<i64> for InputValue {
    fn from(n: i64) -> Self {
        InputValue::Scalar(ScalarValue::Num(n as f64))
    }
}

impl From<bool> for InputValue {
    fn from(b: bool) -> Self {
        InputValue::Scalar(ScalarValue::Bool(b))
    }
}

impl InputValue {
    pub fn vector(components: &[f64]) -> Self {
        let mut v = VectorValue::default();
        for (i, c) in components.iter().enumerate() {
            v.set(i, ScalarValue::Num(*c));
        }
        InputValue::Vector(v)
    }

    /// One component as a number; a scalar fills all of them.
    pub fn component(&self, index: usize) -> f64 {
        match self {
            InputValue::Scalar(s) => s.as_f64(),
            InputValue::Vector(v) => v.component(index).map_or(0.0, ScalarValue::as_f64),
        }
    }

    /// The value as a stored enum / count: its first component, truncated.
    pub fn as_i64(&self) -> i64 {
        self.component(0).trunc() as i64
    }

    /// `stored` of type `ty` with one component set.
    pub fn with_component(
        stored: Option<&InputValue>,
        ty: WgslType,
        index: usize,
        value: f64,
    ) -> InputValue {
        if ty.components() == 1 {
            return value.into();
        }
        let mut vector = match stored {
            Some(InputValue::Vector(v)) => *v,
            other => {
                let fill = other.map_or(0.0, |v| v.component(0));
                let mut v = VectorValue::default();
                for i in 0..ty.components() {
                    v.set(i, ScalarValue::Num(fill));
                }
                v
            }
        };
        vector.set(index, ScalarValue::Num(value));
        InputValue::Vector(vector)
    }
}

/// How a group input of a colour type is edited on group nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorDisplay {
    Swatch,
    Wheel,
}

/// One case of an enum. `value` is what's stored and compiled: it stays the
/// same when cases are renamed or reordered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnumVariant {
    #[serde(deserialize_with = "whole_number")]
    pub value: i64,
    pub label: String,
    /// On an enum input with `variants_by`: the types of that variable the
    /// variant is offered for, all of them without.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<WgslType>>,
}

impl EnumVariant {
    pub fn new(value: i64, label: impl Into<String>) -> Self {
        Self { value, label: label.into(), types: None }
    }
}

/// An integer that older files stored as a float (`2.0`).
fn whole_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Int(i64),
        Float(f64),
    }
    Ok(match Number::deserialize(d)? {
        Number::Int(n) => n,
        Number::Float(n) => n as i64,
    })
}

/// The variants of a checkbox.
pub fn toggle_variants() -> Vec<EnumVariant> {
    vec![EnumVariant::new(0, "Off"), EnumVariant::new(1, "On")]
}

/// A variant of a group's enum input, by the input's handle id and the variant's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputOwner {
    pub enum_id: String,
    #[serde(deserialize_with = "whole_number")]
    pub value: i64,
}

/// Some of the types of a group's type variable, by the variable's id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeOwner {
    pub variable: String,
    pub types: Vec<WgslType>,
}

/// A type a group's sockets share: each group node gives it one of `types`,
/// inferred from what's linked to those sockets (or picked on the node).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeVariable {
    pub id: String,
    pub name: String,
    /// The types it may take; the first one when nothing decides.
    pub types: Vec<WgslType>,
}

/// What a group socket carries: a fixed type, the type of one of the group's
/// type variables, or (on an output) whatever reaches it inside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HandleType {
    Fixed(WgslType),
    Variable { variable: String },
    Inferred { inferred: bool },
}

impl From<WgslType> for HandleType {
    fn from(t: WgslType) -> Self {
        HandleType::Fixed(t)
    }
}

impl HandleType {
    pub fn inferred() -> Self {
        HandleType::Inferred { inferred: true }
    }

    pub fn is_inferred(&self) -> bool {
        matches!(self, HandleType::Inferred { .. })
    }

    pub fn variable(&self) -> Option<&str> {
        match self {
            HandleType::Variable { variable } => Some(variable),
            _ => None,
        }
    }

    pub fn fixed(&self) -> Option<WgslType> {
        match self {
            HandleType::Fixed(t) => Some(*t),
            _ => None,
        }
    }
}

/// The types a group's variables take for one group node, by variable id.
pub type TypeBindings = BTreeMap<String, WgslType>;

/// The type a socket has with `bindings`; an inferred one is `None` (it's resolved).
pub fn bound_type(
    ty: &HandleType,
    variables: &[TypeVariable],
    bindings: &TypeBindings,
) -> Option<WgslType> {
    match ty {
        HandleType::Fixed(t) => Some(*t),
        HandleType::Inferred { .. } => None,
        HandleType::Variable { variable } => Some(
            bindings
                .get(variable)
                .copied()
                .or_else(|| variables.iter().find(|v| &v.id == variable)?.types.first().copied())
                .unwrap_or(WgslType::F32),
        ),
    }
}

/// A socket of a Custom WGSL node, a group's interface or a zone's items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handle<T> {
    /// Stable: what edges' handles reference.
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: T,
    /// On a group input: makes it an enum input choosing among these (a u32),
    /// or a bool edited as a checkbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<EnumVariant>>,
    /// An integer group input whose value is compiled in, as an enum's is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiled: Option<bool>,
    /// The variant of one of the group's enum inputs it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<InputOwner>,
    /// The types of one of the group's type variables it's shown for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_owner: Option<TypeOwner>,
    /// On an enum input: the id of the type variable deciding which variants are offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<ColorDisplay>,
    /// On a group output: added to preview a node inside the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<bool>,
    /// A zone item's texture format and size mode; kept as saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

impl<T> Handle<T> {
    pub fn new(id: impl Into<String>, label: impl Into<String>, ty: T) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            ty,
            variants: None,
            compiled: None,
            owner: None,
            type_owner: None,
            variants_by: None,
            min: None,
            max: None,
            display: None,
            viewer: None,
            format: None,
            size: None,
        }
    }
}

/// A Custom WGSL node's socket.
pub type HandleDef = Handle<WgslType>;
/// A socket of a group's boundary node: its type may be decided per group node.
pub type GroupHandleDef = Handle<HandleType>;
/// A zone's state item: a fixed type; in a group, a variable's; or (inferred)
/// the type of its initial value.
pub type ZoneItem = Handle<HandleType>;

impl GroupHandleDef {
    /// Whether an integer input's value can be compiled in.
    pub fn can_compile(&self) -> bool {
        self.variants.is_none()
            && matches!(self.ty, HandleType::Fixed(WgslType::I32 | WgslType::U32))
    }
}

/// A Custom WGSL node's sockets; their labels name them in its code.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CustomSockets {
    pub inputs: Vec<HandleDef>,
    pub outputs: Vec<HandleDef>,
}

/// Everything a node stores. Which fields apply depends on the node type.
/// Fields this crate has no typed field for (a Frame's tint and note) are
/// kept in `extra` so saved projects round-trip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeData {
    /// The name the user gave the node, shown instead of its default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Values of the node's unlinked inputs, by input id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, InputValue>,
    /// Enum settings shown on the node, by setting key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// Types the user forced, by type family key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub types: BTreeMap<String, WgslType>,
    /// A zone output's state items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ZoneItem>>,
    /// A zone input's partner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_output: Option<String>,
    /// A Repeat Input's iteration safety cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    /// How many cases an Index Switch picks among.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<u32>,
    /// A Custom WGSL node's function body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// A Custom WGSL node's sockets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sockets: Option<CustomSockets>,
    /// A reroute's links out are drawn as tags on the inputs they reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_links: Option<bool>,
    /// A Curve node's control points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Vec<crate::curve::CurvePoint>>,
    /// A Color Ramp's stops; their colours are its inputs' values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<Vec<crate::ramp::RampStop>>,
    /// The project asset an Image or Video node reads, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// How a Video node plays its asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<crate::assets::VideoPlayback>,
    /// Only the node's linked sockets are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_unlinked: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl NodeData {
    pub fn with_values<I, V>(values: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, V)>,
        V: Into<InputValue>,
    {
        Self {
            values: values.into_iter().map(|(k, v)| (k.to_owned(), v.into())).collect(),
            ..Self::default()
        }
    }

    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.insert(key.to_owned(), value.to_owned());
        self
    }

    /// `self` with `defaults` filled in where it has no value of its own.
    pub fn with_defaults(&self, defaults: &NodeData) -> NodeData {
        let mut data = defaults.clone();
        data.label.clone_from(&self.label);
        data.items = self.items.clone().or(data.items);
        data.paired_output = self.paired_output.clone().or(data.paired_output);
        data.max_iterations = self.max_iterations.or(data.max_iterations);
        data.cases = self.cases.or(data.cases);
        data.code = self.code.clone().or(data.code);
        data.sockets = self.sockets.clone().or(data.sockets);
        data.hide_links = self.hide_links.or(data.hide_links);
        data.hide_unlinked = self.hide_unlinked.or(data.hide_unlinked);
        data.asset = self.asset.clone().or(data.asset);
        data.playback = self.playback.or(data.playback);
        data.curve = self.curve.clone().or(data.curve);
        data.ramp = self.ramp.clone().or(data.ramp);
        data.values.extend(self.values.clone());
        data.settings.extend(self.settings.clone());
        data.types.extend(self.types.clone());
        data.extra.extend(self.extra.clone());
        data
    }
}

/// A stored enum value as one of `variants`' values: the first when it isn't one.
pub fn enum_value(stored: Option<&InputValue>, variants: &[EnumVariant]) -> Option<i64> {
    let value = stored.map_or(0, InputValue::as_i64);
    variants
        .iter()
        .find(|v| v.value == value)
        .or_else(|| variants.first())
        .map(|v| v.value)
}

pub fn scalar_number_of(kind: ScalarKind, value: Option<f64>) -> f64 {
    let raw = value.filter(|v| v.is_finite()).unwrap_or(0.0);
    match kind {
        ScalarKind::Bool => f64::from(u8::from(raw != 0.0)),
        ScalarKind::F32 => raw,
        ScalarKind::I32 => raw.trunc().clamp(f64::from(i32::MIN), f64::from(i32::MAX)),
        ScalarKind::U32 => raw.trunc().clamp(0.0, f64::from(u32::MAX)),
    }
}
