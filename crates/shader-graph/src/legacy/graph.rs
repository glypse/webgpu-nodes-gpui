use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::legacy::nodes::{Interface, NodeKind, Signature};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupId(pub u64);

/// The root graph or one group's definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocId {
    Root,
    Group(GroupId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    pub node: NodeId,
    pub socket: usize,
}

/// A link from an output socket to an input socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    pub from: Endpoint,
    pub to: Endpoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub position: [f32; 2],
    /// Values of the inputs, used while they're unlinked.
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Document {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn edge_into(&self, to: Endpoint) -> Option<&Edge> {
        self.edges.iter().find(|e| e.to == to)
    }

    /// Whether `to` can be reached from `from` following edges downstream.
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut stack = vec![from];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == to {
                return true;
            }
            if seen.insert(id) {
                stack.extend(
                    self.edges
                        .iter()
                        .filter(|e| e.from.node == id)
                        .map(|e| e.to.node),
                );
            }
        }
        false
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub label: String,
    pub interface: Interface,
    pub document: Document,
}

#[derive(Debug, Error, PartialEq)]
pub enum EditError {
    #[error("no such document")]
    MissingDocument,
    #[error("no such node")]
    MissingNode,
    #[error("no such socket")]
    MissingSocket,
    #[error("the link would make a cycle")]
    Cycle,
    #[error("a group can't contain itself")]
    RecursiveGroup,
    #[error("nothing to group")]
    EmptyGroup,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub root: Document,
    pub groups: BTreeMap<GroupId, Group>,
    next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        let mut project = Self {
            root: Document::default(),
            groups: BTreeMap::new(),
            next_id: 0,
        };
        project
            .add_node(DocId::Root, NodeKind::Output, [400.0, 0.0])
            .ok();
        project
    }
}

impl Project {
    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn document(&self, doc: DocId) -> Option<&Document> {
        match doc {
            DocId::Root => Some(&self.root),
            DocId::Group(id) => self.groups.get(&id).map(|g| &g.document),
        }
    }

    fn document_mut(&mut self, doc: DocId) -> Result<&mut Document, EditError> {
        match doc {
            DocId::Root => Ok(&mut self.root),
            DocId::Group(id) => self
                .groups
                .get_mut(&id)
                .map(|g| &mut g.document)
                .ok_or(EditError::MissingDocument),
        }
    }

    pub fn signature(&self, doc: DocId, kind: NodeKind) -> Signature {
        let group = match (kind, doc) {
            (NodeKind::Group(id), _)
            | (NodeKind::GroupInput | NodeKind::GroupOutput, DocId::Group(id)) => {
                self.groups.get(&id)
            }
            _ => None,
        };
        kind.signature(group.map(|g| (g.label.as_str(), &g.interface)))
    }

    /// Whether `group`'s definition uses `target`, directly or through other groups.
    pub fn group_uses(&self, group: GroupId, target: GroupId) -> bool {
        group == target
            || self.groups.get(&group).is_some_and(|g| {
                g.document.nodes.iter().any(|n| match n.kind {
                    NodeKind::Group(inner) => self.group_uses(inner, target),
                    _ => false,
                })
            })
    }

    pub fn add_node(
        &mut self,
        doc: DocId,
        kind: NodeKind,
        position: [f32; 2],
    ) -> Result<NodeId, EditError> {
        if let (NodeKind::Group(inner), DocId::Group(outer)) = (kind, doc)
            && self.group_uses(inner, outer)
        {
            return Err(EditError::RecursiveGroup);
        }
        let values = self
            .signature(doc, kind)
            .inputs
            .iter()
            .map(|s| s.default)
            .collect();
        let id = NodeId(self.fresh_id());
        self.document_mut(doc)?.nodes.push(Node {
            id,
            kind,
            position,
            values,
        });
        Ok(id)
    }

    /// Removes nodes and their links. The root's Output node stays.
    pub fn remove_nodes(&mut self, doc: DocId, ids: &[NodeId]) -> Result<(), EditError> {
        let document = self.document_mut(doc)?;
        let removed: HashSet<NodeId> = document
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id) && n.kind != NodeKind::Output)
            .map(|n| n.id)
            .collect();
        document.nodes.retain(|n| !removed.contains(&n.id));
        document
            .edges
            .retain(|e| !removed.contains(&e.from.node) && !removed.contains(&e.to.node));
        Ok(())
    }

    pub fn move_node(
        &mut self,
        doc: DocId,
        id: NodeId,
        position: [f32; 2],
    ) -> Result<(), EditError> {
        self.document_mut(doc)?
            .node_mut(id)
            .ok_or(EditError::MissingNode)?
            .position = position;
        Ok(())
    }

    pub fn set_value(&mut self, doc: DocId, to: Endpoint, value: f32) -> Result<(), EditError> {
        let node = self
            .document_mut(doc)?
            .node_mut(to.node)
            .ok_or(EditError::MissingNode)?;
        *node
            .values
            .get_mut(to.socket)
            .ok_or(EditError::MissingSocket)? = value;
        Ok(())
    }

    /// Links `from` to `to`, replacing the link `to` had.
    pub fn connect(&mut self, doc: DocId, edge: Edge) -> Result<(), EditError> {
        let document = self.document(doc).ok_or(EditError::MissingDocument)?;
        let (Some(from), Some(to)) = (document.node(edge.from.node), document.node(edge.to.node))
        else {
            return Err(EditError::MissingNode);
        };
        if edge.from.socket >= self.signature(doc, from.kind).outputs.len()
            || edge.to.socket >= self.signature(doc, to.kind).inputs.len()
        {
            return Err(EditError::MissingSocket);
        }
        if document.reaches(edge.to.node, edge.from.node) {
            return Err(EditError::Cycle);
        }
        let document = self.document_mut(doc)?;
        document.edges.retain(|e| e.to != edge.to);
        document.edges.push(edge);
        Ok(())
    }

    pub fn disconnect(&mut self, doc: DocId, to: Endpoint) -> Result<(), EditError> {
        self.document_mut(doc)?.edges.retain(|e| e.to != to);
        Ok(())
    }

    /// Moves `ids` into a new group and puts a node using it in their place.
    /// Links crossing the selection become the group's inputs and outputs.
    pub fn group_nodes(&mut self, doc: DocId, ids: &[NodeId]) -> Result<NodeId, EditError> {
        let document = self.document(doc).ok_or(EditError::MissingDocument)?;
        let selected: HashSet<NodeId> = document
            .nodes
            .iter()
            .filter(|n| {
                ids.contains(&n.id)
                    && !matches!(
                        n.kind,
                        NodeKind::Output | NodeKind::GroupInput | NodeKind::GroupOutput
                    )
            })
            .map(|n| n.id)
            .collect();
        if selected.is_empty() {
            return Err(EditError::EmptyGroup);
        }

        let inside = |e: &Endpoint| selected.contains(&e.node);
        let mut incoming: Vec<Endpoint> = Vec::new();
        let mut outgoing: Vec<Endpoint> = Vec::new();
        for edge in &document.edges {
            match (inside(&edge.from), inside(&edge.to)) {
                (false, true) if !incoming.contains(&edge.from) => incoming.push(edge.from),
                (true, false) if !outgoing.contains(&edge.from) => outgoing.push(edge.from),
                _ => {}
            }
        }
        let output_label = |e: &Endpoint| {
            let node = document.node(e.node).expect("edges link existing nodes");
            self.signature(doc, node.kind).outputs[e.socket]
                .label
                .clone()
        };
        let interface = Interface {
            inputs: incoming.iter().map(output_label).collect(),
            outputs: outgoing.iter().map(output_label).collect(),
        };

        let (nodes, others): (Vec<Node>, Vec<Node>) = document
            .nodes
            .iter()
            .cloned()
            .partition(|n| selected.contains(&n.id));
        let count = nodes.len() as f32;
        let center = nodes.iter().fold([0.0, 0.0], |c, n| {
            [c[0] + n.position[0] / count, c[1] + n.position[1] / count]
        });
        let min_x = nodes
            .iter()
            .map(|n| n.position[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = nodes
            .iter()
            .map(|n| n.position[0])
            .fold(f32::NEG_INFINITY, f32::max);

        let group_id = GroupId(self.fresh_id());
        let group_node = NodeId(self.fresh_id());
        let input_node = NodeId(self.fresh_id());
        let output_node = NodeId(self.fresh_id());

        let mut inner_edges = Vec::new();
        let mut outer_edges = Vec::new();
        let document = self.document(doc).ok_or(EditError::MissingDocument)?;
        for edge in &document.edges {
            match (inside(&edge.from), inside(&edge.to)) {
                (true, true) => inner_edges.push(*edge),
                (false, false) => outer_edges.push(*edge),
                (false, true) => {
                    let socket = incoming
                        .iter()
                        .position(|e| *e == edge.from)
                        .expect("collected above");
                    inner_edges.push(Edge {
                        from: Endpoint {
                            node: input_node,
                            socket,
                        },
                        to: edge.to,
                    });
                    outer_edges.push(Edge {
                        from: edge.from,
                        to: Endpoint {
                            node: group_node,
                            socket,
                        },
                    });
                }
                (true, false) => {
                    let socket = outgoing
                        .iter()
                        .position(|e| *e == edge.from)
                        .expect("collected above");
                    outer_edges.push(Edge {
                        from: Endpoint {
                            node: group_node,
                            socket,
                        },
                        to: edge.to,
                    });
                    inner_edges.push(Edge {
                        from: edge.from,
                        to: Endpoint {
                            node: output_node,
                            socket,
                        },
                    });
                }
            }
        }

        let mut inner_nodes = nodes;
        inner_nodes.push(Node {
            id: input_node,
            kind: NodeKind::GroupInput,
            position: [min_x - 250.0, center[1]],
            values: vec![],
        });
        inner_nodes.push(Node {
            id: output_node,
            kind: NodeKind::GroupOutput,
            position: [max_x + 250.0, center[1]],
            values: vec![0.0; interface.outputs.len()],
        });
        let label = format!("Group {}", self.groups.len() + 1);
        let group_values = vec![0.0; interface.inputs.len()];
        self.groups.insert(
            group_id,
            Group {
                label,
                interface,
                document: Document {
                    nodes: inner_nodes,
                    edges: inner_edges,
                },
            },
        );

        let document = self.document_mut(doc)?;
        document.nodes = others;
        document.nodes.push(Node {
            id: group_node,
            kind: NodeKind::Group(group_id),
            position: center,
            values: group_values,
        });
        document.edges = outer_edges;
        Ok(group_node)
    }
}
