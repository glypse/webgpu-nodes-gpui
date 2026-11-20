//! A node editor canvas for GPUI: nodes with input and output ports, links
//! between them, panning, zooming, selection and value scrubbing.
//!
//! [`Flow`] is controlled: it shows the nodes and edges it's given and reports
//! edits as [`FlowEvent`]s, which the owner applies to its model before
//! handing the result back with [`Flow::set_graph`].

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, Context, DispatchPhase, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, Styled as _, Window, canvas, div, hsla, point, px,
    quad, size,
};

pub type NodeId = u64;

/// Layout in graph units; everything scales with the zoom.
const WIDTH: f32 = 160.0;
const HEADER: f32 = 26.0;
const ROW: f32 = 22.0;
const FOOTER: f32 = 6.0;
const PORT_RADIUS: f32 = 5.0;
const PORT_HIT_RADIUS: f32 = 10.0;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node: NodeId,
    pub port: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlowEdge {
    pub from: PortRef,
    pub to: PortRef,
}

#[derive(Clone, Debug)]
pub struct FlowPort {
    pub label: SharedString,
    /// For inputs: the value shown, and scrubbable, while unlinked.
    pub value: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct FlowNode {
    pub id: NodeId,
    /// Top-left corner in graph units.
    pub position: Point<f32>,
    pub title: SharedString,
    pub color: Hsla,
    pub inputs: Vec<FlowPort>,
    pub outputs: Vec<FlowPort>,
}

impl FlowNode {
    fn height(&self) -> f32 {
        HEADER + ROW * (self.outputs.len() + self.inputs.len()) as f32 + FOOTER
    }

    fn contains(&self, p: Point<f32>) -> bool {
        p.x >= self.position.x
            && p.x <= self.position.x + WIDTH
            && p.y >= self.position.y
            && p.y <= self.position.y + self.height()
    }

    /// Row centres, relative to the node: outputs first, then inputs.
    fn output_y(&self, port: usize) -> f32 {
        HEADER + ROW * port as f32 + ROW / 2.0
    }

    fn input_y(&self, port: usize) -> f32 {
        HEADER + ROW * (self.outputs.len() + port) as f32 + ROW / 2.0
    }

    fn output_anchor(&self, port: usize) -> Point<f32> {
        point(
            self.position.x + WIDTH,
            self.position.y + self.output_y(port),
        )
    }

    fn input_anchor(&self, port: usize) -> Point<f32> {
        point(self.position.x, self.position.y + self.input_y(port))
    }
}

#[derive(Clone, Debug)]
pub enum FlowEvent {
    Connected(FlowEdge),
    Disconnected(PortRef),
    NodesMoved(Vec<(NodeId, Point<f32>)>),
    Deleted(Vec<NodeId>),
    ValueChanged {
        port: PortRef,
        value: f32,
    },
    /// A node was double-clicked.
    Opened(NodeId),
    /// A right click on the background, at a graph and a window position.
    ContextMenu {
        graph: Point<f32>,
        window: Point<Pixels>,
    },
}

enum Drag {
    None,
    Pan {
        last: Point<f32>,
    },
    Nodes {
        last: Point<f32>,
        moved: bool,
    },
    Connect {
        from: PortRef,
        cursor: Point<f32>,
    },
    Scrub {
        port: PortRef,
        start_x: f32,
        start_value: f32,
    },
    Select {
        start: Point<f32>,
        current: Point<f32>,
    },
}

enum Hit {
    Output(PortRef),
    Input(PortRef),
    Value(PortRef, f32),
    Node(NodeId),
    Background,
}

pub struct Flow {
    nodes: Vec<FlowNode>,
    edges: Vec<FlowEdge>,
    selection: HashSet<NodeId>,
    /// Screen offset of the graph origin from the canvas' top-left corner.
    pan: Point<f32>,
    zoom: f32,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Drag,
    focus: FocusHandle,
}

impl EventEmitter<FlowEvent> for Flow {}

impl Focusable for Flow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Flow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            selection: HashSet::new(),
            pan: point(500.0, 350.0),
            zoom: 1.0,
            bounds: Rc::default(),
            drag: Drag::None,
            focus: cx.focus_handle(),
        }
    }

    /// Replaces what's shown, keeping the selection of nodes still there.
    pub fn set_graph(
        &mut self,
        nodes: Vec<FlowNode>,
        edges: Vec<FlowEdge>,
        cx: &mut Context<Self>,
    ) {
        self.selection
            .retain(|id| nodes.iter().any(|n| n.id == *id));
        self.nodes = nodes;
        self.edges = edges;
        cx.notify();
    }

    pub fn selection(&self) -> Vec<NodeId> {
        self.selection.iter().copied().collect()
    }

    pub fn select(&mut self, ids: impl IntoIterator<Item = NodeId>, cx: &mut Context<Self>) {
        self.selection = ids.into_iter().collect();
        cx.notify();
    }

    fn node(&self, id: NodeId) -> Option<&FlowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn linked(&self, to: PortRef) -> Option<&FlowEdge> {
        self.edges.iter().find(|e| e.to == to)
    }

    /// Canvas-local pixels from window pixels.
    fn local(&self, p: Point<Pixels>) -> Point<f32> {
        let origin = self.bounds.get().origin;
        point(f32::from(p.x - origin.x), f32::from(p.y - origin.y))
    }

    fn to_graph(&self, local: Point<f32>) -> Point<f32> {
        point(
            (local.x - self.pan.x) / self.zoom,
            (local.y - self.pan.y) / self.zoom,
        )
    }

    fn to_local(&self, graph: Point<f32>) -> Point<f32> {
        point(
            graph.x * self.zoom + self.pan.x,
            graph.y * self.zoom + self.pan.y,
        )
    }

    fn hit(&self, g: Point<f32>) -> Hit {
        let near = |a: Point<f32>| (a.x - g.x).hypot(a.y - g.y) <= PORT_HIT_RADIUS;
        for node in self.nodes.iter().rev() {
            for port in 0..node.outputs.len() {
                if near(node.output_anchor(port)) {
                    return Hit::Output(PortRef {
                        node: node.id,
                        port,
                    });
                }
            }
            for port in 0..node.inputs.len() {
                if near(node.input_anchor(port)) {
                    return Hit::Input(PortRef {
                        node: node.id,
                        port,
                    });
                }
            }
            if node.contains(g) {
                let row = (g.y - node.position.y - HEADER) / ROW;
                let input = row.floor() as isize - node.outputs.len() as isize;
                if let Ok(port) = usize::try_from(input) {
                    let port_ref = PortRef {
                        node: node.id,
                        port,
                    };
                    if let Some(value) = node.inputs.get(port).and_then(|p| p.value)
                        && self.linked(port_ref).is_none()
                    {
                        return Hit::Value(port_ref, value);
                    }
                }
                return Hit::Node(node.id);
            }
        }
        Hit::Background
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let local = self.local(event.position);
        let g = self.to_graph(local);
        match event.button {
            MouseButton::Middle => self.drag = Drag::Pan { last: local },
            MouseButton::Right => {
                if let Hit::Background = self.hit(g) {
                    cx.emit(FlowEvent::ContextMenu {
                        graph: g,
                        window: event.position,
                    });
                }
            }
            MouseButton::Left => match self.hit(g) {
                Hit::Output(from) => self.drag = Drag::Connect { from, cursor: g },
                Hit::Input(to) => {
                    // Picking up a link's end to move it elsewhere.
                    if let Some(edge) = self.linked(to).copied() {
                        self.edges.retain(|e| e.to != to);
                        cx.emit(FlowEvent::Disconnected(to));
                        self.drag = Drag::Connect {
                            from: edge.from,
                            cursor: g,
                        };
                    }
                }
                Hit::Value(port, value) => {
                    self.drag = Drag::Scrub {
                        port,
                        start_x: local.x,
                        start_value: value,
                    }
                }
                Hit::Node(id) => {
                    if event.click_count == 2 {
                        cx.emit(FlowEvent::Opened(id));
                        return;
                    }
                    if event.modifiers.shift {
                        if !self.selection.remove(&id) {
                            self.selection.insert(id);
                        }
                    } else if !self.selection.contains(&id) {
                        self.selection = HashSet::from([id]);
                    }
                    // Dragged nodes are drawn on top.
                    if let Some(index) = self.nodes.iter().position(|n| n.id == id) {
                        let node = self.nodes.remove(index);
                        self.nodes.push(node);
                    }
                    self.drag = Drag::Nodes {
                        last: g,
                        moved: false,
                    };
                }
                Hit::Background => {
                    if !event.modifiers.shift {
                        self.selection.clear();
                    }
                    self.drag = Drag::Select {
                        start: g,
                        current: g,
                    };
                }
            },
            MouseButton::Navigate(_) => {}
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let local = self.local(event.position);
        let g = self.to_graph(local);
        match &mut self.drag {
            Drag::None => return,
            Drag::Pan { last } => {
                self.pan = point(self.pan.x + local.x - last.x, self.pan.y + local.y - last.y);
                *last = local;
            }
            Drag::Nodes { last, moved } => {
                let delta = point(g.x - last.x, g.y - last.y);
                *last = g;
                *moved = true;
                for node in &mut self.nodes {
                    if self.selection.contains(&node.id) {
                        node.position = point(node.position.x + delta.x, node.position.y + delta.y);
                    }
                }
            }
            Drag::Connect { cursor, .. } => *cursor = g,
            Drag::Scrub {
                port,
                start_x,
                start_value,
            } => {
                let speed = if event.modifiers.shift { 0.001 } else { 0.01 };
                let value = *start_value + (local.x - *start_x) * speed;
                let value = (value * 1000.0).round() / 1000.0;
                let port = *port;
                if let Some(input) = self
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == port.node)
                    .and_then(|n| n.inputs.get_mut(port.port))
                {
                    input.value = Some(value);
                }
                cx.emit(FlowEvent::ValueChanged { port, value });
            }
            Drag::Select { current, .. } => *current = g,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let g = self.to_graph(self.local(event.position));
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Nodes { moved: true, .. } => {
                let moved = self
                    .nodes
                    .iter()
                    .filter(|n| self.selection.contains(&n.id))
                    .map(|n| (n.id, n.position))
                    .collect();
                cx.emit(FlowEvent::NodesMoved(moved));
            }
            Drag::Connect { from, .. } => {
                if let Hit::Input(to) | Hit::Value(to, _) = self.hit(g)
                    && to.node != from.node
                {
                    cx.emit(FlowEvent::Connected(FlowEdge { from, to }));
                }
            }
            Drag::Select { start, current } => {
                let (min, max) = (
                    point(start.x.min(current.x), start.y.min(current.y)),
                    point(start.x.max(current.x), start.y.max(current.y)),
                );
                for node in &self.nodes {
                    let p = node.position;
                    if p.x >= min.x
                        && p.y >= min.y
                        && p.x + WIDTH <= max.x
                        && p.y + node.height() <= max.y
                    {
                        self.selection.insert(node.id);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let zoom_by = match event.delta {
            ScrollDelta::Lines(lines) => Some(lines.y * 0.1),
            ScrollDelta::Pixels(delta) if event.modifiers.platform || event.modifiers.control => {
                Some(f32::from(delta.y) * 0.01)
            }
            ScrollDelta::Pixels(delta) => {
                self.pan = point(
                    self.pan.x + f32::from(delta.x),
                    self.pan.y + f32::from(delta.y),
                );
                None
            }
        };
        if let Some(by) = zoom_by {
            // Zoom around the cursor: the graph point under it stays put.
            let local = self.local(event.position);
            let anchor = self.to_graph(local);
            self.zoom = (self.zoom * (1.0 + by)).clamp(MIN_ZOOM, MAX_ZOOM);
            self.pan = point(
                local.x - anchor.x * self.zoom,
                local.y - anchor.y * self.zoom,
            );
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "backspace" | "delete" if !self.selection.is_empty() => {
                cx.emit(FlowEvent::Deleted(self.selection()));
            }
            "a" if event.keystroke.modifiers.platform => {
                self.selection = self.nodes.iter().map(|n| n.id).collect();
                cx.notify();
            }
            _ => {}
        }
    }

    fn render_node(&self, node: &FlowNode) -> impl IntoElement + use<> {
        let z = self.zoom;
        let origin = self.to_local(node.position);
        let selected = self.selection.contains(&node.id);
        let text = hsla(0.0, 0.0, 0.88, 1.0);
        let row = || div().h(px(ROW * z)).px(px(10.0 * z)).flex().items_center();
        let port_dot = |x: f32, y: f32, color: Hsla| {
            div()
                .absolute()
                .left(px((x - PORT_RADIUS) * z))
                .top(px((y - PORT_RADIUS) * z))
                .size(px(PORT_RADIUS * 2.0 * z))
                .rounded_full()
                .bg(color)
                .border(px(z))
                .border_color(hsla(0.0, 0.0, 0.1, 1.0))
        };
        let socket = hsla(0.58, 0.15, 0.65, 1.0);

        div()
            .absolute()
            .left(px(origin.x))
            .top(px(origin.y))
            .w(px(WIDTH * z))
            .h(px(node.height() * z))
            .rounded(px(6.0 * z))
            .bg(hsla(0.0, 0.0, 0.19, 0.97))
            .border(px(z.max(1.0)))
            .border_color(if selected {
                hsla(0.0, 0.0, 0.95, 1.0)
            } else {
                hsla(0.0, 0.0, 0.08, 1.0)
            })
            .shadow_md()
            .text_size(px(12.0 * z))
            .text_color(text)
            .child(
                div()
                    .h(px(HEADER * z))
                    .px(px(8.0 * z))
                    .flex()
                    .items_center()
                    .rounded_t(px(6.0 * z))
                    .bg(node.color)
                    .child(node.title.clone()),
            )
            .children(
                node.outputs
                    .iter()
                    .map(|port| row().justify_end().child(port.label.clone())),
            )
            .children(node.inputs.iter().enumerate().map(|(i, port)| {
                let linked = self
                    .linked(PortRef {
                        node: node.id,
                        port: i,
                    })
                    .is_some();
                match port.value {
                    Some(value) if !linked => row().child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_between()
                            .px(px(6.0 * z))
                            .rounded(px(3.0 * z))
                            .bg(hsla(0.0, 0.0, 0.29, 1.0))
                            .child(port.label.clone())
                            .child(format!("{value:.3}")),
                    ),
                    _ => row().child(port.label.clone()),
                }
            }))
            .children((0..node.outputs.len()).map(|i| port_dot(WIDTH, node.output_y(i), socket)))
            .children((0..node.inputs.len()).map(|i| port_dot(0.0, node.input_y(i), socket)))
    }
}

impl Render for Flow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let link = |a: Point<f32>, b: Point<f32>| (self.to_local(a), self.to_local(b));
        let mut links: Vec<(Point<f32>, Point<f32>)> = self
            .edges
            .iter()
            .filter_map(|e| {
                let from = self.node(e.from.node)?.output_anchor(e.from.port);
                let to = self.node(e.to.node)?.input_anchor(e.to.port);
                Some(link(from, to))
            })
            .collect();
        let mut marquee = None;
        match &self.drag {
            Drag::Connect { from, cursor } => {
                if let Some(node) = self.node(from.node) {
                    links.push(link(node.output_anchor(from.port), *cursor));
                }
            }
            Drag::Select { start, current } => {
                marquee = Some((self.to_local(*start), self.to_local(*current)))
            }
            _ => {}
        }

        let zoom = self.zoom;
        let bounds = self.bounds.clone();
        let entity = cx.entity().downgrade();
        let edges = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, _| {
                let at = |p: Point<f32>| point(b.origin.x + px(p.x), b.origin.y + px(p.y));
                for (from, to) in links {
                    let bend = ((to.x - from.x).abs() * 0.5).max(40.0 * zoom);
                    let mut path = PathBuilder::stroke(px(2.0 * zoom));
                    path.move_to(at(from));
                    path.cubic_bezier_to(
                        at(to),
                        at(point(from.x + bend, from.y)),
                        at(point(to.x - bend, to.y)),
                    );
                    if let Ok(path) = path.build() {
                        window.paint_path(path, hsla(0.0, 0.0, 0.72, 1.0));
                    }
                }
                if let Some((a, c)) = marquee {
                    let min = at(point(a.x.min(c.x), a.y.min(c.y)));
                    let extent = size(px((a.x - c.x).abs()), px((a.y - c.y).abs()));
                    window.paint_quad(quad(
                        Bounds::new(min, extent),
                        px(2.0),
                        hsla(0.6, 0.6, 0.6, 0.12),
                        px(1.0),
                        hsla(0.6, 0.6, 0.7, 0.8),
                        BorderStyle::Solid,
                    ));
                }
                // Drags follow the mouse beyond the canvas.
                let on_move = entity.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        on_move.update(cx, |this, cx| this.mouse_move(e, cx)).ok();
                    }
                });
                let on_up = entity.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        on_up.update(cx, |this, cx| this.mouse_up(e, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .id("flow")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(hsla(0.0, 0.0, 0.12, 1.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e, w, cx| this.mouse_down(e, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, e, w, cx| this.mouse_down(e, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e, w, cx| this.mouse_down(e, w, cx)),
            )
            .on_scroll_wheel(cx.listener(|this, e, _, cx| this.scroll(e, cx)))
            .on_key_down(cx.listener(|this, e, _, cx| this.key_down(e, cx)))
            .child(edges)
            .children(self.nodes.iter().map(|n| self.render_node(n)))
    }
}
