//! What a node's renderer receives, the `Handle` element it places, and the
//! measurements the flow takes of both while painting.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    Bounds, Div, InteractiveElement as _, IntoElement, MouseButton, ParentElement, Pixels,
    RenderOnce, StyleRefinement, Styled, WeakEntity, canvas, div, point, px, relative,
};

use crate::edge_path::Position;
use crate::flow::Flow;
use crate::model::{ConnectionState, HandleType, Id};
use crate::theme::FlowTheme;

#[derive(Default)]
pub(crate) struct Measure {
    pub nodes: HashMap<Id, NodeMeasure>,
    /// Bumped whenever a measurement changes size, place or handle set.
    pub generation: u64,
}

pub(crate) type HandleKey = (Option<Id>, HandleType);

pub(crate) struct NodeMeasure {
    pub bounds: Bounds<Pixels>,
    /// Zero until the node's measurer has run, which is how the lookup tells a node is unmeasured.
    pub zoom: f32,
    /// The handles painted in the last completed paint of the node.
    pub handles: HashMap<HandleKey, HandleMeasure>,
    /// The handles painted so far in the current paint. The node's commit canvas, painted
    /// after the body, moves them into `handles`.
    pub painting: HashMap<HandleKey, HandleMeasure>,
}

impl NodeMeasure {
    pub fn unmeasured() -> Self {
        Self {
            bounds: Bounds::default(),
            zoom: 0.0,
            handles: HashMap::new(),
            painting: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct HandleMeasure {
    /// Relative to the node's top-left corner, in pixels.
    pub bounds: Bounds<Pixels>,
    pub position: Position,
    pub connectable: bool,
    pub connectable_start: bool,
    pub connectable_end: bool,
}

/// Whether a paint measured the node's handles differently from the last one.
pub(crate) fn handles_changed(
    before: &HashMap<HandleKey, HandleMeasure>,
    now: &HashMap<HandleKey, HandleMeasure>,
) -> bool {
    if before.len() != now.len() {
        return true;
    }
    now.iter().any(|(key, h)| {
        before.get(key).is_none_or(|p| {
            let (a, b) = (&p.bounds, &h.bounds);
            (f32::from(a.origin.x) - f32::from(b.origin.x)).abs() > 0.5
                || (f32::from(a.origin.y) - f32::from(b.origin.y)).abs() > 0.5
                || (f32::from(a.size.width) - f32::from(b.size.width)).abs() > 0.5
                || (f32::from(a.size.height) - f32::from(b.size.height)).abs() > 0.5
                || p.position != h.position
                || p.connectable != h.connectable
                || p.connectable_start != h.connectable_start
                || p.connectable_end != h.connectable_end
        })
    })
}

/// The links touching a node, by handle.
#[derive(Default, Debug)]
pub struct NodeConnections {
    /// Target handles linked to a source, with the edge ids.
    pub incoming: Vec<(Option<Id>, Id)>,
    pub outgoing: Vec<(Option<Id>, Id)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandleState {
    pub connected: bool,
    /// This handle is where the connection in progress started.
    pub connecting_from: bool,
    /// The connection in progress is over this handle.
    pub connecting_to: bool,
    pub valid: bool,
}

#[derive(Clone)]
pub struct NodeProps {
    pub id: Id,
    pub selected: bool,
    pub dragging: bool,
    pub zoom: f32,
    /// Measured or fixed size, in flow units.
    pub size: Option<(f32, f32)>,
    pub connectable: bool,
    pub theme: FlowTheme,
    pub connections: Rc<NodeConnections>,
    pub connection: Option<Rc<ConnectionState>>,
    pub(crate) flow: WeakEntity<Flow>,
    pub(crate) measure: Rc<RefCell<Measure>>,
}

impl NodeProps {
    /// A length in flow units, as pixels at the current zoom. Everything
    /// inside a node is drawn through this so it scales with the viewport.
    pub fn scaled(&self, flow_units: f32) -> Pixels {
        px(flow_units * self.zoom)
    }

    pub fn is_connected(&self, kind: HandleType, handle: Option<&Id>) -> bool {
        let list = match kind {
            HandleType::Target => &self.connections.incoming,
            HandleType::Source => &self.connections.outgoing,
        };
        list.iter().any(|(h, _)| h.as_ref() == handle)
    }

    /// The handle ids with a link, as `useNodeConnections` gives.
    pub fn connected_handles(&self, kind: HandleType) -> HashSet<Option<Id>> {
        let list = match kind {
            HandleType::Target => &self.connections.incoming,
            HandleType::Source => &self.connections.outgoing,
        };
        list.iter().map(|(h, _)| h.clone()).collect()
    }

    pub fn handle_state(&self, kind: HandleType, handle: Option<&Id>) -> HandleState {
        let mut state = HandleState {
            connected: self.is_connected(kind, handle),
            ..HandleState::default()
        };
        if let Some(c) = &self.connection {
            let from =
                c.from_node == self.id && c.from_type == kind && c.from_handle.as_ref() == handle;
            let to = c.to_node.as_ref() == Some(&self.id)
                && c.to_handle.as_ref() == handle
                && c.from_type != kind;
            state.connecting_from = from;
            state.connecting_to = to;
            state.valid = to && c.is_valid == Some(true);
        }
        state
    }

    /// A connection point. Positioned at the node's border
    /// for `position`, in the nearest positioned ancestor.
    pub fn handle(
        &self,
        id: impl Into<Option<Id>>,
        kind: HandleType,
        position: Position,
    ) -> Handle {
        let id = id.into();
        Handle {
            base: div(),
            node: self.id.clone(),
            id: id.clone(),
            kind,
            position,
            size: 8.0,
            zoom: self.zoom,
            free: false,
            connectable: self.connectable,
            connectable_start: true,
            connectable_end: true,
            state: self.handle_state(kind, id.as_ref()),
            theme: self.theme,
            flow: self.flow.clone(),
            measure: self.measure.clone(),
            custom_look: false,
        }
    }
}

/// A node's connection point: the element the pointer grabs to start a
/// connection, measured so edges can attach to it.
#[derive(IntoElement)]
pub struct Handle {
    base: Div,
    node: Id,
    id: Option<Id>,
    kind: HandleType,
    position: Position,
    size: f32,
    zoom: f32,
    free: bool,
    connectable: bool,
    connectable_start: bool,
    connectable_end: bool,
    state: HandleState,
    theme: FlowTheme,
    flow: WeakEntity<Flow>,
    measure: Rc<RefCell<Measure>>,
    custom_look: bool,
}

impl Handle {
    /// Size in flow units; the border placement is recomputed from it.
    pub fn sized(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Leaves placement to the caller's layout instead of the node's border.
    pub fn free(mut self) -> Self {
        self.free = true;
        self
    }

    pub fn connectable(mut self, connectable: bool) -> Self {
        self.connectable = connectable;
        self
    }

    pub fn connectable_start(mut self, connectable: bool) -> Self {
        self.connectable_start = connectable;
        self
    }

    pub fn connectable_end(mut self, connectable: bool) -> Self {
        self.connectable_end = connectable;
        self
    }

    /// Skips the default dot, for handles that draw their own shape.
    pub fn custom_look(mut self) -> Self {
        self.custom_look = true;
        self
    }
}

impl Styled for Handle {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Handle {
    fn extend(&mut self, elements: impl IntoIterator<Item = gpui::AnyElement>) {
        self.base.extend(elements);
    }
}

impl RenderOnce for Handle {
    fn render(self, _: &mut gpui::Window, _: &mut gpui::App) -> impl IntoElement {
        guard::render("flow handle", || {
            let z = self.zoom;
            let size = px(self.size * z);
            let half = px(-self.size * z / 2.0);
            let mut base = self.base.size(size);
            if !self.free {
                base = base.absolute();
                base = match self.position {
                    Position::Left => base.left(half).top(relative(0.5)).mt(half),
                    Position::Right => base.right(half).top(relative(0.5)).mt(half),
                    Position::Top => base.top(half).left(relative(0.5)).ml(half),
                    Position::Bottom => base.bottom(half).left(relative(0.5)).ml(half),
                };
            }
            if !self.custom_look {
                let theme = self.theme;
                let color = if self.state.connecting_to {
                    if self.state.valid {
                        theme.handle_valid
                    } else {
                        theme.handle_invalid
                    }
                } else {
                    theme.handle
                };
                base = base
                    .rounded_full()
                    .bg(color)
                    .border(px(z.max(0.5)))
                    .border_color(theme.handle_border);
            }

            let key = (self.id.clone(), self.kind);
            let node = self.node.clone();
            let position = self.position;
            let (connectable, start, end) = (
                self.connectable,
                self.connectable_start,
                self.connectable_end,
            );
            let measure = self.measure.clone();
            let (measure_key, measure_node) = (key.clone(), node.clone());
            let measurer = canvas(
                move |bounds, _, _| {
                    guard::guard("node measure", || {
                        let mut m = measure.borrow_mut();
                        let Some(entry) = m.nodes.get_mut(&measure_node) else {
                            return;
                        };
                        let origin = entry.bounds.origin;
                        entry.painting.insert(
                            measure_key.clone(),
                            HandleMeasure {
                                bounds: Bounds {
                                    origin: point(
                                        bounds.origin.x - origin.x,
                                        bounds.origin.y - origin.y,
                                    ),
                                    size: bounds.size,
                                },
                                position,
                                connectable,
                                connectable_start: start,
                                connectable_end: end,
                            },
                        );
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full();

            let down_flow = self.flow.clone();
            let kind = self.kind;
            let id = self.id.clone();
            let can_start = connectable && start;
            base.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                guard::guard("flow handle", || {
                    cx.stop_propagation();
                    if !can_start {
                        return;
                    }
                    let (node, id) = (node.clone(), id.clone());
                    let event = event.clone();
                    down_flow
                        .update(cx, |flow, cx| {
                            flow.start_connection(node, id, kind, &event, window, cx)
                        })
                        .ok();
                });
            })
            .child(measurer)
        })
    }
}
