//! Left-to-right layout of a document's nodes, computed by ELK. Each zone
//! and frame is laid out as a block around the nodes it holds.

mod branches;
mod compute;
mod elk;
mod geometry;

pub use branches::{RerouteBranches, reroute_branches};
pub use compute::{LayoutInput, compute_layout};
pub use geometry::{InputRow, NodeGeometry, NodeRows, estimate_node_geometry};

use crate::graph::GraphNode;

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("the layout engine failed: {0}")]
    Elk(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutSpacing {
    /// Horizontal gap between consecutive columns of nodes.
    pub between_layers: f32,
    /// Vertical gap between nodes of a column.
    pub between_nodes: f32,
    /// Room kept around the nodes of a repeat zone, for its bubble.
    pub zone_padding: f32,
}

impl Default for LayoutSpacing {
    fn default() -> Self {
        Self {
            between_layers: 80.0,
            between_nodes: 32.0,
            zone_padding: 40.0,
        }
    }
}

/// Gives a node's geometry as drawn (`None` to fall back on an estimate),
/// called with the key of the document the node is in.
pub type Measure<'a> = &'a dyn Fn(&str, &GraphNode) -> Option<NodeGeometry>;

#[derive(Default)]
pub struct LayoutOptions<'a> {
    pub measure: Option<Measure<'a>>,
    pub spacing: LayoutSpacing,
}
