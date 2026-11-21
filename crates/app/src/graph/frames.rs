//! Frames: a header node (label and note) on top of a box drawn under the
//! nodes, both in the frame's tint. The box follows what the frame holds, as
//! the nodes are measured and dragged.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    AnyElement, App, BorderStyle, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Rgba, Styled as _, WeakEntity, div, px, size,
};
use gpui_flow::{Flow, Layer, LayerContext, NodeProps, Xy};
use shader_graph::graph::{
    Bounds, FrameAnalysis, FrameItem, FrameTint, Point as GraphPoint, Size,
    analyze_frame_items, estimate_frame_header, frame_rects, union_of,
};
use theme::{ActiveTheme as _, Theme, oklch};

use crate::editor::Editor;

/// What a frame's header shows.
#[derive(Clone)]
pub struct FrameView {
    pub label: String,
    pub note: Option<String>,
    pub tint: Option<FrameTint>,
}

/// The colour of a tint; untinted frames are drawn in the muted foreground.
pub fn tint_color(tint: Option<FrameTint>, theme: &Theme) -> Hsla {
    match tint {
        None => theme.colors.muted_foreground,
        Some(FrameTint::Red) => oklch(0.637, 0.237, 25.331),
        Some(FrameTint::Orange) => oklch(0.705, 0.213, 47.604),
        Some(FrameTint::Yellow) => oklch(0.795, 0.184, 86.047),
        Some(FrameTint::Green) => oklch(0.723, 0.219, 149.579),
        Some(FrameTint::Teal) => oklch(0.704, 0.14, 182.503),
        Some(FrameTint::Blue) => oklch(0.623, 0.214, 259.815),
        Some(FrameTint::Purple) => oklch(0.627, 0.265, 303.9),
        Some(FrameTint::Pink) => oklch(0.656, 0.241, 354.308),
    }
}

fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let lerp = |x: f32, y: f32| x * amount + y * (1.0 - amount);
    Rgba { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: lerp(a.a, b.a) }.into()
}

pub fn header(view: &FrameView, props: &NodeProps, ed: &WeakEntity<Editor>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = tint_color(view.tint, theme);
    let s = |v: f32| props.scaled(v);
    let (editor, id) = (ed.clone(), props.id.to_string());
    div()
        .id("frame-header")
        .flex()
        .flex_col()
        .gap(s(2.0))
        .max_w(s(448.0))
        .px(s(12.0))
        .pt(s(8.0))
        .pb(s(4.0))
        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
            if event.click_count == 2 {
                cx.stop_propagation();
                editor.update(cx, |e, cx| e.open_sheet(&id, cx)).ok();
            }
        })
        .child(
            div()
                .whitespace_nowrap()
                .text_size(s(14.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(mix(color, theme.colors.foreground, 0.8))
                .child(view.label.clone()),
        )
        .children(view.note.clone().map(|note| {
            div()
                .text_size(s(12.0))
                .text_color(theme.colors.muted_foreground)
                .child(note)
        }))
        .into_any_element()
}

/// A node as the frame boxes see it.
pub struct SnapshotItem {
    pub id: String,
    pub is_frame: bool,
    pub frame: Option<String>,
}

/// The bubble of a zone, which frames holding both its boundaries go around.
pub struct ZoneBox {
    pub input: String,
    pub output: String,
    pub members: Vec<String>,
    pub padding: f32,
}

pub struct FrameLook {
    pub tint: Option<FrameTint>,
    pub label: String,
    pub note: Option<String>,
}

/// Everything the boxes are drawn from but the nodes' places: taken from the
/// model whenever the editor syncs.
#[derive(Default)]
pub struct Snapshot {
    pub items: Vec<SnapshotItem>,
    pub analysis: FrameAnalysis,
    pub zones: Vec<ZoneBox>,
    pub looks: HashMap<String, FrameLook>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.analysis.frames.is_empty()
    }
}

/// A drag of nodes, frames taking what they hold along.
#[derive(Default)]
pub struct Drag {
    /// Where the nodes dragged were.
    pub from: HashMap<String, GraphPoint>,
    /// What dragged frames hold, not dragged itself, from where it was.
    pub inside: HashMap<String, GraphPoint>,
    /// Dragged frames and what they hold.
    pub moving: HashSet<String>,
    /// Dragged nodes not in a dragged frame: the drop decides their frame.
    pub loose: HashSet<String>,
    /// The frame the drop would put the loose nodes in.
    pub drop_frame: Option<String>,
}

pub type SharedDrag = Rc<RefCell<Option<Drag>>>;

impl Drag {
    /// `positions` are the nodes' places in the document.
    pub fn start(
        analysis: &FrameAnalysis,
        positions: &HashMap<String, GraphPoint>,
        dragged: &[String],
    ) -> Self {
        let set: HashSet<&String> = dragged.iter().collect();
        let mut inside = HashMap::new();
        for id in dragged {
            let Some(frame) = analysis.frames.get(id) else { continue };
            for held in &frame.descendants {
                if let (false, Some(p)) = (set.contains(held), positions.get(held)) {
                    inside.insert(held.clone(), *p);
                }
            }
        }
        let from: HashMap<String, GraphPoint> =
            dragged.iter().filter_map(|id| Some((id.clone(), *positions.get(id)?))).collect();
        let loose: HashSet<String> = dragged
            .iter()
            .filter(|id| !analysis.frames_around(id).iter().any(|f| set.contains(f)))
            .cloned()
            .collect();
        let moving = dragged.iter().cloned().chain(inside.keys().cloned()).collect();
        Self { from, inside, moving, loose, drop_frame: None }
    }
}

/// Where each frame is drawn, by id, and the analysis they came from. Nodes
/// of `loose` count as in no frame.
pub fn boxes(
    snapshot: &Snapshot,
    flow: &Flow,
    loose: &HashSet<String>,
) -> (HashMap<String, Bounds>, FrameAnalysis) {
    if snapshot.is_empty() {
        return (HashMap::new(), FrameAnalysis::default());
    }
    let analysis = if loose.is_empty() {
        snapshot.analysis.clone()
    } else {
        analyze_frame_items(snapshot.items.iter().map(|i| FrameItem {
            id: &i.id,
            is_frame: i.is_frame,
            frame: if loose.contains(&i.id) { None } else { i.frame.as_deref() },
        }))
    };
    let rects = node_rects(flow);
    let header = |id: &str| match flow.node_rect(&id.to_owned().into()) {
        Some(r) => Size { width: r.width, height: r.height },
        None => snapshot
            .looks
            .get(id)
            .map_or(Size::default(), |l| estimate_frame_header(&l.label, l.note.as_deref())),
    };
    let bubbles: Vec<(&ZoneBox, Bounds)> = snapshot
        .zones
        .iter()
        .filter_map(|zone| {
            let held: Vec<Bounds> = [&zone.input, &zone.output]
                .into_iter()
                .chain(&zone.members)
                .filter_map(|id| rects.get(id).copied())
                .collect();
            let b = union_of(&held)?;
            Some((zone, Bounds { y: b.y - zone.padding, height: b.height + 2.0 * zone.padding, ..b }))
        })
        .collect();
    let around = |id: &str| -> Vec<Bounds> {
        let Some(inside) = analysis.frames.get(id).map(|f| &f.descendants) else { return Vec::new() };
        bubbles
            .iter()
            .filter(|(z, _)| inside.contains(&z.input) && inside.contains(&z.output))
            .map(|(_, b)| *b)
            .collect()
    };
    let boxes = frame_rects(&analysis, |id| rects.get(id).copied(), header, around);
    (boxes, analysis)
}

/// Every node's box as the flow has it, from its live position and measured size.
pub fn node_rects(flow: &Flow) -> HashMap<String, Bounds> {
    flow.nodes()
        .iter()
        .map(|n| {
            let (width, height) = flow.node_rect(&n.id).map_or(ESTIMATED_NODE, |r| (r.width, r.height));
            (n.id.to_string(), Bounds { x: n.position.x, y: n.position.y, width, height })
        })
        .collect()
}

/// Node size to assume before it's measured.
const ESTIMATED_NODE: (f32, f32) = (180.0, 100.0);

/// The innermost frame drawn under `point`, frames of `excluded` aside.
pub fn frame_at(
    boxes: &HashMap<String, Bounds>,
    analysis: &FrameAnalysis,
    point: GraphPoint,
    excluded: &HashSet<String>,
) -> Option<String> {
    let mut found: Option<(&String, usize)> = None;
    let mut ids: Vec<&String> = boxes.keys().collect();
    ids.sort();
    for id in ids {
        let depth = analysis.frames.get(id).map_or(0, |f| f.depth);
        if excluded.contains(id) || !boxes[id].contains(point) || found.is_some_and(|(_, d)| d >= depth) {
            continue;
        }
        found = Some((id, depth));
    }
    found.map(|(id, _)| id.clone())
}

/// The frame each node of `node_ids` (but those in frames among them) is
/// dropped in: the innermost its center is on, frames of `moving` aside.
/// Nodes grouped by frame, `None` for none.
pub fn drop_targets(
    analysis: &FrameAnalysis,
    boxes: &HashMap<String, Bounds>,
    rects: &HashMap<String, Bounds>,
    node_ids: &[String],
    moving: &HashSet<String>,
) -> Vec<(Option<String>, Vec<String>)> {
    let ids: HashSet<&String> = node_ids.iter().collect();
    let mut targets: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for id in node_ids {
        if analysis.frames_around(id).iter().any(|f| ids.contains(f)) {
            continue;
        }
        let Some(rect) = boxes.get(id).or_else(|| rects.get(id)) else { continue };
        let center = rect.center();
        let target = frame_at(boxes, analysis, center, moving);
        match targets.iter_mut().find(|(t, _)| *t == target) {
            Some((_, list)) => list.push(id.clone()),
            None => targets.push((target, vec![id.clone()])),
        }
    }
    targets
}

/// A back layer painting the frames' boxes, outermost first.
pub fn layer(flow: WeakEntity<Flow>, snapshot: Rc<Snapshot>, drag: SharedDrag) -> Layer {
    Rc::new(move |ctx: &LayerContext, window, cx| {
        let Some(flow) = flow.upgrade() else { return };
        let flow = flow.read(cx);
        let (loose, drop_frame) = match &*drag.borrow() {
            Some(d) => (d.loose.clone(), d.drop_frame.clone()),
            None => (HashSet::new(), None),
        };
        let (boxes, analysis) = boxes(&snapshot, flow, &loose);
        let mut ordered: Vec<(&String, &Bounds)> = boxes.iter().collect();
        ordered.sort_by_key(|(id, _)| (analysis.frames.get(*id).map_or(0, |f| f.depth), (*id).clone()));
        let theme = cx.theme().clone();
        let zoom = ctx.viewport.zoom;
        for (id, b) in ordered {
            let tint = snapshot.looks.get(id).and_then(|l| l.tint);
            let color = tint_color(tint, &theme);
            let dropping = drop_frame.as_ref() == Some(id);
            let selected = flow.nodes().iter().any(|n| n.selected && n.id.as_ref() == id.as_str());
            let bounds = gpui::Bounds {
                origin: ctx.to_window(Xy::new(b.x, b.y)),
                size: size(px(b.width * zoom), px(b.height * zoom)),
            };
            let (background, border, style) = if dropping {
                (color.opacity(0.2), color, BorderStyle::Dashed)
            } else if selected {
                (color.opacity(0.1), theme.colors.primary, BorderStyle::Solid)
            } else {
                (color.opacity(0.1), color.opacity(0.35), BorderStyle::Solid)
            };
            window.paint_quad(gpui::quad(
                bounds,
                theme.radius.md * zoom,
                background,
                px(1.0),
                border,
                style,
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_graph::graph::{GraphNode, LAYOUT_FRAME, analyze_frames};

    fn node(id: &str, frame_type: bool, frame: Option<&str>) -> GraphNode {
        let mut n = GraphNode::new(if frame_type { LAYOUT_FRAME } else { "sin" }, GraphPoint::default());
        n.id = id.to_owned();
        n.frame = frame.map(str::to_owned);
        n
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
        Bounds { x, y, width: w, height: h }
    }

    #[test]
    fn the_innermost_frame_under_a_point_takes_a_drop() {
        let nodes = [node("outer", true, None), node("inner", true, Some("outer"))];
        let analysis = analyze_frames(&nodes);
        let boxes = HashMap::from([
            ("outer".to_owned(), bounds(0.0, 0.0, 100.0, 100.0)),
            ("inner".to_owned(), bounds(10.0, 10.0, 30.0, 30.0)),
        ]);
        let none = HashSet::new();
        assert_eq!(frame_at(&boxes, &analysis, GraphPoint::new(20.0, 20.0), &none).as_deref(), Some("inner"));
        assert_eq!(frame_at(&boxes, &analysis, GraphPoint::new(80.0, 80.0), &none).as_deref(), Some("outer"));
        assert_eq!(frame_at(&boxes, &analysis, GraphPoint::new(200.0, 0.0), &none), None);
        let moving = HashSet::from(["inner".to_owned()]);
        assert_eq!(frame_at(&boxes, &analysis, GraphPoint::new(20.0, 20.0), &moving).as_deref(), Some("outer"));
    }

    #[test]
    fn dragging_a_frame_takes_what_it_holds_and_leaves_the_frame_to_the_drop() {
        let nodes = [node("f", true, None), node("a", false, Some("f")), node("b", false, None)];
        let analysis = analyze_frames(&nodes);
        let positions: HashMap<String, GraphPoint> =
            nodes.iter().map(|n| (n.id.clone(), n.position)).collect();
        let drag = Drag::start(&analysis, &positions, &["f".to_owned(), "b".to_owned()]);
        assert_eq!(drag.inside.keys().collect::<Vec<_>>(), ["a"]);
        assert!(drag.moving.contains("a") && drag.moving.contains("f"));
        assert!(drag.loose.contains("f") && drag.loose.contains("b"));
        let in_frame = Drag::start(&analysis, &positions, &["a".to_owned()]);
        assert!(in_frame.loose.contains("a"));
        let both = Drag::start(&analysis, &positions, &["f".to_owned(), "a".to_owned()]);
        assert!(!both.loose.contains("a"));
    }

    #[test]
    fn nodes_are_dropped_by_their_centre_and_those_in_dragged_frames_stay() {
        let nodes = [node("f", true, None), node("a", false, None), node("b", false, None)];
        let analysis = analyze_frames(&nodes);
        let boxes = HashMap::from([("f".to_owned(), bounds(0.0, 0.0, 100.0, 100.0))]);
        let rects = HashMap::from([
            ("a".to_owned(), bounds(10.0, 10.0, 20.0, 20.0)),
            ("b".to_owned(), bounds(300.0, 10.0, 20.0, 20.0)),
        ]);
        let targets = drop_targets(
            &analysis,
            &boxes,
            &rects,
            &["a".to_owned(), "b".to_owned()],
            &HashSet::from(["a".to_owned(), "b".to_owned()]),
        );
        assert!(targets.contains(&(Some("f".to_owned()), vec!["a".to_owned()])));
        assert!(targets.contains(&(None, vec!["b".to_owned()])));
    }
}
