//! Headless model of a float-only shader graph and its WGSL codegen.

mod codegen;
mod graph;
mod nodes;

pub use codegen::{CodegenError, generate};
pub use graph::{
    DocId, Document, Edge, EditError, Endpoint, Group, GroupId, Node, NodeId, Project,
};
pub use nodes::{Interface, MathOp, NodeKind, Signature, Socket};

impl Project {
    /// A starting graph: animated stripes from UV and Time.
    pub fn demo() -> Self {
        let mut project = Project::default();
        let doc = DocId::Root;
        let output = project.root.nodes[0].id;
        let add =
            |p: &mut Project, kind, position| p.add_node(doc, kind, position).expect("root exists");
        let uv = add(&mut project, NodeKind::Uv, [-600.0, -100.0]);
        let time = add(&mut project, NodeKind::Time, [-600.0, 100.0]);
        let mul = add(
            &mut project,
            NodeKind::Math(MathOp::Multiply),
            [-380.0, -100.0],
        );
        let sum = add(&mut project, NodeKind::Math(MathOp::Add), [-160.0, -50.0]);
        let sin = add(&mut project, NodeKind::Math(MathOp::Sin), [60.0, -50.0]);
        project
            .set_value(
                doc,
                Endpoint {
                    node: mul,
                    socket: 1,
                },
                12.0,
            )
            .ok();
        let link = |p: &mut Project, from: (NodeId, usize), to: (NodeId, usize)| {
            p.connect(
                doc,
                Edge {
                    from: Endpoint {
                        node: from.0,
                        socket: from.1,
                    },
                    to: Endpoint {
                        node: to.0,
                        socket: to.1,
                    },
                },
            )
            .expect("demo links are valid");
        };
        link(&mut project, (uv, 0), (mul, 0));
        link(&mut project, (mul, 0), (sum, 0));
        link(&mut project, (time, 0), (sum, 1));
        link(&mut project, (sum, 0), (sin, 0));
        link(&mut project, (sin, 0), (output, 0));
        link(&mut project, (uv, 1), (output, 1));
        project
    }
}

#[cfg(test)]
mod tests;
