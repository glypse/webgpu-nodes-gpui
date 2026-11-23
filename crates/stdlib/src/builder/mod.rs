//! Describing groups in Rust: nodes are added to a `GroupBuilder`, which
//! checks them against the node registry and produces the group's document.

mod error;
mod group;
mod ids;
mod interface;
mod operand;
mod spec;

pub use error::{BuildError, BuildErrors, ErrorKind};
pub use group::{
    CustomWgsl, Frame, GroupBuilder, Node, NodeBuilder, RepeatBody, RepeatItem, Repeated,
    ResolvedGroup, Switch,
};
pub use ids::{handle_id, stable_id};
pub use interface::{EnumRef, Input, Output, TypeVar, Variant};
pub use operand::{Operand, Operands, Ref, vector};
pub use spec::{GroupSpec, InputIds};
