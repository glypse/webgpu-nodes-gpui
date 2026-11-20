use serde::{Deserialize, Serialize};

use crate::graph::GroupId;

/// What a node computes. Every socket carries an `f32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Uv,
    Time,
    Value,
    Math(MathOp),
    Output,
    Group(GroupId),
    GroupInput,
    GroupOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MathOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
    Min,
    Max,
    Sin,
    Cos,
    Tan,
    Abs,
    Floor,
    Fract,
    Sqrt,
    Step,
    Mix,
    Clamp,
    Smoothstep,
}

impl MathOp {
    pub const ALL: [MathOp; 19] = [
        MathOp::Add,
        MathOp::Subtract,
        MathOp::Multiply,
        MathOp::Divide,
        MathOp::Modulo,
        MathOp::Power,
        MathOp::Min,
        MathOp::Max,
        MathOp::Sin,
        MathOp::Cos,
        MathOp::Tan,
        MathOp::Abs,
        MathOp::Floor,
        MathOp::Fract,
        MathOp::Sqrt,
        MathOp::Step,
        MathOp::Mix,
        MathOp::Clamp,
        MathOp::Smoothstep,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MathOp::Add => "Add",
            MathOp::Subtract => "Subtract",
            MathOp::Multiply => "Multiply",
            MathOp::Divide => "Divide",
            MathOp::Modulo => "Modulo",
            MathOp::Power => "Power",
            MathOp::Min => "Minimum",
            MathOp::Max => "Maximum",
            MathOp::Sin => "Sine",
            MathOp::Cos => "Cosine",
            MathOp::Tan => "Tangent",
            MathOp::Abs => "Absolute",
            MathOp::Floor => "Floor",
            MathOp::Fract => "Fract",
            MathOp::Sqrt => "Square Root",
            MathOp::Step => "Step",
            MathOp::Mix => "Mix",
            MathOp::Clamp => "Clamp",
            MathOp::Smoothstep => "Smoothstep",
        }
    }

    /// Input labels with the values they hold until linked.
    pub fn inputs(self) -> &'static [(&'static str, f32)] {
        match self {
            MathOp::Add | MathOp::Subtract => &[("a", 0.0), ("b", 0.0)],
            MathOp::Multiply | MathOp::Divide | MathOp::Modulo => &[("a", 1.0), ("b", 1.0)],
            MathOp::Power => &[("base", 1.0), ("exponent", 1.0)],
            MathOp::Min | MathOp::Max => &[("a", 0.0), ("b", 0.0)],
            MathOp::Sin
            | MathOp::Cos
            | MathOp::Tan
            | MathOp::Abs
            | MathOp::Floor
            | MathOp::Fract
            | MathOp::Sqrt => &[("x", 0.0)],
            MathOp::Step => &[("edge", 0.5), ("x", 0.0)],
            MathOp::Mix => &[("a", 0.0), ("b", 1.0), ("t", 0.5)],
            MathOp::Clamp => &[("x", 0.0), ("low", 0.0), ("high", 1.0)],
            MathOp::Smoothstep => &[("low", 0.0), ("high", 1.0), ("x", 0.5)],
        }
    }

    /// The WGSL expression for the given argument expressions.
    pub fn wgsl(self, args: &[String]) -> String {
        let a = |i: usize| args[i].as_str();
        match self {
            MathOp::Add => format!("{} + {}", a(0), a(1)),
            MathOp::Subtract => format!("{} - {}", a(0), a(1)),
            MathOp::Multiply => format!("{} * {}", a(0), a(1)),
            // Zero instead of inf/NaN, so one bad value doesn't blank the preview.
            MathOp::Divide => format!("select({0} / {1}, 0.0, {1} == 0.0)", a(0), a(1)),
            MathOp::Modulo => format!("select({0} % {1}, 0.0, {1} == 0.0)", a(0), a(1)),
            MathOp::Power => format!("pow({}, {})", a(0), a(1)),
            MathOp::Min => format!("min({}, {})", a(0), a(1)),
            MathOp::Max => format!("max({}, {})", a(0), a(1)),
            MathOp::Sin => format!("sin({})", a(0)),
            MathOp::Cos => format!("cos({})", a(0)),
            MathOp::Tan => format!("tan({})", a(0)),
            MathOp::Abs => format!("abs({})", a(0)),
            MathOp::Floor => format!("floor({})", a(0)),
            MathOp::Fract => format!("fract({})", a(0)),
            MathOp::Sqrt => format!("sqrt({})", a(0)),
            MathOp::Step => format!("step({}, {})", a(0), a(1)),
            MathOp::Mix => format!("mix({}, {}, {})", a(0), a(1), a(2)),
            MathOp::Clamp => format!("clamp({}, {}, {})", a(0), a(1), a(2)),
            MathOp::Smoothstep => format!("smoothstep({}, {}, {})", a(0), a(1), a(2)),
        }
    }
}

/// A socket's label and, for inputs, the value it holds until linked.
#[derive(Clone, Debug, PartialEq)]
pub struct Socket {
    pub label: String,
    pub default: f32,
}

impl Socket {
    fn new(label: impl Into<String>, default: f32) -> Self {
        Self {
            label: label.into(),
            default,
        }
    }
}

/// A node's sockets, which for group nodes depend on the group's interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signature {
    pub title: String,
    pub inputs: Vec<Socket>,
    pub outputs: Vec<Socket>,
}

/// The interface a group exposes to the nodes that use it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Interface {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

impl NodeKind {
    /// Kinds offered when adding a node; groups are listed separately.
    pub fn catalog() -> impl Iterator<Item = NodeKind> {
        [NodeKind::Uv, NodeKind::Time, NodeKind::Value]
            .into_iter()
            .chain(MathOp::ALL.into_iter().map(NodeKind::Math))
    }

    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Uv => "UV",
            NodeKind::Time => "Time",
            NodeKind::Value => "Value",
            NodeKind::Math(op) => op.label(),
            NodeKind::Output => "Output",
            NodeKind::Group(_) => "Group",
            NodeKind::GroupInput => "Group Input",
            NodeKind::GroupOutput => "Group Output",
        }
    }

    /// The sockets of a node of this kind. `group` is the interface of the
    /// group it refers to (for `Group`) or sits in (for `GroupInput`/`GroupOutput`).
    pub fn signature(self, group: Option<(&str, &Interface)>) -> Signature {
        let sockets = |labels: &[String]| labels.iter().map(|l| Socket::new(l, 0.0)).collect();
        let (inputs, outputs) = match self {
            NodeKind::Uv => (vec![], vec![Socket::new("u", 0.0), Socket::new("v", 0.0)]),
            NodeKind::Time => (vec![], vec![Socket::new("seconds", 0.0)]),
            NodeKind::Value => (
                vec![Socket::new("value", 0.0)],
                vec![Socket::new("value", 0.0)],
            ),
            NodeKind::Math(op) => (
                op.inputs()
                    .iter()
                    .map(|&(l, d)| Socket::new(l, d))
                    .collect(),
                vec![Socket::new("result", 0.0)],
            ),
            NodeKind::Output => (["r", "g", "b"].map(|c| Socket::new(c, 0.0)).into(), vec![]),
            NodeKind::Group(_) => group
                .map(|(_, i)| (sockets(&i.inputs), sockets(&i.outputs)))
                .unwrap_or_default(),
            NodeKind::GroupInput => (
                vec![],
                group.map(|(_, i)| sockets(&i.inputs)).unwrap_or_default(),
            ),
            NodeKind::GroupOutput => (
                group.map(|(_, i)| sockets(&i.outputs)).unwrap_or_default(),
                vec![],
            ),
        };
        let title = match (self, group) {
            (NodeKind::Group(_), Some((label, _))) => label.to_owned(),
            _ => self.label().to_owned(),
        };
        Signature {
            title,
            inputs,
            outputs,
        }
    }
}
