//! A node editor canvas for GPUI.
//!
//! [`Flow`] is controlled: it shows the nodes and edges it's given, applies
//! interactions to its own copy so they feel immediate, and reports them as
//! [`FlowEvent`]s. The owner applies those to its model (with
//! [`apply_node_changes`] and friends, or its own logic) and hands the result
//! back with [`Flow::set_nodes`] and [`Flow::set_edges`].

mod background;
pub mod edge_path;
mod flow;
pub mod geometry;
pub mod lookup;
pub mod model;
mod node;
mod panels;
mod stroke;
mod theme;

pub use background::{BackgroundOptions, BackgroundVariant};
pub use edge_path::{EdgePath, PathParams, Position, Segment, StepOptions};
pub use flow::{
    ConnectionLineStyle, ConnectionValidator,
    FitViewOptions, Flow, FlowEvent, FlowOptions, Layer, LayerContext, Layers, Portal,
    PortalRender, SelectionMode, StrokeKind,
};
pub use geometry::{Extent, Rect, Viewport, Xy};
pub use model::{
    Connection, ConnectionMode, ConnectionState, Edge, EdgeChange, EdgeColor, EdgeKind, EdgeTag,
    EdgeLabel, EdgeStyle, HandleType, Id, Node, NodeChange, NodeKind, add_edge,
    apply_edge_changes, apply_node_changes, connected_edges, edge_id, incomers, outgoers,
    reconnect_edge,
};
pub use node::{Handle, HandleState, NodeConnections, NodeProps};
pub use panels::{ControlButton, ControlsOptions, MiniMapOptions, Panel, PanelPosition};
pub use theme::FlowTheme;
