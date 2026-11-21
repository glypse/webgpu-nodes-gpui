//! Node sizes and handle offsets, as the front end draws them, or estimated
//! from what a node shows when they can't be measured.

/// A node's size and the vertical offset (from its top) of each of its handles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeGeometry {
    pub width: f32,
    pub height: f32,
    /// Handle id and its offset, in the order the handles are drawn.
    pub inputs: Vec<(String, f32)>,
    pub outputs: Vec<(String, f32)>,
}

/// What a node shows, top to bottom, for estimating its geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeRows {
    /// Rows above the handles (settings, type picker…), the title excluded.
    pub header: usize,
    pub outputs: Vec<String>,
    /// Rows taken by a widget between the outputs and the inputs.
    pub widget: usize,
    pub inputs: Vec<InputRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputRow {
    pub id: String,
    /// The input shows a control under its label.
    pub editor: bool,
}

// Only used when the front end can't measure a node (e.g. headless): a rough
// guess from what it shows, erring on the large side.
const ESTIMATED_WIDTH: f32 = 220.0;
const ESTIMATED_TITLE: f32 = 40.0;
const ESTIMATED_ROW: f32 = 30.0;

pub fn estimate_node_geometry(rows: &NodeRows) -> NodeGeometry {
    let mut row = rows.header;
    let mut outputs = Vec::with_capacity(rows.outputs.len());
    for id in &rows.outputs {
        outputs.push((id.clone(), row_centre(row)));
        row += 1;
    }
    row += rows.widget;
    let mut inputs = Vec::with_capacity(rows.inputs.len());
    for input in &rows.inputs {
        inputs.push((input.id.clone(), row_centre(row)));
        row += if input.editor { 2 } else { 1 };
    }
    NodeGeometry {
        width: ESTIMATED_WIDTH,
        height: ESTIMATED_TITLE + ESTIMATED_ROW * (row.max(1) as f32),
        inputs,
        outputs,
    }
}

fn row_centre(row: usize) -> f32 {
    ESTIMATED_TITLE + ESTIMATED_ROW * (row as f32 + 0.5)
}
