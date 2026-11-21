//! The one place that calls ELK. Its input and output are elkjs's JSON graph
//! shape, so the rest of the layout is independent of the engine.

use serde_json::Value;

use super::LayoutError;

pub fn layout(graph: Value) -> Result<Value, LayoutError> {
    elk_layout::layout_value(graph, "{}").map_err(LayoutError::Elk)
}
