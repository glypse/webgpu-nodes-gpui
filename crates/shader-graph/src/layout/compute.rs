//! The ELK graph of a document's nodes and of the blocks they're grouped in,
//! and the positions read back from ELK's output.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::elk;
use super::{LayoutError, LayoutSpacing, NodeGeometry};
use crate::graph::{
    Bounds, EMPTY_FRAME_SIZE, FRAME_PADDING, FrameAnalysis, GraphEdge, GraphNode, LAYOUT_FRAME,
    Point, Size, frame_rects,
};
use crate::zones::ZoneAnalysis;

pub struct LayoutInput<'a> {
    pub nodes: &'a [GraphNode],
    pub edges: &'a [GraphEdge],
    pub zones: &'a ZoneAnalysis,
    pub frames: &'a FrameAnalysis,
    /// A frame's geometry is that of its header (label and note).
    pub geometry: &'a dyn Fn(&GraphNode) -> NodeGeometry,
    pub spacing: LayoutSpacing,
}

/// Left-to-right layered layout of `input.nodes`. Links run between handles,
/// so crossings are minimised with the handles' order, and each zone and
/// frame is laid out as a block holding its nodes. Only links between the
/// given nodes count. Returns the new top-left corner of every node laid out.
pub fn compute_layout(input: &LayoutInput<'_>) -> Result<HashMap<String, Point>, LayoutError> {
    let nodes = input.nodes;
    if nodes.is_empty() {
        return Ok(HashMap::new());
    }
    let spacing = input.spacing;
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let links: Vec<&GraphEdge> = input
        .edges
        .iter()
        .filter(|e| ids.contains(e.source.as_str()) && ids.contains(e.target.as_str()))
        .collect();
    let mut touching: HashMap<&str, Vec<&GraphEdge>> = HashMap::new();
    for e in &links {
        touching.entry(e.source.as_str()).or_default().push(e);
        if e.target != e.source {
            touching.entry(e.target.as_str()).or_default().push(e);
        }
    }
    let geometries: HashMap<&str, NodeGeometry> = nodes
        .iter()
        .map(|n| (n.id.as_str(), (input.geometry)(n)))
        .collect();

    let mut elk_nodes: HashMap<&str, Value> = HashMap::new();
    for node in nodes {
        if is_frame_block(input.frames, node, &ids) {
            continue;
        }
        let Some(geometry) = geometries.get(node.id.as_str()).cloned() else {
            continue;
        };
        let geometry = empty_frame_size(node, geometry);
        let mut ports = Ports::default();
        for (handle, y) in &geometry.inputs {
            ports.add(input_port(&node.id, handle, *y));
        }
        for (handle, y) in &geometry.outputs {
            ports.add(output_port(&node.id, handle, *y, geometry.width));
        }
        // Links to handles the geometry doesn't know of still get a port.
        for e in touching.get(node.id.as_str()).into_iter().flatten() {
            if e.target == node.id {
                ports.add(input_port(
                    &node.id,
                    &e.target_handle,
                    geometry.height / 2.0,
                ));
            }
            if e.source == node.id {
                ports.add(output_port(
                    &node.id,
                    &e.source_handle,
                    geometry.height / 2.0,
                    geometry.width,
                ));
            }
        }
        elk_nodes.insert(
            node.id.as_str(),
            json!({
                "id": node.id,
                "width": geometry.width,
                "height": geometry.height,
                "ports": ports.into_values(),
                "layoutOptions": { "elk.portConstraints": "FIXED_POS" },
            }),
        );
    }

    let blocks = layout_blocks(input, &ids, &geometries);
    let mut children: HashMap<Option<usize>, Vec<Child>> = HashMap::new();
    for (index, parent) in blocks.parent.iter().enumerate() {
        children
            .entry(*parent)
            .or_default()
            .push(Child::Block(index));
    }
    for node in nodes {
        if !elk_nodes.contains_key(node.id.as_str()) {
            continue;
        }
        let container = blocks.of_node.get(&node.id).copied().flatten();
        children
            .entry(container)
            .or_default()
            .push(Child::Node(node.id.as_str()));
    }
    let mut builder = Builder {
        blocks: &blocks.list,
        children,
        elk_nodes,
    };
    let root_children = builder.build(None);

    let mut elk_edges: Vec<Value> = links
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "sources": [port_key(&e.source, "out", &e.source_handle)],
                "targets": [port_key(&e.target, "in", &e.target_handle)],
            })
        })
        .collect();
    // Keeps a zone's input left of its output, even with nothing in between.
    for block in &blocks.list {
        if block.kind != BlockKind::Zone {
            continue;
        }
        if let Some(zone) = input.zones.zones.get(&block.id) {
            elk_edges.push(json!({
                "id": format!("zone-link:{}", zone.id),
                "sources": [zone.input_id],
                "targets": [zone.output_id],
            }));
        }
    }

    let root = json!({
        "id": "root",
        "layoutOptions": root_options(spacing),
        "children": root_children,
        "edges": elk_edges,
    });
    let laid_out = elk::layout(root)?;

    let mut positions = HashMap::new();
    collect(&laid_out, (0.0, 0.0), &ids, &mut positions)?;

    // Frames go where they're drawn: around what they hold, once it's moved.
    let placed: HashMap<&str, (Point, Size)> = nodes
        .iter()
        .map(|n| {
            let at = positions.get(&n.id).copied().unwrap_or(n.position);
            let size = geometries
                .get(n.id.as_str())
                .map_or(Size::default(), |g| Size {
                    width: g.width,
                    height: g.height,
                });
            (n.id.as_str(), (at, size))
        })
        .collect();
    let rects = frame_rects(
        input.frames,
        |id| {
            placed.get(id).map(|(at, size)| Bounds {
                x: at.x,
                y: at.y,
                width: size.width,
                height: size.height,
            })
        },
        |id| placed.get(id).map_or(Size::default(), |(_, size)| *size),
        |_| Vec::new(),
    );
    for (id, rect) in rects {
        if ids.contains(id.as_str()) {
            positions.insert(id, Point::new(rect.x, rect.y));
        }
    }

    // Keep the nodes where they were: same top-left corner as before.
    let (Some(before), Some(after)) = (
        top_left(nodes.iter().map(|n| n.position)),
        top_left(positions.values().copied()),
    ) else {
        log::warn!("the layout gave no positions for {} nodes", nodes.len());
        return Ok(HashMap::new());
    };
    let (dx, dy) = (before.x - after.x, before.y - after.y);
    for p in positions.values_mut() {
        *p = Point::new(p.x + dx, p.y + dy);
    }
    Ok(positions)
}

/// A port's id and its JSON; a port id given twice is kept once.
#[derive(Default)]
struct Ports(Vec<(String, Value)>);

impl Ports {
    fn add(&mut self, (id, port): (String, Value)) {
        if !self.0.iter().any(|(existing, _)| *existing == id) {
            self.0.push((id, port));
        }
    }

    fn into_values(self) -> Vec<Value> {
        self.0.into_iter().map(|(_, port)| port).collect()
    }
}

fn port_key(node_id: &str, side: &str, handle_id: &str) -> String {
    format!("{node_id}:{side}:{handle_id}")
}

fn input_port(node_id: &str, handle_id: &str, y: f32) -> (String, Value) {
    let id = port_key(node_id, "in", handle_id);
    let port = json!({
        "id": id,
        "x": 0,
        "y": y,
        "width": 0,
        "height": 0,
        "layoutOptions": { "elk.port.side": "WEST" },
    });
    (id, port)
}

fn output_port(node_id: &str, handle_id: &str, y: f32, width: f32) -> (String, Value) {
    let id = port_key(node_id, "out", handle_id);
    let port = json!({
        "id": id,
        "x": width,
        "y": y,
        "width": 0,
        "height": 0,
        "layoutOptions": { "elk.port.side": "EAST" },
    });
    (id, port)
}

fn root_options(spacing: LayoutSpacing) -> Value {
    json!({
        "elk.algorithm": "layered",
        "elk.direction": "RIGHT",
        "elk.hierarchyHandling": "INCLUDE_CHILDREN",
        "elk.layered.spacing.nodeNodeBetweenLayers": spacing.between_layers.to_string(),
        "elk.spacing.nodeNode": spacing.between_nodes.to_string(),
        "elk.spacing.componentComponent": spacing.between_layers.to_string(),
        "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
        // More tries at ordering each column (default 7): measured on the
        // Voronoi groups, up to a quarter fewer crossings for under half again
        // the time.
        "elk.layered.thoroughness": "30",
        // Links are drawn by the front end: only node positions matter.
        "elk.edgeRouting": "POLYLINE",
    })
}

/// Whether `node` is a frame holding nodes laid out: it's laid out as a block around them.
fn is_frame_block(frames: &FrameAnalysis, node: &GraphNode, ids: &HashSet<&str>) -> bool {
    frames
        .frames
        .get(&node.id)
        .is_some_and(|frame| frame.descendants.iter().any(|id| ids.contains(id.as_str())))
}

/// An empty frame is laid out as a node of the size it's drawn at.
fn empty_frame_size(node: &GraphNode, geometry: NodeGeometry) -> NodeGeometry {
    if !node.is(LAYOUT_FRAME) {
        return geometry;
    }
    NodeGeometry {
        width: geometry.width.max(EMPTY_FRAME_SIZE.width),
        height: geometry.height + EMPTY_FRAME_SIZE.height,
        ..geometry
    }
}

fn top_left(points: impl Iterator<Item = Point>) -> Option<Point> {
    let mut found = false;
    let (mut x, mut y) = (f32::INFINITY, f32::INFINITY);
    for p in points {
        found = true;
        x = x.min(p.x);
        y = y.min(p.y);
    }
    found.then_some(Point::new(x, y))
}

/// Reads the top-left corner of each node laid out, summing offsets through the blocks.
fn collect(
    parent: &Value,
    offset: (f32, f32),
    ids: &HashSet<&str>,
    out: &mut HashMap<String, Point>,
) -> Result<(), LayoutError> {
    let Some(children) = parent.get("children").and_then(Value::as_array) else {
        return Ok(());
    };
    for child in children {
        let Some(id) = child.get("id").and_then(Value::as_str) else {
            continue;
        };
        let at = (
            offset.0 + coord(child, "x", id)?,
            offset.1 + coord(child, "y", id)?,
        );
        if ids.contains(id) {
            out.insert(id.to_owned(), Point::new(at.0, at.1));
        } else {
            collect(child, at, ids, out)?;
        }
    }
    Ok(())
}

fn coord(node: &Value, key: &str, id: &str) -> Result<f32, LayoutError> {
    node.get(key)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .ok_or_else(|| LayoutError::Elk(format!("no {key} for {id} in the layout output")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Zone,
    Frame,
}

/// A zone or a frame, laid out as a box holding its nodes.
struct Block {
    kind: BlockKind,
    /// The zone's or frame's id.
    id: String,
    /// The ELK id of the block.
    key: String,
    /// The nodes laid out inside it, at any depth.
    members: HashSet<String>,
    options: Value,
}

struct Blocks {
    list: Vec<Block>,
    /// The index in `list` of the block each block sits in, if any.
    parent: Vec<Option<usize>>,
    /// The block each laid-out node sits in, if any.
    of_node: HashMap<String, Option<usize>>,
}

enum Child<'a> {
    Block(usize),
    Node(&'a str),
}

/// Builds the ELK tree: each container lists its blocks, then its nodes.
struct Builder<'a> {
    blocks: &'a [Block],
    children: HashMap<Option<usize>, Vec<Child<'a>>>,
    elk_nodes: HashMap<&'a str, Value>,
}

impl Builder<'_> {
    fn build(&mut self, container: Option<usize>) -> Vec<Value> {
        let Some(children) = self.children.remove(&container) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(children.len());
        for child in children {
            match child {
                Child::Block(index) => {
                    let Some(block) = self.blocks.get(index) else {
                        continue;
                    };
                    let inner = self.build(Some(index));
                    out.push(json!({ "id": block.key, "layoutOptions": block.options, "children": inner }));
                }
                Child::Node(id) => {
                    if let Some(node) = self.elk_nodes.remove(id) {
                        out.push(node);
                    }
                }
            }
        }
        out
    }
}

/// The blocks to lay out: each zone with both its boundaries laid out, and
/// each frame holding nodes laid out, unless it holds part of a zone only
/// (zones decide what's in them, frames give way). Blocks nest as their
/// nodes do; a frame holding exactly a zone's nodes goes around it.
fn layout_blocks(
    input: &LayoutInput<'_>,
    ids: &HashSet<&str>,
    geometries: &HashMap<&str, NodeGeometry>,
) -> Blocks {
    let frames = input.frames;
    let spacing = input.spacing;

    let mut zone_blocks: Vec<Block> = Vec::new();
    for zone in input.zones.zones.values() {
        if !ids.contains(zone.input_id.as_str()) || !ids.contains(zone.output_id.as_str()) {
            continue;
        }
        let members: HashSet<String> = [&zone.input_id, &zone.output_id]
            .into_iter()
            .chain(zone.members.iter())
            .filter(|id| ids.contains(id.as_str()))
            .cloned()
            .collect();
        let p = spacing.zone_padding;
        zone_blocks.push(Block {
            kind: BlockKind::Zone,
            id: zone.id.clone(),
            key: format!("zone:{}", zone.id),
            members,
            options: pad(p, p, 0.0),
        });
    }

    let block_frames: Vec<&GraphNode> = input
        .nodes
        .iter()
        .filter(|n| is_frame_block(frames, n, ids))
        .collect();
    let block_frame_ids: HashSet<&str> = block_frames.iter().map(|n| n.id.as_str()).collect();
    let mut frame_blocks: Vec<Block> = Vec::new();
    for node in block_frames {
        let Some(frame) = frames.frames.get(&node.id) else {
            continue;
        };
        let members: HashSet<String> = frame
            .descendants
            .iter()
            .filter(|d| ids.contains(d.as_str()) && !block_frame_ids.contains(d.as_str()))
            .cloned()
            .collect();
        let straddles = zone_blocks.iter().any(|z| {
            let shared = z.members.iter().filter(|id| members.contains(*id)).count();
            shared > 0 && shared < z.members.len() && shared < members.len()
        });
        if straddles {
            continue;
        }
        let Some(header) = geometries.get(node.id.as_str()) else {
            continue;
        };
        frame_blocks.push(Block {
            kind: BlockKind::Frame,
            id: frame.id.clone(),
            key: format!("frame:{}", frame.id),
            members,
            options: pad(FRAME_PADDING + header.height, FRAME_PADDING, header.width),
        });
    }

    let list: Vec<Block> = zone_blocks.into_iter().chain(frame_blocks).collect();
    let parent: Vec<Option<usize>> = (0..list.len())
        .map(|inner| {
            let around: Vec<usize> = (0..list.len())
                .filter(|&outer| holds(&list, frames, outer, inner))
                .collect();
            innermost(&list, frames, &around)
        })
        .collect();
    let mut of_node = HashMap::new();
    for id in ids {
        let around: Vec<usize> = list
            .iter()
            .enumerate()
            .filter(|(_, b)| b.members.contains(*id))
            .map(|(index, _)| index)
            .collect();
        of_node.insert((*id).to_owned(), innermost(&list, frames, &around));
    }
    Blocks {
        list,
        parent,
        of_node,
    }
}

/// Whether block `outer` goes around block `inner`.
fn holds(list: &[Block], frames: &FrameAnalysis, outer: usize, inner: usize) -> bool {
    let (Some(o), Some(i)) = (list.get(outer), list.get(inner)) else {
        return false;
    };
    if outer == inner || i.members.iter().any(|id| !o.members.contains(id)) {
        return false;
    }
    if i.members.len() < o.members.len() {
        return true;
    }
    if o.kind == BlockKind::Frame && i.kind == BlockKind::Zone {
        return true;
    }
    o.kind == BlockKind::Frame && is_ancestor_frame(frames, &o.id, &i.id)
}

fn is_ancestor_frame(frames: &FrameAnalysis, outer: &str, inner: &str) -> bool {
    let mut up = frames.frame_of.get(inner);
    while let Some(frame) = up {
        if frame == outer {
            return true;
        }
        up = frames.frame_of.get(frame);
    }
    false
}

/// Of blocks around something, the innermost: the one all the others hold.
fn innermost(list: &[Block], frames: &FrameAnalysis, around: &[usize]) -> Option<usize> {
    around.iter().copied().find(|&b| {
        around
            .iter()
            .all(|&other| other == b || holds(list, frames, other, b))
    })
}

fn pad(top: f32, side: f32, min_width: f32) -> Value {
    json!({
        "elk.padding": format!("[top={top},left={side},bottom={side},right={side}]"),
        "elk.nodeSize.constraints": "[MINIMUM_SIZE]",
        "elk.nodeSize.minimum": format!("({min_width}, 0)"),
    })
}
