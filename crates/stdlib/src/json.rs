//! A library file's JSON: written with tab indentation and scalar arrays on one line, read back as plain values.

use serde::Serialize;
use serde_json::{Number, Value};
use shader_graph::graph::{GraphDocument, GraphEdge, GraphNode, GroupWidget};
use shader_graph::types::{GroupHandleDef, InputValue, TypeVariable};
use thiserror::Error;

use crate::build::BuiltGroup;

#[derive(Debug, Error)]
pub enum JsonError {
    #[error("the file isn't valid JSON: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("the JSON text is malformed at byte {0}")]
    Malformed(usize),
}

/// An edge as the files spell it: `sourceHandle` after `source`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EdgeOut<'a> {
    id: &'a str,
    source: &'a str,
    source_handle: &'a str,
    target: &'a str,
    target_handle: &'a str,
}

impl<'a> From<&'a GraphEdge> for EdgeOut<'a> {
    fn from(e: &'a GraphEdge) -> Self {
        Self {
            id: &e.id,
            source: &e.source,
            source_handle: &e.source_handle,
            target: &e.target,
            target_handle: &e.target_handle,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DefinitionOut<'a> {
    nodes: &'a [GraphNode],
    edges: Vec<EdgeOut<'a>>,
    inputs: &'a [GroupHandleDef],
    outputs: &'a [GroupHandleDef],
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    defaults: &'a std::collections::BTreeMap<String, InputValue>,
    #[serde(skip_serializing_if = "<[TypeVariable]>::is_empty")]
    type_variables: &'a [TypeVariable],
}

impl<'a> From<&'a GraphDocument> for DefinitionOut<'a> {
    fn from(d: &'a GraphDocument) -> Self {
        Self {
            nodes: &d.nodes,
            edges: d.edges.iter().map(Into::into).collect(),
            inputs: &d.inputs,
            outputs: &d.outputs,
            defaults: &d.defaults,
            type_variables: &d.type_variables,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOut<'a> {
    id: &'a str,
    name: &'a str,
    aliases: &'a [&'a str],
    category: &'a str,
    #[serde(skip_serializing_if = "<[&str]>::is_empty")]
    section: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    search_section: Option<&'a str>,
    dependencies: &'a [String],
    definition: DefinitionOut<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    widget: Option<&'a GroupWidget>,
}

fn file_out(group: &BuiltGroup) -> FileOut<'_> {
    FileOut {
        id: group.spec.id,
        name: group.spec.name,
        aliases: group.spec.aliases,
        category: group.spec.category,
        section: &group.spec.section,
        search_section: group.spec.search_section,
        dependencies: &group.dependencies,
        definition: (&group.definition).into(),
        widget: group.widget.as_ref(),
    }
}

/// `value` with whole floats as integers, as the files spell them.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9e15 => {
                Value::Number(Number::from(f as i64))
            }
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

fn to_value(value: &impl Serialize) -> Result<Value, JsonError> {
    Ok(normalize(serde_json::to_value(value)?))
}

/// The `definition` of a library file, as reading the file gives it.
pub fn definition_value(definition: &GraphDocument) -> Value {
    to_value(&DefinitionOut::from(definition)).unwrap_or(Value::Null)
}

/// The whole library file of `group`, as reading it gives it.
pub fn file_value(group: &BuiltGroup) -> Result<Value, JsonError> {
    to_value(&file_out(group))
}

/// `file` (a library file's value) with every node at the origin: where nodes are is the layout's business.
pub fn unplaced(mut file: Value) -> Value {
    let nodes = file
        .get_mut("definition")
        .and_then(|d| d.get_mut("nodes"))
        .and_then(Value::as_array_mut);
    for node in nodes.into_iter().flatten() {
        if let Some(position) = node.get_mut("position") {
            *position = serde_json::json!({ "x": 0, "y": 0 });
        }
    }
    file
}

/// The text of the library file of `group`.
pub fn render(group: &BuiltGroup) -> Result<String, JsonError> {
    let compact = serde_json::to_string(&file_out(group))?;
    reformat(&compact)
}

enum Tree {
    Scalar(String),
    Array(Vec<Tree>),
    Object(Vec<(String, Tree)>),
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn string(&mut self) -> Result<String, JsonError> {
        let start = self.pos;
        self.pos += 1;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'\\' => self.pos += 1,
                b'"' => {
                    let text = self
                        .bytes
                        .get(start..self.pos)
                        .ok_or(JsonError::Malformed(start))?;
                    return String::from_utf8(text.to_vec())
                        .map_err(|_| JsonError::Malformed(start));
                }
                _ => {}
            }
        }
        Err(JsonError::Malformed(start))
    }

    fn value(&mut self) -> Result<Tree, JsonError> {
        match self.peek() {
            Some(b'"') => Ok(Tree::Scalar(self.string()?)),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek() != Some(b']') {
                    items.push(self.value()?);
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    }
                }
                self.pos += 1;
                Ok(Tree::Array(items))
            }
            Some(b'{') => {
                self.pos += 1;
                let mut entries = Vec::new();
                while self.peek() != Some(b'}') {
                    let key = self.string()?;
                    if self.peek() != Some(b':') {
                        return Err(JsonError::Malformed(self.pos));
                    }
                    self.pos += 1;
                    entries.push((key, self.value()?));
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    }
                }
                self.pos += 1;
                Ok(Tree::Object(entries))
            }
            Some(_) => {
                let start = self.pos;
                while self
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b']' | b'}'))
                {
                    self.pos += 1;
                }
                let text = self
                    .bytes
                    .get(start..self.pos)
                    .ok_or(JsonError::Malformed(start))?;
                let text = String::from_utf8_lossy(text).into_owned();
                Ok(Tree::Scalar(
                    text.strip_suffix(".0").map_or(text.clone(), str::to_owned),
                ))
            }
            None => Err(JsonError::Malformed(self.pos)),
        }
    }
}

fn print(tree: &Tree, depth: usize, out: &mut String) {
    let indent = |out: &mut String, depth: usize| (0..depth).for_each(|_| out.push('\t'));
    match tree {
        Tree::Scalar(text) => out.push_str(text),
        Tree::Array(items) if items.iter().all(|i| matches!(i, Tree::Scalar(_))) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print(item, depth, out);
            }
            out.push(']');
        }
        Tree::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                print(item, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push(']');
        }
        Tree::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Tree::Object(entries) => {
            out.push_str("{\n");
            for (i, (key, value)) in entries.iter().enumerate() {
                indent(out, depth + 1);
                out.push_str(key);
                out.push_str(": ");
                print(value, depth + 1, out);
                out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push('}');
        }
    }
}

/// Compact JSON as the library's files are laid out: tabs, one entry per line,
/// arrays of scalars on one line, whole floats without `.0`.
fn reformat(compact: &str) -> Result<String, JsonError> {
    let mut parser = Parser {
        bytes: compact.as_bytes(),
        pos: 0,
    };
    let tree = parser.value()?;
    let mut out = String::new();
    print(&tree, 0, &mut out);
    out.push('\n');
    Ok(out)
}
