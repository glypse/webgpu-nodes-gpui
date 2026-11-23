use shader_graph::types::{
    ColorDisplay, EnumVariant, HandleType, InputOwner, InputValue, TypeOwner, WgslType,
};

use super::operand::Ref;

/// A type the sockets typed with it share, decided per group node among its types.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeVar {
    pub(super) id: String,
}

impl From<&TypeVar> for HandleType {
    fn from(v: &TypeVar) -> Self {
        HandleType::Variable {
            variable: v.id.clone(),
        }
    }
}

impl From<TypeVar> for HandleType {
    fn from(v: TypeVar) -> Self {
        HandleType::Variable { variable: v.id }
    }
}

/// A variant of an enum input: its stored value, label, and (on an enum
/// choosing by a type variable) the types it is offered for, all without.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub value: i64,
    pub label: String,
    pub types: Option<Vec<WgslType>>,
}

impl Variant {
    pub fn new(value: i64, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            types: None,
        }
    }

    pub fn types(mut self, types: &[WgslType]) -> Self {
        self.types = Some(types.to_vec());
        self
    }
}

impl<L: Into<String>> From<(i64, L)> for Variant {
    fn from((value, label): (i64, L)) -> Self {
        Variant::new(value, label)
    }
}

impl From<Variant> for EnumVariant {
    fn from(v: Variant) -> Self {
        EnumVariant {
            value: v.value,
            label: v.label,
            types: v.types,
        }
    }
}

/// An enum input of the group being described.
#[derive(Clone, Debug)]
pub struct EnumRef {
    pub(super) socket: Ref,
    pub(super) handle: String,
    pub(super) variants: Vec<EnumVariant>,
}

impl EnumRef {
    /// The input's socket, to link where its value is used.
    pub fn value(&self) -> Ref {
        self.socket
    }
}

/// A group input: its type and everything the editor shows of it.
#[derive(Clone, Debug)]
pub struct Input {
    pub(super) ty: HandleType,
    pub(super) label: Option<String>,
    pub(super) default: Option<InputValue>,
    pub(super) min: Option<f64>,
    pub(super) max: Option<f64>,
    pub(super) display: Option<ColorDisplay>,
    pub(super) compiled: bool,
    pub(super) owner: Option<InputOwner>,
    pub(super) type_owner: Option<TypeOwner>,
    pub(super) variants: Option<Vec<EnumVariant>>,
    pub(super) variants_by: Option<String>,
}

impl Input {
    pub fn new(ty: impl Into<HandleType>) -> Self {
        Self {
            ty: ty.into(),
            label: None,
            default: None,
            min: None,
            max: None,
            display: None,
            compiled: false,
            owner: None,
            type_owner: None,
            variants: None,
            variants_by: None,
        }
    }

    /// An enum input (a `u32`) choosing among `variants`, set to `initial` on new group nodes.
    pub fn enumeration<V: Into<Variant>>(
        variants: impl IntoIterator<Item = V>,
        initial: i64,
    ) -> Self {
        let mut input = Self::new(WgslType::U32);
        input.variants = Some(variants.into_iter().map(|v| v.into().into()).collect());
        input.default = Some(initial.into());
        input
    }

    /// An on/off input (a `bool`), edited as a checkbox.
    pub fn toggle(initial: bool) -> Self {
        let mut input = Self::new(WgslType::Bool);
        input.variants = Some(shader_graph::types::toggle_variants());
        input.default = Some(i64::from(initial).into());
        input
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// What group nodes hold until edited.
    pub fn default(mut self, value: impl Into<InputValue>) -> Self {
        self.default = Some(value.into());
        self
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    pub fn display(mut self, display: ColorDisplay) -> Self {
        self.display = Some(display);
        self
    }

    /// An integer input compiled in rather than read from a uniform.
    pub fn compiled(mut self) -> Self {
        self.compiled = true;
        self
    }

    /// Shown only while `variant` of the enum input `owner` is picked.
    pub fn owner(mut self, owner: &EnumRef, variant: i64) -> Self {
        self.owner = Some(InputOwner {
            enum_id: owner.handle.clone(),
            value: variant,
        });
        self
    }

    /// Shown only while `variable` has one of `types`.
    pub fn type_owner(mut self, variable: &TypeVar, types: &[WgslType]) -> Self {
        self.type_owner = Some(TypeOwner {
            variable: variable.id.clone(),
            types: types.to_vec(),
        });
        self
    }

    /// On an enum input: the variants it offers depend on `variable`'s type (see `Variant::types`).
    pub fn variants_by(mut self, variable: &TypeVar) -> Self {
        self.variants_by = Some(variable.id.clone());
        self
    }
}

impl From<WgslType> for Input {
    fn from(ty: WgslType) -> Self {
        Input::new(ty)
    }
}

impl From<&TypeVar> for Input {
    fn from(variable: &TypeVar) -> Self {
        Input::new(variable)
    }
}

impl From<HandleType> for Input {
    fn from(ty: HandleType) -> Self {
        Input::new(ty)
    }
}

/// A group output: its type and label.
#[derive(Clone, Debug)]
pub struct Output {
    pub(super) ty: HandleType,
    pub(super) label: Option<String>,
}

impl Output {
    pub fn new(ty: impl Into<HandleType>) -> Self {
        Self {
            ty: ty.into(),
            label: None,
        }
    }

    /// An output typed by whatever reaches it inside the group.
    pub fn inferred() -> Self {
        Self::new(HandleType::inferred())
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl From<WgslType> for Output {
    fn from(ty: WgslType) -> Self {
        Output::new(ty)
    }
}

impl From<&TypeVar> for Output {
    fn from(variable: &TypeVar) -> Self {
        Output::new(variable)
    }
}
