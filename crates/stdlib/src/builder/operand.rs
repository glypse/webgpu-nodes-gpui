use shader_graph::types::InputValue;

use super::group::Node;
use super::interface::EnumRef;

/// An output socket of a node in the group being described, or one of its
/// inputs. Only the builder that made it can resolve it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref {
    pub(super) node: u32,
    pub(super) handle: u32,
}

impl Ref {
    /// Stands in for a socket whose creation failed; the error is already recorded.
    pub(super) const INVALID: Ref = Ref {
        node: u32::MAX - 1,
        handle: 0,
    };
    /// The node of a group input's socket: the Group Input node of wherever it's used.
    pub(super) const GROUP_INPUT_NODE: u32 = u32::MAX;
}

/// What an input receives: a link from a socket, or a value stored on the node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operand {
    Ref(Ref),
    Value(InputValue),
}

impl From<Ref> for Operand {
    fn from(r: Ref) -> Self {
        Operand::Ref(r)
    }
}

impl From<Node<'_>> for Operand {
    fn from(node: Node<'_>) -> Self {
        node.operand()
    }
}

impl From<&EnumRef> for Operand {
    fn from(e: &EnumRef) -> Self {
        Operand::Ref(e.value())
    }
}

impl From<InputValue> for Operand {
    fn from(v: InputValue) -> Self {
        Operand::Value(v)
    }
}

impl From<f64> for Operand {
    fn from(v: f64) -> Self {
        Operand::Value(v.into())
    }
}

impl From<i32> for Operand {
    fn from(v: i32) -> Self {
        Operand::Value(v.into())
    }
}

impl From<bool> for Operand {
    fn from(v: bool) -> Self {
        Operand::Value(v.into())
    }
}

/// A value of several components, as a vector input stores it.
pub fn vector(components: &[f64]) -> Operand {
    Operand::Value(InputValue::vector(components))
}

/// Operands in order: a single one, a tuple or array of them, or a `Vec`.
pub trait Operands {
    fn into_operands(self) -> Vec<Operand>;
}

impl<T: Into<Operand>> Operands for T {
    fn into_operands(self) -> Vec<Operand> {
        vec![self.into()]
    }
}

impl<T: Into<Operand>, const N: usize> Operands for [T; N] {
    fn into_operands(self) -> Vec<Operand> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Operand>> Operands for Vec<T> {
    fn into_operands(self) -> Vec<Operand> {
        self.into_iter().map(Into::into).collect()
    }
}

macro_rules! tuple_operands {
    ($($name:ident),+) => {
        impl<$($name: Into<Operand>),+> Operands for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_operands(self) -> Vec<Operand> {
                let ($($name,)+) = self;
                vec![$($name.into()),+]
            }
        }
    };
}

tuple_operands!(A, B);
tuple_operands!(A, B, C);
tuple_operands!(A, B, C, D);
tuple_operands!(A, B, C, D, E);
tuple_operands!(A, B, C, D, E, F);
tuple_operands!(A, B, C, D, E, F, G);
tuple_operands!(A, B, C, D, E, F, G, H);
