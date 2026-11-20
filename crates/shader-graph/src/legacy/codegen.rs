use std::collections::HashSet;
use std::fmt::Write;

use thiserror::Error;

use crate::legacy::graph::{Document, Endpoint, GroupId, Node, NodeId, Project};
use crate::legacy::nodes::NodeKind;

#[derive(Debug, Error, PartialEq)]
pub enum CodegenError {
    #[error("group {0:?} is missing")]
    MissingGroup(GroupId),
    #[error("group {0:?} contains itself")]
    RecursiveGroup(GroupId),
    #[error("the graph has a cycle")]
    Cycle,
}

const PRELUDE: &str = r#"struct Frame {
    time: f32,
    _pad: f32,
    resolution: vec2f,
}

@group(0) @binding(0) var<uniform> frame: Frame;

// Readable from group functions too, which don't get the fragment's inputs.
var<private> frag_uv: vec2f;

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let xy = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4f(xy * 2.0 - 1.0, 0.0, 1.0);
    // Clip space points up, so v grows upwards too.
    out.uv = xy;
    return out;
}
"#;

/// Generates the WGSL module drawing `project`'s root graph: a full-screen
/// triangle (`vs_main`) and its colour (`fs_main`).
pub fn generate(project: &Project) -> Result<String, CodegenError> {
    let mut generator = Generator {
        project,
        functions: String::new(),
        emitted: HashSet::new(),
        in_progress: HashSet::new(),
    };
    let output = project
        .root
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Output)
        .map(|n| n.id);
    let mut body = Body::new(&project.root);
    let channels = match output {
        Some(id) => (0..3)
            .map(|socket| generator.input(&mut body, Endpoint { node: id, socket }))
            .collect::<Result<Vec<_>, _>>()?,
        None => vec!["0.0".into(); 3],
    };

    let mut wgsl = String::from(PRELUDE);
    wgsl.push_str(&generator.functions);
    wgsl.push_str("\n@fragment\nfn fs_main(in: VertexOut) -> @location(0) vec4f {\n");
    wgsl.push_str("    frag_uv = in.uv;\n");
    wgsl.push_str(&body.lines);
    let _ = writeln!(wgsl, "    return vec4f({}, 1.0);\n}}", channels.join(", "));
    Ok(wgsl)
}

/// A function body being written: its statements and the nodes they compute.
struct Body<'a> {
    document: &'a Document,
    lines: String,
    done: HashSet<NodeId>,
    visiting: HashSet<NodeId>,
}

impl<'a> Body<'a> {
    fn new(document: &'a Document) -> Self {
        Self {
            document,
            lines: String::new(),
            done: HashSet::new(),
            visiting: HashSet::new(),
        }
    }
}

struct Generator<'a> {
    project: &'a Project,
    functions: String,
    emitted: HashSet<GroupId>,
    in_progress: HashSet<GroupId>,
}

impl<'a> Generator<'a> {
    /// The expression an input socket reads: its link's value, else its own.
    fn input(&mut self, body: &mut Body<'a>, to: Endpoint) -> Result<String, CodegenError> {
        if let Some(edge) = body.document.edge_into(to) {
            let from = edge.from;
            let node = body
                .document
                .node(from.node)
                .expect("edges link existing nodes");
            self.node(body, node)?;
            return Ok(output_var(node, from.socket));
        }
        let value = body
            .document
            .node(to.node)
            .and_then(|n| n.values.get(to.socket))
            .copied()
            .unwrap_or_default();
        Ok(literal(value))
    }

    /// Writes the statements computing `node`, after those of its inputs.
    fn node(&mut self, body: &mut Body<'a>, node: &'a Node) -> Result<(), CodegenError> {
        if body.done.contains(&node.id) {
            return Ok(());
        }
        if !body.visiting.insert(node.id) {
            return Err(CodegenError::Cycle);
        }
        let inputs = (0..node.values.len())
            .map(|socket| {
                self.input(
                    body,
                    Endpoint {
                        node: node.id,
                        socket,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let id = node.id.0;
        let lines = &mut body.lines;
        match node.kind {
            NodeKind::Uv => {
                let _ = writeln!(
                    lines,
                    "    let n{id}_0 = frag_uv.x;\n    let n{id}_1 = frag_uv.y;"
                );
            }
            NodeKind::Time => {
                let _ = writeln!(lines, "    let n{id}_0 = frame.time;");
            }
            NodeKind::Value => {
                let _ = writeln!(lines, "    let n{id}_0 = {};", inputs[0]);
            }
            NodeKind::Math(op) => {
                let _ = writeln!(lines, "    let n{id}_0 = {};", op.wgsl(&inputs));
            }
            NodeKind::GroupInput => {
                // Its outputs are the enclosing function's parameters.
            }
            NodeKind::Group(group) => {
                let name = self.group_function(group)?;
                let _ = writeln!(body.lines, "    let n{id} = {name}({});", inputs.join(", "));
            }
            NodeKind::Output | NodeKind::GroupOutput => {}
        }
        body.visiting.remove(&node.id);
        body.done.insert(node.id);
        Ok(())
    }

    /// Emits `group`'s function (once) and returns its name.
    fn group_function(&mut self, id: GroupId) -> Result<String, CodegenError> {
        let name = format!("group_{}", id.0);
        if self.emitted.contains(&id) {
            return Ok(name);
        }
        if !self.in_progress.insert(id) {
            return Err(CodegenError::RecursiveGroup(id));
        }
        let group = self
            .project
            .groups
            .get(&id)
            .ok_or(CodegenError::MissingGroup(id))?;
        let output = group
            .document
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::GroupOutput);
        let mut body = Body::new(&group.document);
        let results = (0..group.interface.outputs.len())
            .map(|socket| match output {
                Some(node) => self.input(
                    &mut body,
                    Endpoint {
                        node: node.id,
                        socket,
                    },
                ),
                None => Ok(literal(0.0)),
            })
            .collect::<Result<Vec<_>, _>>()?;

        let struct_name = format!("Group{}", id.0);
        let mut source = String::new();
        let _ = writeln!(source, "\nstruct {struct_name} {{");
        for i in 0..results.len().max(1) {
            let _ = writeln!(source, "    o{i}: f32,");
        }
        let _ = writeln!(source, "}}\n");
        let params: Vec<String> = (0..group.interface.inputs.len())
            .map(|i| format!("i{i}: f32"))
            .collect();
        let _ = writeln!(
            source,
            "fn {name}({}) -> {struct_name} {{",
            params.join(", ")
        );
        source.push_str(&body.lines);
        let results = if results.is_empty() {
            vec![literal(0.0)]
        } else {
            results
        };
        let _ = writeln!(
            source,
            "    return {struct_name}({});\n}}",
            results.join(", ")
        );

        self.functions.push_str(&source);
        self.in_progress.remove(&id);
        self.emitted.insert(id);
        Ok(name)
    }
}

fn output_var(node: &Node, socket: usize) -> String {
    match node.kind {
        NodeKind::Group(_) => format!("n{}.o{socket}", node.id.0),
        NodeKind::GroupInput => format!("i{socket}"),
        _ => format!("n{}_{socket}", node.id.0),
    }
}

fn literal(value: f32) -> String {
    let value = if value.is_finite() { value } else { 0.0 };
    if value < 0.0 {
        format!("({value:?})")
    } else {
        format!("{value:?}")
    }
}
