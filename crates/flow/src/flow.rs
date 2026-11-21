use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, Styled as _, WeakEntity, Window, canvas, div, point, prelude::FluentBuilder as _, px, size,
};

use crate::background::BackgroundOptions;
use crate::edge_path::{
    DEFAULT_CURVATURE, EdgePath, PathParams, Position, StepOptions, bezier_path, flatten,
    smooth_step_path, straight_path,
};
use crate::geometry::{
    Extent, Rect, Viewport, Xy, auto_pan, bounds_of, clamp_position, constrain_viewport,
    snap_position, viewport_for_bounds, wheel_exponent,
};
use crate::lookup::{
    EdgeEnds, InternalNode, Lookup, closest_handle, edge_ends, find_handle, handle_under,
    validate,
};
use crate::model::{
    Connection, ConnectionMode, ConnectionState, Edge, EdgeChange, EdgeKind, EdgeStyle,
    HandleBounds, HandleType, Id, Node, NodeChange, NodeKind,
};
use crate::node::{Measure, NodeConnections, NodeMeasure, NodeProps};
use crate::panels::{ControlsOptions, MiniMapOptions, Panel};
use crate::theme::FlowTheme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    /// Nodes must be inside the box entirely.
    #[default]
    Full,
    /// Touching the box is enough.
    Partial,
}

#[derive(Clone, Copy, Debug)]
pub struct FitViewOptions {
    pub padding: f32,
    pub min_zoom: Option<f32>,
    pub max_zoom: Option<f32>,
    pub duration: Option<Duration>,
    pub include_hidden: bool,
}

impl Default for FitViewOptions {
    fn default() -> Self {
        Self { padding: 0.1, min_zoom: None, max_zoom: None, duration: None, include_hidden: false }
    }
}

pub type ConnectionValidator = Rc<dyn Fn(&Connection) -> bool>;
pub type ConnectionLineStyle = Rc<dyn Fn(&ConnectionState) -> EdgeStyle>;

#[derive(Clone)]
pub struct FlowOptions {
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub translate_extent: Extent,
    pub node_extent: Extent,
    pub snap_grid: Option<(f32, f32)>,
    pub nodes_draggable: bool,
    pub nodes_connectable: bool,
    pub elements_selectable: bool,
    pub pan_on_drag: bool,
    pub zoom_on_scroll: bool,
    pub zoom_on_pinch: bool,
    pub pan_on_scroll: bool,
    pub pan_on_scroll_speed: f32,
    pub zoom_on_double_click: bool,
    /// Dragging on the pane draws a selection box instead of panning.
    pub selection_on_drag: bool,
    pub selection_mode: SelectionMode,
    pub connection_mode: ConnectionMode,
    /// How close, in flow units, a handle must be to take a dropped link.
    pub connection_radius: f32,
    pub auto_pan_on_connect: bool,
    pub auto_pan_on_node_drag: bool,
    pub auto_pan_speed: f32,
    /// Pixels the pointer must move before a node drag starts.
    pub node_drag_threshold: f32,
    pub select_nodes_on_drag: bool,
    /// Backspace and Delete emit `FlowEvent::Delete`.
    pub delete_key: bool,
    pub fit_view_on_init: bool,
    pub fit_view_options: FitViewOptions,
    pub elevate_nodes_on_select: bool,
    pub elevate_edges_on_select: bool,
    pub edges_reconnectable: bool,
    pub reconnect_radius: f32,
    pub default_edge_kind: EdgeKind,
    pub default_edge_style: Option<EdgeStyle>,
    pub is_valid_connection: Option<ConnectionValidator>,
    /// Colours the connection line while it's dragged.
    pub connection_line_style: Option<ConnectionLineStyle>,
    pub theme: FlowTheme,
    pub background: Option<BackgroundOptions>,
    pub controls: Option<ControlsOptions>,
    pub minimap: Option<MiniMapOptions>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            min_zoom: 0.5,
            max_zoom: 2.0,
            translate_extent: Extent::INFINITE,
            node_extent: Extent::INFINITE,
            snap_grid: None,
            nodes_draggable: true,
            nodes_connectable: true,
            elements_selectable: true,
            pan_on_drag: true,
            zoom_on_scroll: true,
            zoom_on_pinch: true,
            pan_on_scroll: false,
            pan_on_scroll_speed: 0.5,
            zoom_on_double_click: true,
            selection_on_drag: false,
            selection_mode: SelectionMode::Full,
            connection_mode: ConnectionMode::Strict,
            connection_radius: 20.0,
            auto_pan_on_connect: true,
            auto_pan_on_node_drag: true,
            auto_pan_speed: 15.0,
            node_drag_threshold: 1.0,
            select_nodes_on_drag: true,
            delete_key: true,
            fit_view_on_init: false,
            fit_view_options: FitViewOptions::default(),
            elevate_nodes_on_select: true,
            elevate_edges_on_select: false,
            edges_reconnectable: false,
            reconnect_radius: 10.0,
            default_edge_kind: EdgeKind::Bezier,
            default_edge_style: None,
            is_valid_connection: None,
            connection_line_style: None,
            theme: FlowTheme::dark(),
            background: Some(BackgroundOptions::default()),
            controls: None,
            minimap: None,
        }
    }
}

/// Everything the flow tells its owner. The owner applies changes to its own
/// model and hands the result back with `set_nodes` and `set_edges`.
#[derive(Clone)]
pub enum FlowEvent {
    NodesChange(Vec<NodeChange>),
    EdgesChange(Vec<EdgeChange>),
    SelectionChange { nodes: Vec<Id>, edges: Vec<Id> },
    Connect(Connection),
    ConnectStart { node: Id, handle: Option<Id>, kind: HandleType },
    ConnectEnd(ConnectionState),
    /// An edge's end was dropped on another handle.
    Reconnect { edge: Id, connection: Connection },
    ReconnectEnd { edge: Id, state: ConnectionState },
    NodeClick { id: Id, modifiers: Modifiers },
    NodeDoubleClick { id: Id },
    NodeContextMenu { id: Id, window: Point<Pixels>, flow: Xy },
    NodeDragStart { id: Id, nodes: Vec<Id> },
    NodeDrag { id: Id, nodes: Vec<Id> },
    NodeDragStop { id: Id, nodes: Vec<Id> },
    EdgeClick { id: Id, modifiers: Modifiers },
    EdgeContextMenu { id: Id, window: Point<Pixels>, flow: Xy },
    PaneClick { window: Point<Pixels>, flow: Xy },
    PaneContextMenu { window: Point<Pixels>, flow: Xy },
    SelectionStart,
    SelectionEnd,
    /// Delete was pressed with these selected, edges touching the nodes included.
    Delete { nodes: Vec<Id>, edges: Vec<Id> },
    Move(Viewport),
    MoveEnd(Viewport),
    /// Every node has been measured for the first time.
    Initialized,
}

/// A painter in flow space, drawn under or over the nodes.
pub type Layer = Rc<dyn Fn(&LayerContext, &mut Window, &mut App)>;
/// An element placed at a flow position, drawn under or over the nodes.
pub type PortalRender = Rc<dyn Fn(&NodeProps, &mut Window, &mut App) -> AnyElement>;

pub struct LayerContext {
    pub viewport: Viewport,
    pub bounds: Bounds<Pixels>,
}

impl LayerContext {
    pub fn to_window(&self, p: Xy) -> Point<Pixels> {
        let s = self.viewport.to_screen(p);
        point(self.bounds.origin.x + px(s.x), self.bounds.origin.y + px(s.y))
    }
}

#[derive(Clone)]
pub struct Portal {
    pub position: Xy,
    pub front: bool,
    pub render: PortalRender,
}

#[derive(Clone)]
pub struct Layers {
    pub back: Vec<Layer>,
    pub front: Vec<Layer>,
    pub portals: Vec<Portal>,
}

struct DragItem {
    id: Id,
    start: Xy,
}

enum Drag {
    None,
    Pan { last: Xy, moved: bool, press: Xy },
    Select { start: Xy, current: Xy },
    Nodes(NodeDrag),
    Connect,
    MiniMap { last: Xy },
}

struct NodeDrag {
    clicked: Id,
    items: Vec<DragItem>,
    /// Pointer position in flow space where the drag began.
    start_flow: Xy,
    start_local: Xy,
    pointer: Xy,
    started: bool,
    toggle_off_on_click: bool,
    collapse_on_click: bool,
    modifiers: Modifiers,
}

struct Reconnecting {
    edge: Edge,
}

struct ConnectionDrag {
    from_node: Id,
    from_handle: Option<Id>,
    from_type: HandleType,
    press: Xy,
    started: bool,
    reconnecting: Option<Reconnecting>,
    window_position: Point<Pixels>,
}

struct Anim {
    from: Viewport,
    to: Viewport,
    start: Instant,
    duration: Duration,
}

pub(crate) struct EdgeGeom {
    pub id: Id,
    pub index: usize,
    pub ends: EdgeEnds,
    pub path: EdgePath,
    pub flat: Vec<Xy>,
}

pub struct Flow {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    pub options: FlowOptions,
    viewport: Viewport,
    pub(crate) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) measure: Rc<RefCell<Measure>>,
    pub(crate) lookup: Lookup,
    pub(crate) edge_geoms: Vec<EdgeGeom>,
    drag: Drag,
    connection_drag: Option<ConnectionDrag>,
    pub(crate) connection: Option<ConnectionState>,
    hovered_edge: Option<Id>,
    pointer: Option<Xy>,
    anim: Option<Anim>,
    fit_request: Option<FitViewOptions>,
    initialized: bool,
    /// Every visible node has been measured.
    ready: bool,
    pub(crate) layers: Layers,
    pub(crate) panels: Vec<Panel>,
    focus: FocusHandle,
    interactive: bool,
}

impl EventEmitter<FlowEvent> for Flow {}

impl Focusable for Flow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn is_multi(m: &Modifiers) -> bool {
    m.platform || m.control || m.shift
}

impl Flow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            options: FlowOptions::default(),
            viewport: Viewport::default(),
            bounds: Rc::default(),
            measure: Rc::default(),
            lookup: Lookup::new(),
            edge_geoms: Vec::new(),
            drag: Drag::None,
            connection_drag: None,
            connection: None,
            hovered_edge: None,
            pointer: None,
            anim: None,
            fit_request: None,
            initialized: false,
            ready: false,
            layers: Layers { back: Vec::new(), front: Vec::new(), portals: Vec::new() },
            panels: Vec::new(),
            focus: cx.focus_handle(),
            interactive: true,
        }
    }

    pub fn with_options(mut self, options: FlowOptions) -> Self {
        self.options = options;
        self.fit_request = self.options.fit_view_on_init.then_some(self.options.fit_view_options);
        self
    }

    pub fn set_options(&mut self, f: impl FnOnce(&mut FlowOptions), cx: &mut Context<Self>) {
        f(&mut self.options);
        cx.notify();
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Replaces the nodes. While a node drag is under way the dragged nodes
    /// keep the positions the pointer gave them.
    pub fn set_nodes(&mut self, mut nodes: Vec<Node>, cx: &mut Context<Self>) {
        if let Drag::Nodes(drag) = &self.drag
            && drag.started
        {
            for node in &mut nodes {
                if let Some(current) = self.nodes.iter().find(|n| n.id == node.id)
                    && drag.items.iter().any(|i| i.id == node.id)
                {
                    node.position = current.position;
                    node.dragging = true;
                }
            }
        }
        let alive: HashSet<&Id> = nodes.iter().map(|n| &n.id).collect();
        self.measure.borrow_mut().nodes.retain(|id, _| alive.contains(id));
        self.nodes = nodes;
        cx.notify();
    }

    pub fn set_edges(&mut self, edges: Vec<Edge>, cx: &mut Context<Self>) {
        self.edges = edges;
        cx.notify();
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The canvas size in pixels, once painted.
    pub fn size(&self) -> (f32, f32) {
        let b = self.bounds.get();
        (f32::from(b.size.width), f32::from(b.size.height))
    }

    pub fn set_viewport(&mut self, viewport: Viewport, duration: Option<Duration>, cx: &mut Context<Self>) {
        let (w, h) = self.size();
        let viewport = if w > 0.0 {
            constrain_viewport(viewport, w, h, &self.options.translate_extent)
        } else {
            viewport
        };
        match duration {
            Some(d) if d > Duration::ZERO => {
                self.anim = Some(Anim { from: self.viewport, to: viewport, start: Instant::now(), duration: d });
            }
            _ => {
                self.anim = None;
                self.viewport = viewport;
                cx.emit(FlowEvent::Move(viewport));
                cx.emit(FlowEvent::MoveEnd(viewport));
            }
        }
        cx.notify();
    }

    pub fn pan_by(&mut self, delta: Xy, cx: &mut Context<Self>) {
        let mut vp = self.viewport;
        vp.x += delta.x;
        vp.y += delta.y;
        let (w, h) = self.size();
        self.viewport = constrain_viewport(vp, w, h, &self.options.translate_extent);
        cx.emit(FlowEvent::Move(self.viewport));
        cx.notify();
    }

    /// Fits the nodes in view, once they have been measured.
    pub fn fit_view(&mut self, options: FitViewOptions, cx: &mut Context<Self>) {
        self.fit_request = Some(options);
        cx.notify();
    }

    pub fn fit_bounds(&mut self, bounds: Rect, options: FitViewOptions, cx: &mut Context<Self>) {
        let (w, h) = self.size();
        let vp = viewport_for_bounds(
            bounds,
            w,
            h,
            options.min_zoom.unwrap_or(self.options.min_zoom),
            options.max_zoom.unwrap_or(self.options.max_zoom),
            options.padding,
        );
        self.set_viewport(vp, options.duration, cx);
    }

    pub fn zoom_by(&mut self, factor: f32, duration: Option<Duration>, cx: &mut Context<Self>) {
        let (w, h) = self.size();
        let target = self.anim.as_ref().map_or(self.viewport, |a| a.to);
        let vp = target.zoomed_at(factor, Xy::new(w / 2.0, h / 2.0), self.options.min_zoom, self.options.max_zoom);
        self.set_viewport(vp, duration, cx);
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom_by(1.2, Some(Duration::from_millis(200)), cx);
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom_by(1.0 / 1.2, Some(Duration::from_millis(200)), cx);
    }

    /// Centres the view on a flow point.
    pub fn set_center(&mut self, center: Xy, zoom: Option<f32>, duration: Option<Duration>, cx: &mut Context<Self>) {
        let (w, h) = self.size();
        let zoom = zoom.unwrap_or(self.viewport.zoom);
        let vp = Viewport { x: w / 2.0 - center.x * zoom, y: h / 2.0 - center.y * zoom, zoom };
        self.set_viewport(vp, duration, cx);
    }

    /// Window pixels to flow units.
    pub fn screen_to_flow(&self, p: Point<Pixels>) -> Xy {
        self.viewport.to_flow(self.local(p))
    }

    pub fn screen_to_flow_snapped(&self, p: Point<Pixels>) -> Xy {
        self.viewport.to_flow_snapped(self.local(p), self.options.snap_grid)
    }

    pub fn flow_to_screen(&self, p: Xy) -> Point<Pixels> {
        let s = self.viewport.to_screen(p);
        let o = self.bounds.get().origin;
        point(o.x + px(s.x), o.y + px(s.y))
    }

    /// The last pointer position over the canvas, in flow units.
    pub fn pointer_flow(&self) -> Option<Xy> {
        self.pointer.map(|p| self.viewport.to_flow(p))
    }

    /// A node's rectangle in flow units, once measured.
    pub fn node_rect(&self, id: &Id) -> Option<Rect> {
        self.lookup.get(id).filter(|n| n.size.is_some()).map(|n| n.rect())
    }

    pub fn selected_nodes(&self) -> Vec<Id> {
        self.nodes.iter().filter(|n| n.selected).map(|n| n.id.clone()).collect()
    }

    pub fn selected_edges(&self) -> Vec<Id> {
        self.edges.iter().filter(|e| e.selected).map(|e| e.id.clone()).collect()
    }

    pub fn set_layers(&mut self, layers: Layers, cx: &mut Context<Self>) {
        self.layers = layers;
        cx.notify();
    }

    pub fn set_panels(&mut self, panels: Vec<Panel>, cx: &mut Context<Self>) {
        self.panels = panels;
        cx.notify();
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    pub fn set_interactive(&mut self, interactive: bool, cx: &mut Context<Self>) {
        self.interactive = interactive;
        self.options.nodes_draggable = interactive;
        self.options.nodes_connectable = interactive;
        self.options.elements_selectable = interactive;
        cx.notify();
    }

    /// Replaces the selection, reporting what changed.
    pub fn set_selection(&mut self, nodes: &[Id], edges: &[Id], cx: &mut Context<Self>) {
        let nodes: HashSet<&Id> = nodes.iter().collect();
        let edges: HashSet<&Id> = edges.iter().collect();
        self.apply_selection(|n| nodes.contains(n), |e| edges.contains(e), cx);
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.apply_selection(|_| false, |_| false, cx);
    }

    fn apply_selection(
        &mut self,
        node: impl Fn(&Id) -> bool,
        edge: impl Fn(&Id) -> bool,
        cx: &mut Context<Self>,
    ) {
        let mut node_changes = Vec::new();
        for n in &mut self.nodes {
            let selected = node(&n.id);
            if n.selected != selected {
                n.selected = selected;
                node_changes.push(NodeChange::Select { id: n.id.clone(), selected });
            }
        }
        let mut edge_changes = Vec::new();
        for e in &mut self.edges {
            let selected = edge(&e.id);
            if e.selected != selected {
                e.selected = selected;
                edge_changes.push(EdgeChange::Select { id: e.id.clone(), selected });
            }
        }
        let changed = !node_changes.is_empty() || !edge_changes.is_empty();
        if !node_changes.is_empty() {
            cx.emit(FlowEvent::NodesChange(node_changes));
        }
        if !edge_changes.is_empty() {
            cx.emit(FlowEvent::EdgesChange(edge_changes));
        }
        if changed {
            cx.emit(FlowEvent::SelectionChange {
                nodes: self.selected_nodes(),
                edges: self.selected_edges(),
            });
            cx.notify();
        }
    }

    fn local(&self, p: Point<Pixels>) -> Xy {
        let o = self.bounds.get().origin;
        Xy::new(f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    fn node_selectable(&self, n: &Node) -> bool {
        n.selectable.unwrap_or(self.options.elements_selectable)
    }

    fn node_draggable(&self, n: &Node) -> bool {
        n.draggable.unwrap_or(self.options.nodes_draggable)
    }

    fn node_connectable(&self, n: &Node) -> bool {
        n.connectable.unwrap_or(self.options.nodes_connectable)
    }

    fn edge_selectable(&self, e: &Edge) -> bool {
        e.selectable.unwrap_or(self.options.elements_selectable)
    }

    /// Resolves node geometry from the measurements of the last paint.
    fn rebuild_lookup(&mut self, cx: &mut Context<Self>) {
        let measure = self.measure.borrow();
        let mut lookup = Lookup::with_capacity(self.nodes.len());
        for (z, node) in self.nodes.iter().enumerate() {
            let measured = measure.nodes.get(&node.id);
            let size = match (node.width, node.height, measured) {
                (Some(w), Some(h), _) => Some((w, h)),
                (w, h, Some(m)) if m.zoom > 0.0 => {
                    let mw = f32::from(m.bounds.size.width) / m.zoom;
                    let mh = f32::from(m.bounds.size.height) / m.zoom;
                    Some((w.unwrap_or(mw), h.unwrap_or(mh)))
                }
                _ => None,
            };
            let handles = measured
                .filter(|m| m.zoom > 0.0)
                .map(|m| {
                    m.handles
                        .iter()
                        .map(|((id, kind), h)| {
                            let z = m.zoom;
                            HandleBounds {
                                id: id.clone(),
                                kind: *kind,
                                position: h.position,
                                x: f32::from(h.bounds.origin.x - m.bounds.origin.x) / z,
                                y: f32::from(h.bounds.origin.y - m.bounds.origin.y) / z,
                                width: f32::from(h.bounds.size.width) / z,
                                height: f32::from(h.bounds.size.height) / z,
                                connectable: h.connectable,
                                connectable_start: h.connectable_start,
                                connectable_end: h.connectable_end,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let (w, h) = size.unwrap_or((0.0, 0.0));
            let position = Xy::new(
                node.position.x - node.origin.0 * w,
                node.position.y - node.origin.1 * h,
            );
            let elevated = if node.selected && self.options.elevate_nodes_on_select { 1000 } else { 0 };
            lookup.insert(
                node.id.clone(),
                InternalNode {
                    id: node.id.clone(),
                    position,
                    size,
                    handles,
                    connectable: self.node_connectable(node),
                    z: node.z_index + elevated + (z as i32).min(0),
                },
            );
        }
        drop(measure);
        let ready = !self.nodes.is_empty()
            && self.nodes.iter().filter(|n| !n.hidden).all(|n| {
                lookup.get(&n.id).is_some_and(|i| i.size.is_some())
            });
        self.lookup = lookup;
        self.ready = ready;
        if ready && !self.initialized {
            self.initialized = true;
            cx.emit(FlowEvent::Initialized);
        }
        if self.nodes.is_empty() {
            self.initialized = false;
        }
    }

    fn rebuild_edges(&mut self) {
        let mut geoms = Vec::new();
        for (index, edge) in self.edges.iter().enumerate() {
            if edge.hidden {
                continue;
            }
            let (Some(source), Some(target)) = (self.lookup.get(&edge.source), self.lookup.get(&edge.target)) else {
                continue;
            };
            let Some(ends) = edge_ends(
                source,
                edge.source_handle.as_ref(),
                target,
                edge.target_handle.as_ref(),
                self.options.connection_mode,
            ) else {
                continue;
            };
            let path = edge_geometry(edge.kind, &ends);
            let flat = flatten(&path.segments, 24);
            geoms.push(EdgeGeom { id: edge.id.clone(), index, ends, path, flat });
        }
        let elevate = self.options.elevate_edges_on_select;
        geoms.sort_by_key(|g| {
            let e = &self.edges[g.index];
            e.z_index + if elevate && e.selected { 1000 } else { 0 }
        });
        self.edge_geoms = geoms;
    }

    fn connections_by_node(&self) -> HashMap<Id, Rc<NodeConnections>> {
        let mut map: HashMap<Id, NodeConnections> = HashMap::new();
        for e in &self.edges {
            map.entry(e.source.clone()).or_default().outgoing.push((e.source_handle.clone(), e.id.clone()));
            map.entry(e.target.clone()).or_default().incoming.push((e.target_handle.clone(), e.id.clone()));
        }
        map.into_iter().map(|(k, v)| (k, Rc::new(v))).collect()
    }

    fn edge_at(&self, flow: Xy) -> Option<usize> {
        self.edge_geoms.iter().rev().find_map(|g| {
            let e = &self.edges[g.index];
            let reach = e.interaction_width / 2.0;
            (crate::edge_path::distance_to_polyline(flow, &g.flat) <= reach).then_some(g.index)
        })
    }

    /// The edge end within reach of the pointer, for edges that can be reconnected.
    fn edge_end_at(&self, flow: Xy) -> Option<(usize, HandleType)> {
        if !self.options.edges_reconnectable {
            return None;
        }
        let r = self.options.reconnect_radius;
        self.edge_geoms.iter().rev().find_map(|g| {
            let e = &self.edges[g.index];
            if !e.reconnectable.unwrap_or(true) || !e.selected {
                return None;
            }
            if g.ends.source.distance(flow) <= r {
                Some((g.index, HandleType::Source))
            } else if g.ends.target.distance(flow) <= r {
                Some((g.index, HandleType::Target))
            } else {
                None
            }
        })
    }

    pub(crate) fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut animating = false;
        if let Some(anim) = &self.anim {
            let t = (anim.start.elapsed().as_secs_f32() / anim.duration.as_secs_f32()).min(1.0);
            let eased = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
            self.viewport = anim.from.lerp(&anim.to, eased);
            if t >= 1.0 {
                self.viewport = anim.to;
                self.anim = None;
                cx.emit(FlowEvent::Move(self.viewport));
                cx.emit(FlowEvent::MoveEnd(self.viewport));
            } else {
                cx.emit(FlowEvent::Move(self.viewport));
                animating = true;
            }
        }

        if let Some(options) = self.fit_request
            && self.ready
        {
            self.fit_request = None;
            let rects = self
                .nodes
                .iter()
                .filter(|n| options.include_hidden || !n.hidden)
                .filter_map(|n| self.lookup.get(&n.id).map(|i| i.rect()));
            if let Some(bounds) = bounds_of(rects) {
                let (w, h) = self.size();
                if w > 0.0 && h > 0.0 {
                    let vp = viewport_for_bounds(
                        bounds,
                        w,
                        h,
                        options.min_zoom.unwrap_or(self.options.min_zoom),
                        options.max_zoom.unwrap_or(self.options.max_zoom),
                        options.padding,
                    );
                    self.set_viewport(vp, options.duration, cx);
                } else {
                    self.fit_request = Some(options);
                }
            }
        }

        let (w, h) = self.size();
        let pointer = self.pointer.unwrap_or(Xy::ZERO);
        let pan = auto_pan(pointer, w, h, self.options.auto_pan_speed, 40.0);
        let panning = pan != Xy::ZERO;
        if panning {
            if self.connection_drag.as_ref().is_some_and(|d| d.started) && self.options.auto_pan_on_connect {
                self.pan_by(pan, cx);
                self.update_connection(pointer, window, cx);
                animating = true;
            } else if let Drag::Nodes(d) = &self.drag
                && d.started
                && self.options.auto_pan_on_node_drag
            {
                self.pan_by(pan, cx);
                self.move_dragged_nodes(pointer, cx);
                animating = true;
            }
        }
        if animating {
            window.request_animation_frame();
        }
    }

    // --- pointer handling ---

    pub(crate) fn start_connection(
        &mut self,
        node: Id,
        handle: Option<Id>,
        kind: HandleType,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_flow(window, cx);
        let press = self.local(event.position);
        self.connection_drag = Some(ConnectionDrag {
            from_node: node,
            from_handle: handle,
            from_type: kind,
            press,
            started: false,
            reconnecting: None,
            window_position: event.position,
        });
        self.drag = Drag::Connect;
        cx.notify();
    }

    fn focus_flow(&self, window: &mut Window, cx: &App) {
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus);
        }
    }

    fn update_connection(&mut self, local: Xy, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.connection_drag else {
            return;
        };
        if !drag.started {
            if local.distance(drag.press) <= 1.0 {
                return;
            }
            drag.started = true;
            let (node, handle, kind) = (drag.from_node.clone(), drag.from_handle.clone(), drag.from_type);
            if drag.reconnecting.is_none() {
                cx.emit(FlowEvent::ConnectStart { node, handle, kind });
            }
        }
        let drag = self.connection_drag.as_ref().unwrap();
        let (from_node, from_handle, from_type) = (drag.from_node.clone(), drag.from_handle.clone(), drag.from_type);
        let window_position = drag.window_position;
        let reconnecting = drag.reconnecting.as_ref().map(|r| r.edge.id.clone());
        let flow = self.viewport.to_flow(local);
        let from_key = (&from_node, from_type, from_handle.as_ref());
        let Some(from_internal) = self.lookup.get(&from_node) else {
            return;
        };
        let Some(from_bounds) = find_handle(from_internal, from_type, from_handle.as_ref(), self.options.connection_mode)
        else {
            return;
        };
        let from_pos = crate::lookup::handle_position(from_internal, Some(from_bounds), true);
        let from_position = from_bounds.position;

        let target = handle_under(flow, &self.lookup, from_key)
            .or_else(|| closest_handle(flow, self.options.connection_radius, &self.lookup, from_key));
        let validity = target.as_ref().map(|near| {
            let is_valid = self.options.is_valid_connection.clone();
            validate(
                from_key,
                near,
                self.options.connection_mode,
                is_valid.as_deref().map(|f| f as &dyn Fn(&Connection) -> bool),
            )
        });
        let valid = validity.as_ref().map(|v| v.valid);
        let (to, to_position, to_node, to_handle) = match (&target, valid) {
            (Some(near), Some(true)) => (
                near.center,
                near.handle.position,
                Some(near.node.clone()),
                near.handle.id.clone(),
            ),
            (Some(near), _) => (flow, from_position.opposite(), Some(near.node.clone()), near.handle.id.clone()),
            _ => (flow, from_position.opposite(), None, None),
        };
        self.connection = Some(ConnectionState {
            is_valid: valid,
            from: from_pos,
            from_node,
            from_handle,
            from_type,
            from_position,
            to,
            to_node,
            to_handle,
            to_position,
            pointer: local,
            window_position,
            reconnecting,
        });
        let _ = window;
        cx.notify();
    }

    fn finish_connection(&mut self, local: Xy, cx: &mut Context<Self>) {
        let Some(drag) = self.connection_drag.take() else {
            return;
        };
        let state = self.connection.take();
        if !drag.started {
            return;
        }
        let Some(state) = state else { return };
        let flow = self.viewport.to_flow(local);
        let from_key = (&drag.from_node, drag.from_type, drag.from_handle.as_ref());
        let target = handle_under(flow, &self.lookup, from_key)
            .or_else(|| closest_handle(flow, self.options.connection_radius, &self.lookup, from_key));
        let verdict = target.as_ref().map(|near| {
            let is_valid = self.options.is_valid_connection.clone();
            validate(
                from_key,
                near,
                self.options.connection_mode,
                is_valid.as_deref().map(|f| f as &dyn Fn(&Connection) -> bool),
            )
        });
        if let Some(v) = verdict
            && v.valid
        {
            match &drag.reconnecting {
                Some(r) => cx.emit(FlowEvent::Reconnect { edge: r.edge.id.clone(), connection: v.connection }),
                None => cx.emit(FlowEvent::Connect(v.connection)),
            }
        }
        match &drag.reconnecting {
            Some(r) => cx.emit(FlowEvent::ReconnectEnd { edge: r.edge.id.clone(), state }),
            None => cx.emit(FlowEvent::ConnectEnd(state)),
        }
    }

    fn node_down(&mut self, id: Id, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.focus_flow(window, cx);
        let local = self.local(event.position);
        let flow = self.viewport.to_flow(local);
        let Some(node) = self.nodes.iter().find(|n| n.id == id).cloned() else {
            return;
        };
        match event.button {
            MouseButton::Middle => {
                self.drag = Drag::Pan { last: local, moved: false, press: local };
            }
            MouseButton::Right => {
                if self.node_selectable(&node) && !node.selected {
                    self.apply_selection(|n| *n == id, |_| false, cx);
                }
                cx.emit(FlowEvent::NodeContextMenu { id, window: event.position, flow });
            }
            MouseButton::Left => {
                if event.click_count == 2 {
                    cx.emit(FlowEvent::NodeDoubleClick { id: id.clone() });
                }
                let multi = is_multi(&event.modifiers);
                let selectable = self.node_selectable(&node);
                let mut toggle_off = false;
                let mut collapse = false;
                if selectable {
                    if multi {
                        if node.selected {
                            toggle_off = true;
                        } else {
                            let cur: HashSet<Id> = self.selected_nodes().into_iter().collect();
                            let edges: HashSet<Id> = self.selected_edges().into_iter().collect();
                            self.apply_selection(|n| cur.contains(n) || *n == id, |e| edges.contains(e), cx);
                        }
                    } else if !node.selected {
                        if self.options.select_nodes_on_drag || !self.node_draggable(&node) {
                            self.apply_selection(|n| *n == id, |_| false, cx);
                        }
                    } else {
                        collapse = true;
                    }
                }
                let items: Vec<DragItem> = if self.node_draggable(&node) {
                    self.nodes
                        .iter()
                        .filter(|n| (n.selected || n.id == id) && self.node_draggable(n))
                        .map(|n| DragItem { id: n.id.clone(), start: n.position })
                        .collect()
                } else {
                    Vec::new()
                };
                self.drag = Drag::Nodes(NodeDrag {
                    clicked: id,
                    items,
                    start_flow: flow,
                    start_local: local,
                    pointer: local,
                    started: false,
                    toggle_off_on_click: toggle_off,
                    collapse_on_click: collapse,
                    modifiers: event.modifiers,
                });
            }
            MouseButton::Navigate(_) => {}
        }
        cx.notify();
    }

    fn pane_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_flow(window, cx);
        let local = self.local(event.position);
        let flow = self.viewport.to_flow(local);
        match event.button {
            MouseButton::Right => {
                if let Some(index) = self.edge_at(flow) {
                    let id = self.edges[index].id.clone();
                    if self.edge_selectable(&self.edges[index]) && !self.edges[index].selected {
                        self.apply_selection(|_| false, |e| *e == id, cx);
                    }
                    cx.emit(FlowEvent::EdgeContextMenu { id, window: event.position, flow });
                } else {
                    cx.emit(FlowEvent::PaneContextMenu { window: event.position, flow });
                }
            }
            MouseButton::Middle => {
                self.drag = Drag::Pan { last: local, moved: false, press: local };
            }
            MouseButton::Left => {
                if let Some((index, end)) = self.edge_end_at(flow) {
                    let edge = self.edges[index].clone();
                    // The end that stays put is where the new connection starts from.
                    let (node, handle, kind) = match end {
                        HandleType::Source => (edge.target.clone(), edge.target_handle.clone(), HandleType::Target),
                        HandleType::Target => (edge.source.clone(), edge.source_handle.clone(), HandleType::Source),
                    };
                    self.connection_drag = Some(ConnectionDrag {
                        from_node: node,
                        from_handle: handle,
                        from_type: kind,
                        press: local,
                        started: false,
                        reconnecting: Some(Reconnecting { edge }),
                        window_position: event.position,
                    });
                    self.drag = Drag::Connect;
                } else if let Some(index) = self.edge_at(flow) {
                    let edge = &self.edges[index];
                    let id = edge.id.clone();
                    if self.edge_selectable(edge) {
                        if is_multi(&event.modifiers) {
                            let nodes: HashSet<Id> = self.selected_nodes().into_iter().collect();
                            let mut edges: HashSet<Id> = self.selected_edges().into_iter().collect();
                            if !edges.remove(&id) {
                                edges.insert(id.clone());
                            }
                            self.apply_selection(|n| nodes.contains(n), |e| edges.contains(e), cx);
                        } else {
                            self.apply_selection(|_| false, |e| *e == id, cx);
                        }
                    }
                    cx.emit(FlowEvent::EdgeClick { id, modifiers: event.modifiers });
                } else if event.modifiers.shift || self.options.selection_on_drag {
                    self.drag = Drag::Select { start: local, current: local };
                    cx.emit(FlowEvent::SelectionStart);
                } else {
                    self.drag = Drag::Pan { last: local, moved: false, press: local };
                }
                if event.click_count == 2
                    && self.options.zoom_on_double_click
                    && matches!(self.drag, Drag::Pan { .. })
                {
                    self.drag = Drag::None;
                    let target = self.anim.as_ref().map_or(self.viewport, |a| a.to);
                    let vp = target.zoomed_at(2.0, local, self.options.min_zoom, self.options.max_zoom);
                    self.set_viewport(vp, Some(Duration::from_millis(300)), cx);
                }
            }
            MouseButton::Navigate(_) => {}
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let local = self.local(event.position);
        let inside = self.bounds.get().contains(&event.position);
        if inside {
            self.pointer = Some(local);
        }
        let pan_on_drag = self.options.pan_on_drag;
        match &mut self.drag {
            Drag::None => {
                if inside {
                    let hovered = self.edge_at(self.viewport.to_flow(local)).map(|i| self.edges[i].id.clone());
                    if hovered != self.hovered_edge {
                        self.hovered_edge = hovered;
                        cx.notify();
                    }
                }
                return;
            }
            Drag::Pan { last, moved, press } => {
                if !pan_on_drag || (!*moved && local.distance(*press) <= 2.0) {
                    return;
                }
                *moved = true;
                let delta = local - *last;
                *last = local;
                self.pan_by(delta, cx);
            }
            Drag::Select { current, .. } => {
                *current = local;
                self.update_box_selection(cx);
            }
            Drag::Nodes(d) => {
                d.pointer = local;
                d.modifiers = event.modifiers;
                self.pointer = Some(local);
                self.move_dragged_nodes(local, cx);
            }
            Drag::Connect => {
                self.pointer = Some(local);
                self.update_connection(local, window, cx);
            }
            Drag::MiniMap { last } => {
                let delta = local - *last;
                *last = local;
                self.drag_minimap(delta, cx);
            }
        }
        window.request_animation_frame();
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let local = self.local(event.position);
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Pan { moved, .. } => {
                if moved {
                    cx.emit(FlowEvent::MoveEnd(self.viewport));
                } else if event.button == MouseButton::Left {
                    let flow = self.viewport.to_flow(local);
                    cx.emit(FlowEvent::PaneClick { window: event.position, flow });
                    self.apply_selection(|_| false, |_| false, cx);
                }
            }
            Drag::Select { start, current } => {
                if start.distance(current) <= 2.0 {
                    self.apply_selection(|_| false, |_| false, cx);
                    let flow = self.viewport.to_flow(local);
                    cx.emit(FlowEvent::PaneClick { window: event.position, flow });
                }
                cx.emit(FlowEvent::SelectionEnd);
            }
            Drag::Nodes(d) => self.finish_node_drag(d, cx),
            Drag::Connect => self.finish_connection(local, cx),
            Drag::MiniMap { .. } => cx.emit(FlowEvent::MoveEnd(self.viewport)),
        }
        cx.notify();
    }

    fn move_dragged_nodes(&mut self, local: Xy, cx: &mut Context<Self>) {
        let Drag::Nodes(d) = &mut self.drag else {
            return;
        };
        if !d.started {
            if local.distance(d.start_local) <= self.options.node_drag_threshold || d.items.is_empty() {
                return;
            }
            d.started = true;
            let ids: Vec<Id> = d.items.iter().map(|i| i.id.clone()).collect();
            let clicked = d.clicked.clone();
            cx.emit(FlowEvent::NodeDragStart { id: clicked, nodes: ids });
        }
        let Drag::Nodes(d) = &self.drag else { return };
        let flow = self.viewport.to_flow(local);
        let delta = flow - d.start_flow;
        // Snapping moves the whole selection by the first node's offset.
        let snap_offset = match (self.options.snap_grid, d.items.first()) {
            (Some(grid), Some(first)) => {
                let raw = first.start + delta;
                let snapped = snap_position(raw, grid);
                snapped - raw
            }
            _ => Xy::ZERO,
        };
        let mut changes = Vec::new();
        let ids: Vec<Id> = d.items.iter().map(|i| i.id.clone()).collect();
        let clicked = d.clicked.clone();
        let positions: Vec<(Id, Xy)> = d
            .items
            .iter()
            .map(|item| {
                let mut p = item.start + delta + snap_offset;
                let node = self.nodes.iter().find(|n| n.id == item.id);
                let extent = node.and_then(|n| n.extent).unwrap_or(self.options.node_extent);
                let (w, h) = self.lookup.get(&item.id).and_then(|i| i.size).unwrap_or((0.0, 0.0));
                let origin = node.map_or((0.0, 0.0), |n| n.origin);
                // The extent bounds the top-left corner, whatever the origin.
                let top_left = Xy::new(p.x - origin.0 * w, p.y - origin.1 * h);
                let clamped = clamp_position(top_left, &extent, w, h);
                p = Xy::new(clamped.x + origin.0 * w, clamped.y + origin.1 * h);
                (item.id.clone(), p)
            })
            .collect();
        for (id, p) in positions {
            if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
                n.position = p;
                n.dragging = true;
            }
            changes.push(NodeChange::Position { id, position: Some(p), dragging: Some(true) });
        }
        cx.emit(FlowEvent::NodesChange(changes));
        cx.emit(FlowEvent::NodeDrag { id: clicked, nodes: ids });
        cx.notify();
    }

    fn finish_node_drag(&mut self, d: NodeDrag, cx: &mut Context<Self>) {
        if d.started {
            let mut changes = Vec::new();
            for item in &d.items {
                if let Some(n) = self.nodes.iter_mut().find(|n| n.id == item.id) {
                    n.dragging = false;
                    changes.push(NodeChange::Position { id: n.id.clone(), position: Some(n.position), dragging: Some(false) });
                }
            }
            cx.emit(FlowEvent::NodesChange(changes));
            cx.emit(FlowEvent::NodeDragStop {
                id: d.clicked.clone(),
                nodes: d.items.iter().map(|i| i.id.clone()).collect(),
            });
            return;
        }
        let id = d.clicked.clone();
        if d.toggle_off_on_click {
            let nodes: HashSet<Id> = self.selected_nodes().into_iter().filter(|n| *n != id).collect();
            let edges: HashSet<Id> = self.selected_edges().into_iter().collect();
            self.apply_selection(|n| nodes.contains(n), |e| edges.contains(e), cx);
        } else if d.collapse_on_click {
            self.apply_selection(|n| *n == id, |_| false, cx);
        }
        cx.emit(FlowEvent::NodeClick { id, modifiers: d.modifiers });
    }

    fn update_box_selection(&mut self, cx: &mut Context<Self>) {
        let Drag::Select { start, current } = &self.drag else {
            return;
        };
        let a = self.viewport.to_flow(*start);
        let b = self.viewport.to_flow(*current);
        let rect = Rect::from_corners(a, b);
        let partial = self.options.selection_mode == SelectionMode::Partial;
        let mut picked: HashSet<Id> = HashSet::new();
        for n in &self.nodes {
            if n.hidden || !self.node_selectable(n) {
                continue;
            }
            let Some(r) = self.lookup.get(&n.id).filter(|i| i.size.is_some()).map(|i| i.rect()) else {
                continue;
            };
            let hit = if partial {
                r.overlap_area(&rect) > 0.0
            } else {
                r.x >= rect.x && r.y >= rect.y && r.right() <= rect.right() && r.bottom() <= rect.bottom()
            };
            if hit {
                picked.insert(n.id.clone());
            }
        }
        let edges: HashSet<Id> = self
            .edges
            .iter()
            .filter(|e| self.edge_selectable(e) && picked.contains(&e.source) && picked.contains(&e.target))
            .map(|e| e.id.clone())
            .collect();
        self.apply_selection(|n| picked.contains(n), |e| edges.contains(e), cx);
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let local = self.local(event.position);
        let ctrl = event.modifiers.control || event.modifiers.platform;
        // Browsers report a notch as about three lines.
        let (dx, dy, lines) = match event.delta {
            ScrollDelta::Lines(l) => (l.x * 3.0, l.y * 3.0, true),
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y), false),
        };
        let target = self.anim.as_ref().map_or(self.viewport, |a| a.to);
        self.anim = None;
        let (w, h) = self.size();
        if (self.options.zoom_on_scroll && !self.options.pan_on_scroll)
            || (ctrl && self.options.zoom_on_pinch)
        {
            // GPUI's deltas point the way the content moves, opposite to a browser's.
            let exp = wheel_exponent(-dy, lines, ctrl);
            let vp = target.zoomed_at(2f32.powf(exp), local, self.options.min_zoom, self.options.max_zoom);
            self.viewport = constrain_viewport(vp, w, h, &self.options.translate_extent);
        } else if self.options.pan_on_scroll {
            let speed = self.options.pan_on_scroll_speed;
            let (dx, dy) = if event.modifiers.shift { (dy, dx) } else { (dx, dy) };
            let vp = Viewport { x: target.x + dx * speed, y: target.y + dy * speed, zoom: target.zoom };
            self.viewport = constrain_viewport(vp, w, h, &self.options.translate_extent);
        } else {
            return;
        }
        cx.emit(FlowEvent::Move(self.viewport));
        cx.emit(FlowEvent::MoveEnd(self.viewport));
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "backspace" | "delete" if self.options.delete_key => {
                let nodes: Vec<Id> = self
                    .nodes
                    .iter()
                    .filter(|n| n.selected && n.deletable.unwrap_or(true))
                    .map(|n| n.id.clone())
                    .collect();
                let mut edges: HashSet<Id> = self
                    .edges
                    .iter()
                    .filter(|e| e.selected && e.deletable.unwrap_or(true))
                    .map(|e| e.id.clone())
                    .collect();
                edges.extend(
                    crate::model::connected_edges(&nodes, &self.edges)
                        .into_iter()
                        .filter(|e| e.deletable.unwrap_or(true))
                        .map(|e| e.id.clone()),
                );
                if !nodes.is_empty() || !edges.is_empty() {
                    cx.emit(FlowEvent::Delete { nodes, edges: edges.into_iter().collect() });
                }
            }
            "escape" if self.connection_drag.take().is_some() => {
                self.connection = None;
                self.drag = Drag::None;
                cx.notify();
            }
            _ => {}
        }
    }

    fn drag_minimap(&mut self, delta: Xy, cx: &mut Context<Self>) {
        let Some(view) = self.minimap_view() else {
            return;
        };
        let k = view.scale * self.viewport.zoom;
        self.pan_by(Xy::new(-delta.x * k, -delta.y * k), cx);
    }

    pub(crate) fn local_xy(&self, p: Point<Pixels>) -> Xy {
        self.local(p)
    }

    pub(crate) fn begin_minimap_drag(&mut self, local: Xy) {
        self.drag = Drag::MiniMap { last: local };
    }
}

fn edge_geometry(kind: EdgeKind, ends: &EdgeEnds) -> EdgePath {
    let params = PathParams::new(ends.source, ends.source_position, ends.target, ends.target_position);
    match kind {
        EdgeKind::Bezier => bezier_path(params, DEFAULT_CURVATURE),
        EdgeKind::SmoothStep => smooth_step_path(params, StepOptions::default()),
        EdgeKind::Step => smooth_step_path(params, StepOptions { border_radius: 0.0, ..StepOptions::default() }),
        EdgeKind::Straight => straight_path(ends.source, ends.target),
    }
}

struct PaintEdge {
    segments: Vec<crate::edge_path::Segment>,
    style: EdgeStyle,
    selected: bool,
    hovered: bool,
    arrow: Option<(Xy, f32, gpui::Hsla)>,
}

fn paint_edge(window: &mut Window, origin: Point<Pixels>, vp: &Viewport, e: &PaintEdge, hover: gpui::Hsla) {
    use crate::edge_path::Segment;
    use crate::model::EdgeColor;
    let z = vp.zoom;
    let to_px = |p: Xy| {
        let s = vp.to_screen(p);
        point(origin.x + px(s.x), origin.y + px(s.y))
    };
    let build = |width: f32, dash: Option<(f32, f32)>| {
        let mut b = gpui::PathBuilder::stroke(px((width * z).max(1.0)));
        if let Some((on, off)) = dash {
            b = b.dash_array(&[px((on * z).max(0.5)), px((off * z).max(0.5))]);
        }
        for seg in &e.segments {
            match *seg {
                Segment::Move(p) => b.move_to(to_px(p)),
                Segment::Line(p) => b.line_to(to_px(p)),
                Segment::Quad(c, p) => b.curve_to(to_px(p), to_px(c)),
                Segment::Cubic(c1, c2, p) => b.cubic_bezier_to(to_px(p), to_px(c1), to_px(c2)),
            }
        }
        b.build().ok()
    };
    let opacity = if e.selected { e.style.selected_opacity } else { e.style.opacity };
    let background: gpui::Background = match e.style.color {
        EdgeColor::Solid(c) => c.opacity(opacity).into(),
        EdgeColor::Gradient(from, to) => {
            let (Some(Segment::Move(a)), Some(last)) = (e.segments.first(), e.segments.last()) else {
                return;
            };
            let b = match *last {
                Segment::Move(p) | Segment::Line(p) | Segment::Quad(_, p) | Segment::Cubic(_, _, p) => p,
            };
            // CSS angles run clockwise from "up".
            let angle = (b.x - a.x).atan2(-(b.y - a.y)).to_degrees();
            gpui::linear_gradient(
                angle,
                gpui::linear_color_stop(from.opacity(opacity), 0.0),
                gpui::linear_color_stop(to.opacity(opacity), 1.0),
            )
        }
    };
    if let Some(path) = build(e.style.width, e.style.dash) {
        window.paint_path(path, background);
    }
    if e.hovered
        && let Some(path) = build(1.0, None)
    {
        window.paint_path(path, hover);
    }
    if let Some((at, angle, color)) = e.arrow {
        let c = to_px(at);
        let (sin, cos) = angle.sin_cos();
        let pt = |x: f32, y: f32| {
            point(c.x + px((x * cos - y * sin) * z), c.y + px((x * sin + y * cos) * z))
        };
        let mut b = gpui::PathBuilder::fill();
        b.add_polygon(&[pt(-4.0, -4.5), pt(5.0, 0.0), pt(-4.0, 4.5)], true);
        if let Ok(path) = b.build() {
            window.paint_path(path, color.opacity(opacity));
        }
    }
}

fn default_node(node: &Node, props: &NodeProps) -> AnyElement {
    let theme = props.theme;
    let s = |v: f32| props.scaled(v);
    let mut body = div()
        .relative()
        .min_w(s(150.0))
        .px(s(10.0))
        .py(s(10.0))
        .rounded(s(3.0))
        .border(px(1.0))
        .border_color(if props.selected { theme.node_border_selected } else { theme.node_border })
        .bg(theme.node_background)
        .text_color(theme.node_text)
        .text_size(s(12.0))
        .flex()
        .justify_center()
        .child(node.label.clone().unwrap_or_else(|| node.id.clone()));
    if node.kind != NodeKind::Input {
        body = body.child(props.handle(None, HandleType::Target, Position::Top));
    }
    if node.kind != NodeKind::Output {
        body = body.child(props.handle(None, HandleType::Source, Position::Bottom));
    }
    body.into_any_element()
}

impl Render for Flow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rebuild_lookup(cx);
        self.rebuild_edges();
        self.tick(window, cx);

        let vp = self.viewport;
        let theme = self.options.theme;
        let entity: WeakEntity<Flow> = cx.entity().downgrade();
        let connections = self.connections_by_node();
        let connection = self.connection.clone().map(Rc::new);

        // Back layer: background, flow-space painters, then edges.
        let background = self.options.background;
        let back_layers = self.layers.back.clone();
        let front_layers = self.layers.front.clone();
        let reconnecting = self.connection.as_ref().and_then(|c| c.reconnecting.clone());
        let default_style = self.options.default_edge_style;
        let paint_edges: Vec<PaintEdge> = self
            .edge_geoms
            .iter()
            .filter(|g| reconnecting.as_ref() != Some(&g.id))
            .map(|g| {
                let e = &self.edges[g.index];
                let style = e
                    .style
                    .or(default_style)
                    .unwrap_or_else(|| EdgeStyle::solid(theme.edge).opacity(1.0, 1.0));
                let style = if e.selected && e.style.is_none() && default_style.is_none() {
                    EdgeStyle::solid(theme.edge_selected)
                } else {
                    style
                };
                let arrow = e.arrow.map(|color| {
                    let angle = match g.path.segments.get(1) {
                        Some(crate::edge_path::Segment::Cubic(c1, c2, t)) => {
                            crate::edge_path::midpoint_angle(g.ends.source, *c1, *c2, *t)
                        }
                        _ => (g.ends.target.y - g.ends.source.y).atan2(g.ends.target.x - g.ends.source.x),
                    };
                    (g.path.label, angle, color)
                });
                PaintEdge {
                    segments: g.path.segments.clone(),
                    style,
                    selected: e.selected,
                    hovered: self.hovered_edge.as_ref() == Some(&e.id),
                    arrow,
                }
            })
            .collect();
        let bounds_cell = self.bounds.clone();
        let back = canvas(
            move |b, _, _| bounds_cell.set(b),
            move |b, _, window, cx| {
                let ctx = LayerContext { viewport: vp, bounds: b };
                window.paint_quad(gpui::fill(b, theme.background));
                if let Some(opts) = background {
                    let color = opts.color.unwrap_or(theme.pattern);
                    if let Some(bg) = opts.background {
                        window.paint_quad(gpui::fill(b, bg));
                    }
                    crate::background::paint(window, b, &vp, &opts, color);
                }
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for layer in &back_layers {
                        layer(&ctx, window, cx);
                    }
                    for e in &paint_edges {
                        paint_edge(window, b.origin, &vp, e, theme.edge_hover);
                    }
                });
            },
        )
        .absolute()
        .size_full();

        // Edge labels.
        let labels: Vec<AnyElement> = self
            .edge_geoms
            .iter()
            .filter_map(|g| {
                let e = &self.edges[g.index];
                let label = e.label.clone()?;
                let at = vp.to_screen(g.path.label);
                let id = e.id.clone();
                let on_down = cx.listener(move |this: &mut Flow, ev: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    this.focus_flow(w, cx);
                    let id = id.clone();
                    this.apply_selection(|_| false, |x| *x == id, cx);
                    cx.emit(FlowEvent::EdgeClick { id, modifiers: ev.modifiers });
                });
                Some(
                    div()
                        .absolute()
                        .left(px(at.x - 100.0))
                        .top(px(at.y - 10.0))
                        .w(px(200.0))
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px(px(5.0 * vp.zoom))
                                .h(px(14.0 * vp.zoom))
                                .flex()
                                .items_center()
                                .rounded(px(7.0 * vp.zoom))
                                .bg(label.background)
                                .text_color(label.color)
                                .text_size(px(9.0 * vp.zoom))
                                .when_some(label.border, |d, c| d.border(px(1.0)).border_color(c))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, on_down)
                                .child(label.text),
                        )
                        .into_any_element(),
                )
            })
            .collect();

        // Nodes.
        let mut order: Vec<usize> = (0..self.nodes.len()).filter(|&i| !self.nodes[i].hidden).collect();
        order.sort_by_key(|&i| self.lookup.get(&self.nodes[i].id).map_or(0, |n| n.z));
        let mut node_elements = Vec::with_capacity(order.len());
        for i in order {
            let node = self.nodes[i].clone();
            let Some(internal) = self.lookup.get(&node.id) else {
                continue;
            };
            let props = NodeProps {
                id: node.id.clone(),
                selected: node.selected,
                dragging: node.dragging,
                zoom: vp.zoom,
                size: internal.size,
                connectable: internal.connectable,
                theme,
                connections: connections.get(&node.id).cloned().unwrap_or_default(),
                connection: connection.clone(),
                flow: entity.clone(),
                measure: self.measure.clone(),
            };
            let body = match &node.render {
                Some(f) => f(&props, window, cx),
                None => default_node(&node, &props),
            };
            let at = vp.to_screen(internal.position);
            let measured = internal.size.is_some();
            let draggable = self.node_draggable(&node);
            let measure = self.measure.clone();
            let measure_entity = entity.clone();
            let (measure_id, zoom) = (node.id.clone(), vp.zoom);
            let measurer = canvas(
                move |bounds, _, cx| {
                    let mut m = measure.borrow_mut();
                    let changed = match m.nodes.get_mut(&measure_id) {
                        Some(entry) => {
                            let moved = (f32::from(entry.bounds.size.width) - f32::from(bounds.size.width)).abs() > 0.5
                                || (f32::from(entry.bounds.size.height) - f32::from(bounds.size.height)).abs() > 0.5
                                || entry.zoom != zoom;
                            entry.bounds = bounds;
                            entry.zoom = zoom;
                            entry.previous = std::mem::take(&mut entry.handles);
                            moved
                        }
                        None => {
                            m.nodes.insert(
                                measure_id.clone(),
                                NodeMeasure {
                                    bounds,
                                    zoom,
                                    handles: HashMap::new(),
                                    previous: HashMap::new(),
                                },
                            );
                            true
                        }
                    };
                    drop(m);
                    if changed {
                        measure_entity.update(cx, |_, cx| cx.notify()).ok();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full();
            let id = node.id.clone();
            let down = |id: Id| {
                cx.listener(move |this: &mut Flow, e: &MouseDownEvent, w, cx| this.node_down(id.clone(), e, w, cx))
            };
            let mut wrapper = div()
                .absolute()
                .left(px(at.x))
                .top(px(at.y))
                .block_mouse_except_scroll()
                .cursor(if draggable { CursorStyle::OpenHand } else { CursorStyle::Arrow })
                .on_mouse_down(MouseButton::Left, down(id.clone()))
                .on_mouse_down(MouseButton::Right, down(id.clone()))
                .on_mouse_down(MouseButton::Middle, down(id));
            if !measured {
                wrapper = wrapper.opacity(0.0);
            }
            if let Some(w) = node.width {
                wrapper = wrapper.w(px(w * vp.zoom));
            }
            if let Some(h) = node.height {
                wrapper = wrapper.h(px(h * vp.zoom));
            }
            node_elements.push(wrapper.child(measurer).child(body).into_any_element());
        }

        // Portals.
        let portal = |front: bool, window: &mut Window, cx: &mut Context<Self>| -> Vec<AnyElement> {
            self.layers
                .portals
                .iter()
                .filter(|p| p.front == front)
                .map(|p| {
                    let at = vp.to_screen(p.position);
                    let props = NodeProps {
                        id: "".into(),
                        selected: false,
                        dragging: false,
                        zoom: vp.zoom,
                        size: None,
                        connectable: false,
                        theme,
                        connections: Rc::default(),
                        connection: None,
                        flow: entity.clone(),
                        measure: self.measure.clone(),
                    };
                    div()
                        .absolute()
                        .left(px(at.x))
                        .top(px(at.y))
                        .child((p.render)(&props, window, cx))
                        .into_any_element()
                })
                .collect()
        };
        let back_portals = portal(false, window, cx);
        let front_portals = portal(true, window, cx);

        // Front layer: connection line, selection boxes, listeners.
        let connection_line = self.connection.clone().map(|c| {
            let params = PathParams::new(c.from, c.from_position, c.to, c.to_position);
            let path = match self.options.default_edge_kind {
                EdgeKind::Straight => straight_path(c.from, c.to),
                EdgeKind::SmoothStep => smooth_step_path(params, StepOptions::default()),
                EdgeKind::Step => smooth_step_path(params, StepOptions { border_radius: 0.0, ..StepOptions::default() }),
                EdgeKind::Bezier => bezier_path(params, DEFAULT_CURVATURE),
            };
            let style = match &self.options.connection_line_style {
                Some(f) => f(&c),
                None => EdgeStyle::solid(if c.is_valid == Some(false) { theme.connection_invalid } else { theme.connection_line }),
            };
            PaintEdge { segments: path.segments, style, selected: true, hovered: false, arrow: None }
        });
        let marquee = match &self.drag {
            Drag::Select { start, current } => Some(Rect::from_corners(*start, *current)),
            _ => None,
        };
        let selected_bounds = {
            let rects: Vec<Rect> = self
                .nodes
                .iter()
                .filter(|n| n.selected && !n.hidden)
                .filter_map(|n| self.lookup.get(&n.id).filter(|i| i.size.is_some()).map(|i| i.rect()))
                .collect();
            (rects.len() > 1).then(|| bounds_of(rects)).flatten()
        };
        let on_move = entity.clone();
        let on_up = entity.clone();
        let front = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let ctx = LayerContext { viewport: vp, bounds: b };
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for layer in &front_layers {
                        layer(&ctx, window, cx);
                    }
                    let screen_rect = |r: Rect| {
                        let o = vp.to_screen(Xy::new(r.x, r.y));
                        Bounds::new(
                            point(b.origin.x + px(o.x), b.origin.y + px(o.y)),
                            size(px(r.width * vp.zoom), px(r.height * vp.zoom)),
                        )
                    };
                    if let Some(r) = selected_bounds {
                        let r = r.inflate(4.0);
                        window.paint_quad(gpui::quad(
                            screen_rect(r),
                            px(2.0),
                            theme.selection_fill,
                            px(1.0),
                            theme.nodes_selection_stroke,
                            gpui::BorderStyle::Solid,
                        ));
                    }
                    if let Some(c) = &connection_line {
                        paint_edge(window, b.origin, &vp, c, theme.edge_hover);
                    }
                    if let Some(r) = marquee {
                        let o = point(b.origin.x + px(r.x), b.origin.y + px(r.y));
                        window.paint_quad(gpui::quad(
                            Bounds::new(o, size(px(r.width), px(r.height))),
                            px(0.0),
                            theme.selection_fill,
                            px(1.0),
                            theme.selection_stroke,
                            gpui::BorderStyle::Solid,
                        ));
                    }
                });
                // Drags follow the pointer beyond the canvas.
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        on_move.update(cx, |this, cx| this.mouse_move(e, window, cx)).ok();
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        on_up.update(cx, |this, cx| this.mouse_up(e, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let panels = self.render_panels(window, cx);
        let cursor = match &self.drag {
            Drag::Pan { moved: true, .. } => CursorStyle::ClosedHand,
            Drag::Select { .. } => CursorStyle::Crosshair,
            Drag::Connect => CursorStyle::Crosshair,
            _ if self.hovered_edge.is_some() => CursorStyle::PointingHand,
            _ => CursorStyle::OpenHand,
        };

        div()
            .id("flow")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e, w, cx| this.pane_down(e, w, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, e, w, cx| this.pane_down(e, w, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e, w, cx| this.pane_down(e, w, cx)))
            .on_scroll_wheel(cx.listener(|this, e, _, cx| this.scroll(e, cx)))
            .on_key_down(cx.listener(|this, e, _, cx| this.key_down(e, cx)))
            .child(back)
            .children(back_portals)
            .children(labels)
            .children(node_elements)
            .children(front_portals)
            .child(front)
            .children(panels)
    }
}
