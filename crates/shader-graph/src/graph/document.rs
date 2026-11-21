use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::types::{GroupHandleDef, InputValue, NodeData, TypeVariable};

/// Key of the root document inside `definitions`.
pub const ROOT_ID: &str = "__root__";

/// A fresh id for a node, an edge, a handle or a definition.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A document's camera: pan offset and zoom.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

/// Which side of a group's interface: its inputs, or its outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupSide {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRef {
    /// The key in `definitions` of the subgraph the group node runs.
    pub definition_ref: String,
    /// Whether the subgraph is read-only from this node.
    #[serde(default)]
    pub locked: bool,
}

pub const GROUP_INPUT: &str = "group-input";
pub const GROUP_OUTPUT: &str = "group-output";
pub const NODE_GROUP: &str = "node-group";
pub const FRAG_OUTPUT: &str = "frag-output";
pub const REROUTE: &str = "reroute";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub data: NodeData,
    pub position: Point,
    /// Present on group nodes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupRef>,
    /// The frame node of the same document this node is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

impl GraphNode {
    pub fn new(node_type: impl Into<String>, position: Point) -> Self {
        Self {
            id: new_id(),
            node_type: node_type.into(),
            data: NodeData::default(),
            position,
            group: None,
            frame: None,
        }
    }

    pub fn is(&self, node_type: &str) -> bool {
        self.node_type == node_type
    }

    pub fn definition_ref(&self) -> Option<&str> {
        self.group.as_ref().map(|g| g.definition_ref.as_str())
    }

    /// The side of its group's interface a boundary node shows, if it's one.
    pub fn side(&self) -> Option<GroupSide> {
        match self.node_type.as_str() {
            GROUP_INPUT => Some(GroupSide::Input),
            GROUP_OUTPUT => Some(GroupSide::Output),
            _ => None,
        }
    }

    pub fn deletable(&self) -> bool {
        self.node_type != FRAG_OUTPUT
    }
}

/// A link from an output handle to an input handle of the same document.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub source_handle: String,
    pub target_handle: String,
}

/// A link to make, before it has an id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Connection {
    pub source: String,
    pub source_handle: String,
    pub target: String,
    pub target_handle: String,
}

impl Connection {
    pub fn new(
        source: impl Into<String>,
        source_handle: impl Into<String>,
        target: impl Into<String>,
        target_handle: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            source_handle: source_handle.into(),
            target: target.into(),
            target_handle: target_handle.into(),
        }
    }

    pub fn into_edge(self) -> GraphEdge {
        GraphEdge {
            id: new_id(),
            source: self.source,
            target: self.target,
            source_handle: self.source_handle,
            target_handle: self.target_handle,
        }
    }
}

impl From<&GraphEdge> for Connection {
    fn from(e: &GraphEdge) -> Self {
        Connection::new(&e.source, &e.source_handle, &e.target, &e.target_handle)
    }
}

/// One level of the project: the root graph, or a group's definition.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphDocument {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Shared display label of all instances of this group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    /// Types the group's sockets share, each group node deciding which.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_variables: Vec<TypeVariable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<GroupHandleDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<GroupHandleDef>,
    /// What a group's inputs hold on group nodes until edited, by input id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, InputValue>,
}

impl GraphDocument {
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut GraphNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.node(id).is_some()
    }

    pub fn edge(&self, id: &str) -> Option<&GraphEdge> {
        self.edges.iter().find(|e| e.id == id)
    }

    pub fn edges_into<'a>(&'a self, node: &'a str) -> impl Iterator<Item = &'a GraphEdge> {
        self.edges.iter().filter(move |e| e.target == node)
    }

    pub fn edges_from<'a>(&'a self, node: &'a str) -> impl Iterator<Item = &'a GraphEdge> {
        self.edges.iter().filter(move |e| e.source == node)
    }

    pub fn interface(&self, side: GroupSide) -> &[GroupHandleDef] {
        match side {
            GroupSide::Input => &self.inputs,
            GroupSide::Output => &self.outputs,
        }
    }

    pub fn interface_mut(&mut self, side: GroupSide) -> &mut Vec<GroupHandleDef> {
        match side {
            GroupSide::Input => &mut self.inputs,
            GroupSide::Output => &mut self.outputs,
        }
    }

    /// Definitions referenced by the group nodes of the document.
    pub fn group_refs(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().filter_map(GraphNode::definition_ref)
    }

    /// The Group Output node a group output is linked on, else the first one.
    pub fn output_node_of(&self, handle: &str) -> Option<&GraphNode> {
        let outputs = || self.nodes.iter().filter(|n| n.is(GROUP_OUTPUT));
        outputs()
            .find(|n| self.edges_into(&n.id).any(|e| e.target_handle == handle))
            .or_else(|| outputs().next())
    }
}

/// The whole project: the root in `definitions[ROOT_ID]`, groups beside it.
/// Besides `definitions`, the original's `settings`, `layout` and `assets`
/// are kept as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphProject {
    pub definitions: BTreeMap<String, GraphDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
    #[serde(skip)]
    index: HashMap<String, String>,
}

impl Default for GraphProject {
    /// An empty root holding only the fragment output.
    fn default() -> Self {
        let root = GraphDocument {
            nodes: vec![GraphNode::new(FRAG_OUTPUT, Point::new(400.0, 0.0))],
            ..Default::default()
        };
        let mut project = Self {
            definitions: BTreeMap::from([(ROOT_ID.to_owned(), root)]),
            extra: BTreeMap::new(),
            index: HashMap::new(),
        };
        project.reindex();
        project
    }
}

impl GraphProject {
    pub fn from_definitions(definitions: BTreeMap<String, GraphDocument>) -> Self {
        let mut project = Self { definitions, extra: BTreeMap::new(), index: HashMap::new() };
        project.reindex();
        project
    }

    /// Rebuild the node lookup; call after nodes were added or removed
    /// through the public fields.
    pub fn reindex(&mut self) {
        self.index = self
            .definitions
            .iter()
            .flat_map(|(key, doc)| doc.nodes.iter().map(move |n| (n.id.clone(), key.clone())))
            .collect();
    }

    pub fn root(&self) -> &GraphDocument {
        &self.definitions[ROOT_ID]
    }

    pub fn root_mut(&mut self) -> &mut GraphDocument {
        self.definitions.get_mut(ROOT_ID).expect("a project always has a root")
    }

    pub fn document(&self, key: &str) -> Option<&GraphDocument> {
        self.definitions.get(key)
    }

    /// The key of the document holding a node, anywhere in the project.
    pub fn definition_ref_of(&self, node_id: &str) -> Option<&str> {
        self.index.get(node_id).map(String::as_str)
    }

    /// The document holding a node, anywhere in the project.
    pub fn document_of(&self, node_id: &str) -> Option<&GraphDocument> {
        self.definitions.get(self.definition_ref_of(node_id)?)
    }

    /// Look up a node by id across every document.
    pub fn resolve_node(&self, node_id: &str) -> Option<&GraphNode> {
        self.document_of(node_id)?.node(node_id)
    }
}
