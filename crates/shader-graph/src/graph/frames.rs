//! Frames: labelled boxes around other nodes, for reading only. What a frame
//! holds is each node's `frame`; nesting and the boxes are derived.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use super::{GraphNode, Point};
use crate::types::NodeData;

/// The type of frame nodes.
pub const LAYOUT_FRAME: &str = "layout-frame";

/// Distance between a frame's border and what it holds.
pub const FRAME_PADDING: f32 = 24.0;

/// Room an empty frame keeps under its header, to drop nodes in.
pub const EMPTY_FRAME_SIZE: Size = Size {
    width: 240.0,
    height: 120.0,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }
}

/// The smallest box holding every one of `boxes`.
pub fn union_of(boxes: &[Bounds]) -> Option<Bounds> {
    let (first, rest) = boxes.split_first()?;
    let (mut left, mut top, mut right, mut bottom) =
        (first.x, first.y, first.right(), first.bottom());
    for b in rest {
        left = left.min(b.x);
        top = top.min(b.y);
        right = right.max(b.right());
        bottom = bottom.max(b.bottom());
    }
    Some(Bounds {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// Colours a frame can be drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameTint {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
}

impl FrameTint {
    pub const ALL: [FrameTint; 8] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    pub fn label(self) -> String {
        let id = self.id();
        let mut chars = id.chars();
        chars
            .next()
            .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
    }
}

/// The tint of a frame node's data.
pub fn frame_tint(data: &NodeData) -> Option<FrameTint> {
    data.extra
        .get("tint")
        .and_then(Value::as_str)
        .and_then(FrameTint::from_id)
}

/// The note of a frame node's data.
pub fn frame_note(data: &NodeData) -> Option<&str> {
    data.extra
        .get("note")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
}

pub fn set_frame_tint(data: &mut NodeData, tint: Option<FrameTint>) {
    match tint {
        Some(t) => data.extra.insert("tint".to_owned(), Value::from(t.id())),
        None => data.extra.remove("tint"),
    };
}

pub fn set_frame_note(data: &mut NodeData, note: Option<&str>) {
    match note.filter(|n| !n.is_empty()) {
        Some(n) => data.extra.insert("note".to_owned(), Value::from(n)),
        None => data.extra.remove("note"),
    };
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub id: String,
    /// The frame directly enclosing it.
    pub parent: Option<String>,
    /// 0 for frames in no other frame.
    pub depth: usize,
    /// Nodes directly in the frame, nested frames included.
    pub children: Vec<String>,
    /// Every node in the frame at any depth, nested frames included.
    pub descendants: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameAnalysis {
    pub frames: HashMap<String, Frame>,
    /// The frame directly holding each node that's in one.
    pub frame_of: HashMap<String, String>,
}

impl FrameAnalysis {
    /// The frames around a node, innermost first.
    pub fn frames_around(&self, node_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut up = self.frame_of.get(node_id);
        while let Some(frame) = up {
            if chain.contains(frame) {
                break;
            }
            chain.push(frame.clone());
            up = self.frame_of.get(frame);
        }
        chain
    }

    /// The innermost frame holding every one of `node_ids`, or `None` when they share none.
    pub fn common_frame<'a>(&self, node_ids: impl IntoIterator<Item = &'a str>) -> Option<String> {
        let mut common: Option<Vec<String>> = None;
        for id in node_ids {
            let chain = self.frames_around(id);
            common = Some(match common {
                Some(c) => c.into_iter().filter(|f| chain.contains(f)).collect(),
                None => chain,
            });
        }
        common?.into_iter().next()
    }

    /// The nodes of `ids` in none of the others' frames: moving those moves
    /// the rest along, inside them.
    pub fn outermost(&self, ids: &HashSet<String>) -> Vec<String> {
        ids.iter()
            .filter(|id| !self.frames_around(id).iter().any(|f| ids.contains(f)))
            .cloned()
            .collect()
    }
}

/// What the frame analysis needs of a node.
#[derive(Clone, Copy, Debug)]
pub struct FrameItem<'a> {
    pub id: &'a str,
    pub is_frame: bool,
    pub frame: Option<&'a str>,
}

impl<'a> From<&'a GraphNode> for FrameItem<'a> {
    fn from(n: &'a GraphNode) -> Self {
        Self {
            id: &n.id,
            is_frame: n.is(LAYOUT_FRAME),
            frame: n.frame.as_deref(),
        }
    }
}

/// Frames of a document and what they hold, from the nodes' `frame`.
pub fn analyze_frames(nodes: &[GraphNode]) -> FrameAnalysis {
    analyze_frame_items(nodes.iter().map(FrameItem::from))
}

/// Frames of some nodes and what they hold. A node naming no frame of the
/// document is in none, and so is a frame that would end up inside itself.
pub fn analyze_frame_items<'a>(items: impl IntoIterator<Item = FrameItem<'a>>) -> FrameAnalysis {
    let items: Vec<FrameItem> = items.into_iter().collect();
    let frame_ids: HashSet<&str> = items.iter().filter(|n| n.is_frame).map(|n| n.id).collect();
    let declared: HashMap<&str, &str> = items
        .iter()
        .filter_map(|n| {
            let frame = n.frame?;
            (frame != n.id && frame_ids.contains(frame)).then_some((n.id, frame))
        })
        .collect();

    let mut frame_of: HashMap<String, String> = HashMap::new();
    for (&id, &frame) in &declared {
        let mut seen = HashSet::new();
        let mut up = Some(frame);
        while let Some(current) = up {
            if current == id || !seen.insert(current) {
                break;
            }
            up = declared.get(current).copied();
        }
        // Only frames on a cycle are left out of theirs: what's in them stays.
        if up != Some(id) {
            frame_of.insert(id.to_owned(), frame.to_owned());
        }
    }

    let mut frames: HashMap<String, Frame> = frame_ids
        .iter()
        .map(|&id| {
            (
                id.to_owned(),
                Frame {
                    id: id.to_owned(),
                    ..Frame::default()
                },
            )
        })
        .collect();
    for node in &items {
        let Some(frame) = frame_of.get(node.id) else {
            continue;
        };
        if let Some(f) = frames.get_mut(frame) {
            f.children.push(node.id.to_owned());
        }
        let mut up = Some(frame);
        while let Some(current) = up {
            if let Some(f) = frames.get_mut(current) {
                f.descendants.insert(node.id.to_owned());
            }
            up = frame_of.get(current);
        }
    }
    for frame in frames.values_mut() {
        frame.parent = frame_of.get(&frame.id).cloned();
        let mut up = frame.parent.as_ref();
        while let Some(parent) = up {
            frame.depth += 1;
            up = frame_of.get(parent);
        }
    }
    FrameAnalysis { frames, frame_of }
}

/// A frame's header as drawn, from its label and note, when it can't be measured.
pub fn estimate_frame_header(label: &str, note: Option<&str>) -> Size {
    let lines: Vec<&str> = note.map_or_else(Vec::new, |n| n.split('\n').collect());
    let longest = lines
        .iter()
        .map(|l| l.chars().count() as f32 * 7.0)
        .fold(label.chars().count() as f32 * 9.0, f32::max);
    Size {
        width: longest.min(480.0) + 24.0,
        height: 36.0 + 18.0 * lines.len() as f32,
    }
}

/// The box of every frame: around what it holds (nested frames' boxes
/// included) and the boxes `around` gives it, its header on top. `rect_of`
/// gives the box of a node by id; an empty frame keeps its own position.
/// `header` is the size a frame's header is drawn at.
pub fn frame_rects(
    analysis: &FrameAnalysis,
    rect_of: impl Fn(&str) -> Option<Bounds>,
    header: impl Fn(&str) -> Size,
    around: impl Fn(&str) -> Vec<Bounds>,
) -> HashMap<String, Bounds> {
    let mut rects: HashMap<String, Bounds> = HashMap::new();
    let mut deepest_first: Vec<&Frame> = analysis.frames.values().collect();
    deepest_first.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| a.id.cmp(&b.id)));
    for frame in deepest_first {
        let Some(own) = rect_of(&frame.id) else {
            continue;
        };
        let head = header(&frame.id);
        let mut inner: Vec<Bounds> = frame
            .children
            .iter()
            .filter_map(|id| rects.get(id).copied().or_else(|| rect_of(id)))
            .collect();
        inner.extend(around(&frame.id));
        let rect = match union_of(&inner) {
            None => Bounds {
                x: own.x,
                y: own.y,
                width: head.width.max(EMPTY_FRAME_SIZE.width),
                height: head.height + EMPTY_FRAME_SIZE.height,
            },
            Some(b) => Bounds {
                x: b.x - FRAME_PADDING,
                y: b.y - FRAME_PADDING - head.height,
                width: (b.width + 2.0 * FRAME_PADDING).max(head.width),
                height: b.height + 2.0 * FRAME_PADDING + head.height,
            },
        };
        rects.insert(frame.id.clone(), rect);
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, ty: &str, frame: Option<&str>, at: (f32, f32)) -> GraphNode {
        let mut n = GraphNode::new(ty, Point::new(at.0, at.1));
        n.id = id.to_owned();
        n.frame = frame.map(str::to_owned);
        n
    }

    fn frame(id: &str, in_frame: Option<&str>) -> GraphNode {
        node(id, LAYOUT_FRAME, in_frame, (0.0, 0.0))
    }

    #[test]
    fn nests_frames_and_collects_what_they_hold() {
        let analysis = analyze_frames(&[
            frame("outer", None),
            frame("inner", Some("outer")),
            node("a", "sin", Some("inner"), (0.0, 0.0)),
            node("b", "sin", Some("outer"), (0.0, 0.0)),
        ]);
        let inner = &analysis.frames["inner"];
        assert_eq!(inner.parent.as_deref(), Some("outer"));
        assert_eq!(inner.depth, 1);
        assert_eq!(analysis.frames["outer"].children, ["inner", "b"]);
        let held: Vec<&str> = analysis.frames["outer"]
            .descendants
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(held, ["a", "b", "inner"]);
        assert_eq!(analysis.frame_of["a"], "inner");
    }

    #[test]
    fn ignores_missing_frames_and_takes_frames_on_a_cycle_out_of_theirs() {
        let analysis = analyze_frames(&[
            node("a", "sin", Some("gone"), (0.0, 0.0)),
            node("b", "sin", Some("x"), (0.0, 0.0)),
            frame("x", Some("y")),
            frame("y", Some("x")),
        ]);
        assert!(!analysis.frame_of.contains_key("a"));
        assert_eq!(analysis.frame_of["b"], "x");
        assert!(!analysis.frame_of.contains_key("x"));
        assert!(!analysis.frame_of.contains_key("y"));
    }

    fn rects_of<'a>(nodes: &'a [GraphNode]) -> impl Fn(&str) -> Option<Bounds> + 'a {
        |id| {
            let n = nodes.iter().find(|n| n.id == id)?;
            Some(Bounds {
                x: n.position.x,
                y: n.position.y,
                width: 50.0,
                height: 40.0,
            })
        }
    }

    #[test]
    fn a_frame_goes_around_what_it_holds_under_its_header() {
        let nodes = [
            frame("f", None),
            node("a", "sin", Some("f"), (100.0, 100.0)),
            node("b", "sin", Some("f"), (300.0, 200.0)),
        ];
        let rects = frame_rects(
            &analyze_frames(&nodes),
            rects_of(&nodes),
            |_| Size {
                width: 80.0,
                height: 30.0,
            },
            |_| Vec::new(),
        );
        assert_eq!(
            rects["f"],
            Bounds {
                x: 100.0 - FRAME_PADDING,
                y: 100.0 - FRAME_PADDING - 30.0,
                width: 250.0 + 2.0 * FRAME_PADDING,
                height: 140.0 + 2.0 * FRAME_PADDING + 30.0,
            }
        );
    }

    #[test]
    fn an_empty_frame_keeps_its_position() {
        let nodes = [node("f", LAYOUT_FRAME, None, (5.0, 6.0))];
        let rects = frame_rects(
            &analyze_frames(&nodes),
            rects_of(&nodes),
            |_| Size {
                width: 80.0,
                height: 30.0,
            },
            |_| Vec::new(),
        );
        assert_eq!(rects["f"].x, 5.0);
        assert_eq!(rects["f"].height, 30.0 + EMPTY_FRAME_SIZE.height);
    }

    #[test]
    fn nested_frames_are_boxed_around_the_inner_boxes() {
        let nodes = [
            frame("outer", None),
            frame("inner", Some("outer")),
            node("a", "sin", Some("inner"), (0.0, 0.0)),
        ];
        let rects = frame_rects(
            &analyze_frames(&nodes),
            rects_of(&nodes),
            |_| Size {
                width: 10.0,
                height: 30.0,
            },
            |_| Vec::new(),
        );
        assert!(rects["outer"].x < rects["inner"].x);
        assert!(rects["outer"].bottom() > rects["inner"].bottom());
    }

    #[test]
    fn the_common_frame_is_the_innermost_shared() {
        let analysis = analyze_frames(&[
            frame("outer", None),
            frame("inner", Some("outer")),
            node("a", "sin", Some("inner"), (0.0, 0.0)),
            node("b", "sin", Some("outer"), (0.0, 0.0)),
        ]);
        assert_eq!(analysis.common_frame(["a", "b"]), Some("outer".to_owned()));
        assert_eq!(analysis.common_frame(["a"]), Some("inner".to_owned()));
        assert_eq!(analysis.common_frame(["a", "zzz"]), None);
    }
}
