use thiserror::Error;

/// What went wrong describing one group.
#[derive(Debug, Error)]
pub enum ErrorKind {
    #[error("no node type \"{0}\"")]
    UnknownNodeType(String),
    #[error("a {node} node isn't added with `node` or `op`: use {instead}")]
    OwnMethod { node: String, instead: &'static str },
    #[error("a {node} node has no {side} \"{name}\" (it has {known})")]
    NoSocket {
        node: String,
        side: &'static str,
        name: String,
        known: String,
    },
    #[error("a {node} node has no output to use as a value")]
    NoOutput { node: String },
    #[error("a {node} node takes {max} operands, given {given}")]
    TooManyOperands {
        node: String,
        max: usize,
        given: usize,
    },
    #[error("input \"{name}\" of a {node} node takes one operand, given {given}")]
    NotVariadic {
        node: String,
        name: String,
        given: usize,
    },
    #[error("a {node} node has no setting \"{key}\" (it has {known})")]
    NoSetting {
        node: String,
        key: String,
        known: String,
    },
    #[error("setting \"{key}\" of a {node} node has no option \"{value}\" (options: {options})")]
    NoOption {
        node: String,
        key: String,
        value: String,
        options: String,
    },
    #[error("a {node} node has no type family \"{key}\" (it has {known})")]
    NoFamily {
        node: String,
        key: String,
        known: String,
    },
    #[error("no group described as \"{0}\"")]
    NoGroup(String),
    #[error("\"{0}\" calls itself")]
    Cycle(String),
    #[error("a group it calls, \"{0}\", didn't build")]
    DependencyFailed(String),
    #[error("{what} \"{key}\" added twice")]
    Duplicate { what: &'static str, key: String },
    #[error("declares {side} \"{key}\" but adds none")]
    Missing { side: &'static str, key: String },
    #[error("adds {side} \"{key}\" it doesn't declare")]
    Undeclared { side: &'static str, key: String },
    #[error("\"{0}\" isn't an enum input")]
    NotEnum(String),
    #[error("its widget names no input \"{0}\"")]
    WidgetInput(String),
    #[error("{0}")]
    Invalid(String),
}

/// An error and the group it's in.
#[derive(Debug, Error)]
#[error("{group}: {kind}")]
pub struct BuildError {
    pub group: String,
    #[source]
    pub kind: ErrorKind,
}

/// Every error of a build, one per line.
#[derive(Debug, Error)]
#[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
pub struct BuildErrors(pub Vec<BuildError>);
