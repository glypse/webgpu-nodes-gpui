//! The controlled data model: nodes, edges, connections and the changes the
//! flow reports (`NodeChange`/`EdgeChange`, applied with `apply_*_changes`).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{AnyElement, App, Hsla, SharedString, Window};

use crate::edge_path::Position;
use crate::geometry::{Extent, Xy};
use crate::node::NodeProps;

pub type Id = SharedString;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandleType {
    Source,
    Target,
}

impl HandleType {
    pub fn opposite(self) -> HandleType {
        match self {
            HandleType::Source => HandleType::Target,
            HandleType::Target => HandleType::Source,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Connection {
    pub source: Id,
    pub source_handle: Option<Id>,
    pub target: Id,
    pub target_handle: Option<Id>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionMode {
    /// Source handles connect to target handles only.
    #[default]
    Strict,
    /// Any handle connects to any other.
    Loose,
}

pub type NodeRender = Rc<dyn Fn(&NodeProps, &mut Window, &mut App) -> AnyElement>;

#[derive(Clone)]
pub struct Node {
    pub id: Id,
    pub position: Xy,
    pub label: Option<SharedString>,
    /// Fixed size in flow units; the measured size is used otherwise.
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub selected: bool,
    pub dragging: bool,
    /// `None` follows the flow's global setting.
    pub draggable: Option<bool>,
    pub selectable: Option<bool>,
    pub connectable: Option<bool>,
    pub deletable: Option<bool>,
    pub hidden: bool,
    pub z_index: i32,
    /// Which point of the node `position` refers to, as fractions of its size.
    pub origin: (f32, f32),
    pub extent: Option<Extent>,
    pub render: Option<NodeRender>,
}

impl Node {
    pub fn new(id: impl Into<Id>, position: Xy) -> Self {
        Self {
            id: id.into(),
            position,
            label: None,
            width: None,
            height: None,
            selected: false,
            dragging: false,
            draggable: None,
            selectable: None,
            connectable: None,
            deletable: None,
            hidden: false,
            z_index: 0,
            origin: (0.0, 0.0),
            extent: None,
            render: None,
        }
    }

    pub fn render(
        mut self,
        f: impl Fn(&NodeProps, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.render = Some(Rc::new(f));
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EdgeKind {
    #[default]
    Bezier,
    SmoothStep,
    Step,
    Straight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeColor {
    Solid(Hsla),
    /// From the source's end to the target's.
    Gradient(Hsla, Hsla),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeStyle {
    pub color: EdgeColor,
    pub width: f32,
    /// Dash and gap lengths.
    pub dash: Option<(f32, f32)>,
    pub opacity: f32,
    /// Opacity while selected.
    pub selected_opacity: f32,
}

impl EdgeStyle {
    pub fn solid(color: Hsla) -> Self {
        Self {
            color: EdgeColor::Solid(color),
            width: 1.0,
            dash: None,
            opacity: 1.0,
            selected_opacity: 1.0,
        }
    }

    pub fn gradient(from: Hsla, to: Hsla) -> Self {
        Self { color: EdgeColor::Gradient(from, to), ..Self::solid(from) }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn dash(mut self, on: f32, off: f32) -> Self {
        self.dash = Some((on, off));
        self
    }

    pub fn opacity(mut self, opacity: f32, selected: f32) -> Self {
        self.opacity = opacity;
        self.selected_opacity = selected;
        self
    }
}

/// A pill drawn at the edge's middle.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeLabel {
    pub text: SharedString,
    pub color: Hsla,
    pub background: Hsla,
    pub border: Option<Hsla>,
}

#[derive(Clone)]
pub struct Edge {
    pub id: Id,
    pub source: Id,
    pub source_handle: Option<Id>,
    pub target: Id,
    pub target_handle: Option<Id>,
    pub kind: EdgeKind,
    pub style: Option<EdgeStyle>,
    pub selected: bool,
    pub selectable: Option<bool>,
    pub deletable: Option<bool>,
    pub reconnectable: Option<bool>,
    pub hidden: bool,
    pub z_index: i32,
    pub label: Option<EdgeLabel>,
    /// A direction arrow at the middle, in this colour.
    pub arrow: Option<Hsla>,
    /// Width of the invisible band that takes clicks.
    pub interaction_width: f32,
    /// Drawn as dashed stubs at both ends, the target's carrying a name.
    pub tag: Option<EdgeTag>,
}

/// How a link with its middle left out is drawn: a stub out of the source,
/// and one into the target with `text` at its start.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeTag {
    pub text: String,
    pub source_color: Hsla,
    pub target_color: Hsla,
    /// Moves the target stub vertically, to fan out the stubs of links into one handle.
    pub offset: f32,
    /// The whole link is drawn too, faintly.
    pub revealed: bool,
}

impl Edge {
    pub fn new(
        id: impl Into<Id>,
        source: impl Into<Id>,
        source_handle: Option<Id>,
        target: impl Into<Id>,
        target_handle: Option<Id>,
    ) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            source_handle,
            target: target.into(),
            target_handle,
            kind: EdgeKind::Bezier,
            style: None,
            selected: false,
            selectable: None,
            deletable: None,
            reconnectable: None,
            hidden: false,
            z_index: 0,
            label: None,
            arrow: None,
            interaction_width: 20.0,
            tag: None,
        }
    }

    pub fn from_connection(c: &Connection) -> Self {
        Self::new(
            edge_id(c),
            c.source.clone(),
            c.source_handle.clone(),
            c.target.clone(),
            c.target_handle.clone(),
        )
    }

    pub fn kind(mut self, kind: EdgeKind) -> Self {
        self.kind = kind;
        self
    }

}

/// The default edge id for a connection. Each part is length-prefixed (or `-`
/// when absent), so no two distinct connections share an id.
pub fn edge_id(c: &Connection) -> Id {
    fn part(s: Option<&str>) -> String {
        match s {
            Some(s) => format!("{}:{s}", s.len()),
            None => "-".into(),
        }
    }
    format!(
        "{}{}{}{}",
        part(Some(c.source.as_ref())),
        part(c.source_handle.as_deref()),
        part(Some(c.target.as_ref())),
        part(c.target_handle.as_deref()),
    )
    .into()
}

#[derive(Clone)]
pub enum NodeChange {
    Position { id: Id, position: Option<Xy>, dragging: Option<bool> },
    Select { id: Id, selected: bool },
    Remove { id: Id },
    Add { node: Node },
    Replace { id: Id, node: Node },
}

#[derive(Clone)]
pub enum EdgeChange {
    Select { id: Id, selected: bool },
    Remove { id: Id },
    Add { edge: Edge },
    Replace { id: Id, edge: Edge },
}

/// First position of each id. Changes address nodes by id, so this lets them
/// avoid a scan per change.
fn node_index(nodes: &[Node]) -> HashMap<Id, usize> {
    let mut index = HashMap::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        index.entry(n.id.clone()).or_insert(i);
    }
    index
}

fn edge_index(edges: &[Edge]) -> HashMap<Id, usize> {
    let mut index = HashMap::with_capacity(edges.len());
    for (i, e) in edges.iter().enumerate() {
        index.entry(e.id.clone()).or_insert(i);
    }
    index
}

pub fn apply_node_changes(changes: &[NodeChange], nodes: Vec<Node>) -> Vec<Node> {
    let mut nodes = nodes;
    let mut index = node_index(&nodes);
    for change in changes {
        match change {
            NodeChange::Add { node } => {
                index.entry(node.id.clone()).or_insert(nodes.len());
                nodes.push(node.clone());
            }
            NodeChange::Remove { id } => {
                nodes.retain(|n| n.id != *id);
                index = node_index(&nodes);
            }
            NodeChange::Replace { id, node } => {
                if let Some(&i) = index.get(id) {
                    nodes[i] = node.clone();
                    if node.id != *id {
                        index = node_index(&nodes);
                    }
                }
            }
            NodeChange::Position { id, position, dragging } => {
                if let Some(&i) = index.get(id) {
                    let n = &mut nodes[i];
                    if let Some(p) = position {
                        n.position = *p;
                    }
                    if let Some(d) = dragging {
                        n.dragging = *d;
                    }
                }
            }
            NodeChange::Select { id, selected } => {
                if let Some(&i) = index.get(id) {
                    nodes[i].selected = *selected;
                }
            }
        }
    }
    nodes
}

pub fn apply_edge_changes(changes: &[EdgeChange], edges: Vec<Edge>) -> Vec<Edge> {
    let mut edges = edges;
    let mut index = edge_index(&edges);
    for change in changes {
        match change {
            EdgeChange::Add { edge } => {
                index.entry(edge.id.clone()).or_insert(edges.len());
                edges.push(edge.clone());
            }
            EdgeChange::Remove { id } => {
                edges.retain(|e| e.id != *id);
                index = edge_index(&edges);
            }
            EdgeChange::Replace { id, edge } => {
                if let Some(&i) = index.get(id) {
                    edges[i] = edge.clone();
                    if edge.id != *id {
                        index = edge_index(&edges);
                    }
                }
            }
            EdgeChange::Select { id, selected } => {
                if let Some(&i) = index.get(id) {
                    edges[i].selected = *selected;
                }
            }
        }
    }
    edges
}

/// Whether an equivalent edge exists already.
pub fn connection_exists(c: &Connection, edges: &[Edge]) -> bool {
    edges.iter().any(|e| {
        e.source == c.source
            && e.target == c.target
            && e.source_handle == c.source_handle
            && e.target_handle == c.target_handle
    })
}

/// Adds an edge for a connection unless it exists, as `addEdge`.
pub fn add_edge(connection: &Connection, mut edges: Vec<Edge>) -> Vec<Edge> {
    if !connection_exists(connection, &edges) {
        edges.push(Edge::from_connection(connection));
    }
    edges
}

/// Edges with an end on any of the given nodes.
pub fn connected_edges<'a>(node_ids: &[Id], edges: &'a [Edge]) -> Vec<&'a Edge> {
    let ids: HashSet<&Id> = node_ids.iter().collect();
    edges.iter().filter(|e| ids.contains(&e.source) || ids.contains(&e.target)).collect()
}

pub fn incomers<'a>(id: &Id, nodes: &'a [Node], edges: &[Edge]) -> Vec<&'a Node> {
    let sources: HashSet<&Id> = edges.iter().filter(|e| e.target == *id).map(|e| &e.source).collect();
    nodes.iter().filter(|n| sources.contains(&n.id)).collect()
}

pub fn outgoers<'a>(id: &Id, nodes: &'a [Node], edges: &[Edge]) -> Vec<&'a Node> {
    let targets: HashSet<&Id> = edges.iter().filter(|e| e.source == *id).map(|e| &e.target).collect();
    nodes.iter().filter(|n| targets.contains(&n.id)).collect()
}

/// Moves one end of an edge to a new connection, as `reconnectEdge`.
pub fn reconnect_edge(old: &Edge, connection: &Connection, edges: Vec<Edge>) -> Vec<Edge> {
    let mut edges = edges;
    let Some(index) = edges.iter().position(|e| e.id == old.id) else {
        return edges;
    };
    let mut edge = old.clone();
    edge.id = edge_id(connection);
    edge.source = connection.source.clone();
    edge.source_handle = connection.source_handle.clone();
    edge.target = connection.target.clone();
    edge.target_handle = connection.target_handle.clone();
    edges.remove(index);
    if connection_exists(connection, &edges) {
        return edges;
    }
    edges.insert(index, edge);
    edges
}

/// A connection being dragged; positions are
/// in flow units, `pointer` in the flow's local pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionState {
    /// `None` until the pointer is over something connectable.
    pub is_valid: Option<bool>,
    pub from: Xy,
    pub from_node: Id,
    pub from_handle: Option<Id>,
    pub from_type: HandleType,
    pub from_position: Position,
    pub to: Xy,
    pub to_node: Option<Id>,
    pub to_handle: Option<Id>,
    pub to_position: Position,
    pub pointer: Xy,
    /// The pointer in window coordinates.
    pub window_position: gpui::Point<gpui::Pixels>,
    /// The edge whose end is being moved, when reconnecting.
    pub reconnecting: Option<Id>,
}

/// Where a handle sits on its node, and its size, relative to the node's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct HandleBounds {
    pub id: Option<Id>,
    pub kind: HandleType,
    pub position: Position,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub connectable: bool,
    pub connectable_start: bool,
    pub connectable_end: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Node {
        Node::new(id.to_string(), Xy::ZERO)
    }

    fn conn(a: &str, b: &str) -> Connection {
        Connection {
            source: a.to_string().into(),
            source_handle: Some("out".into()),
            target: b.to_string().into(),
            target_handle: Some("in".into()),
        }
    }

    #[test]
    fn add_edge_skips_duplicates() {
        let edges = add_edge(&conn("a", "b"), Vec::new());
        assert_eq!(edges.len(), 1);
        assert_eq!(add_edge(&conn("a", "b"), edges).len(), 1);
    }

    #[test]
    fn edge_ids_do_not_collide_across_part_boundaries() {
        let split = |source: &str, handle: &str, target: &str| Connection {
            source: source.into(),
            source_handle: Some(handle.into()),
            target: target.into(),
            target_handle: None,
        };
        assert_ne!(edge_id(&split("a", "bc", "d")), edge_id(&split("ab", "c", "d")));
        assert_ne!(edge_id(&split("a", "-", "d")), edge_id(&split("a", "", "d")));
    }

    #[test]
    fn node_changes_apply_in_order() {
        let nodes = vec![node("a"), node("b")];
        let nodes = apply_node_changes(
            &[
                NodeChange::Position {
                    id: "a".into(),
                    position: Some(Xy::new(5.0, 6.0)),
                    dragging: Some(true),
                },
                NodeChange::Select { id: "b".into(), selected: true },
                NodeChange::Remove { id: "a".into() },
                NodeChange::Add { node: node("c") },
            ],
            nodes,
        );
        assert_eq!(nodes.iter().map(|n| n.id.as_ref()).collect::<Vec<_>>(), ["b", "c"]);
        assert!(nodes[0].selected);
    }

    #[test]
    fn edge_changes_apply() {
        let edges = add_edge(&conn("a", "b"), Vec::new());
        let id = edges[0].id.clone();
        let edges = apply_edge_changes(&[EdgeChange::Select { id: id.clone(), selected: true }], edges);
        assert!(edges[0].selected);
        assert!(apply_edge_changes(&[EdgeChange::Remove { id }], edges).is_empty());
    }

    #[test]
    fn connected_edges_touch_either_end() {
        let mut edges = add_edge(&conn("a", "b"), Vec::new());
        edges = add_edge(&conn("b", "c"), edges);
        edges = add_edge(&conn("c", "d"), edges);
        assert_eq!(connected_edges(&["b".into()], &edges).len(), 2);
    }

    #[test]
    fn incomers_and_outgoers() {
        let nodes = vec![node("a"), node("b"), node("c")];
        let edges = add_edge(&conn("a", "b"), add_edge(&conn("b", "c"), Vec::new()));
        assert_eq!(incomers(&"b".into(), &nodes, &edges)[0].id.as_ref(), "a");
        assert_eq!(outgoers(&"b".into(), &nodes, &edges)[0].id.as_ref(), "c");
    }

    #[test]
    fn reconnecting_moves_an_end() {
        let edges = add_edge(&conn("a", "b"), Vec::new());
        let old = edges[0].clone();
        let edges = reconnect_edge(&old, &conn("a", "c"), edges);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target.as_ref(), "c");
        // Reconnecting onto an existing connection merges them.
        let edges = add_edge(&conn("a", "b"), edges);
        let moved = edges.iter().find(|e| e.target.as_ref() == "c").unwrap().clone();
        assert_eq!(reconnect_edge(&moved, &conn("a", "b"), edges).len(), 1);
    }
}
