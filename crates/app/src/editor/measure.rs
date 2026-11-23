//! Node geometry of documents the editor isn't showing. Each document is shown in
//! an editor inside a window that is never shown, so its nodes are built and
//! painted by the same code as the open editor's, with the same text system.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use gpui::{
    App, AppContext as _, Bounds, WindowBounds, WindowHandle, WindowOptions, point, px, size,
};
use shader_graph::graph::{GraphModel, GraphProject};
use shader_graph::layout::NodeGeometry;

use super::Editor;
use super::handle::ModelHandle;

/// The first paint measures the nodes; the next render builds the lookup from them,
/// and a third draw confirms nothing moved.
const MAX_DRAWS: usize = 8;

/// The node geometry of documents, measured in a window that is never shown. The
/// window stays open until `close`, so a caller that is about to quit need not close it.
pub(crate) struct Measurements {
    /// By document key and then node id. A document no group node leads to can't be
    /// shown, so it is left out.
    pub geometry: HashMap<String, HashMap<String, NodeGeometry>>,
    window: WindowHandle<Editor>,
}

impl Measurements {
    /// Closes the window. A window closed while the platform still has a frame pending
    /// logs that the window is missing, so only close one the app keeps running with.
    pub(crate) fn close(self, cx: &mut App) {
        self.window
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// Measures the nodes of each of `definitions` as the editor draws them.
pub(crate) fn measure_documents(
    project: &GraphProject,
    definitions: &[String],
    cx: &mut App,
) -> Result<Measurements> {
    let bounds = Bounds {
        origin: point(px(0.0), px(0.0)),
        size: size(px(1280.0), px(800.0)),
    };
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: false,
                show: false,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Editor::offscreen(project.clone(), window, cx)),
        )
        .context("opening a window to measure the nodes in")?;
    match measure_in(window, definitions, cx) {
        Ok(geometry) => Ok(Measurements { geometry, window }),
        Err(error) => {
            Measurements {
                geometry: HashMap::new(),
                window,
            }
            .close(cx);
            Err(error)
        }
    }
}

fn measure_in(
    window: WindowHandle<Editor>,
    definitions: &[String],
    cx: &mut App,
) -> Result<HashMap<String, HashMap<String, NodeGeometry>>> {
    let mut measured = HashMap::new();
    for definition in definitions {
        if !window.update(cx, |editor, _, cx| editor.show_definition(definition, cx))? {
            continue;
        }
        measured.insert(definition.clone(), draw_until_settled(window, cx)?);
    }
    Ok(measured)
}

fn draw_until_settled(
    window: WindowHandle<Editor>,
    cx: &mut App,
) -> Result<HashMap<String, NodeGeometry>> {
    let mut previous: Option<HashMap<String, NodeGeometry>> = None;
    for _ in 0..MAX_DRAWS {
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        let now = window.update(cx, |editor, _, cx| editor.drawn_geometry(cx))?;
        if previous.as_ref() == Some(&now) {
            return Ok(now);
        }
        previous = Some(now);
    }
    log::warn!("the measurements of a document did not settle after {MAX_DRAWS} draws");
    Ok(previous.unwrap_or_default())
}

impl Editor {
    /// An editor on `project`, for a window that is never shown. It keeps the zoom it
    /// starts at rather than fitting the window: node sizes are pixels divided by zoom,
    /// so zoom 1 is where pixel snapping costs least.
    pub(crate) fn offscreen(
        project: GraphProject,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let editor = Self::build(
            ModelHandle::new(GraphModel::new(project)),
            false,
            window,
            cx,
        );
        editor
            .flow
            .update(cx, |flow, _| flow.options.fit_view_on_init = false);
        editor
    }

    /// Shows the document `definition_ref` leads to, as entering its group node
    /// does. False when no group node leads to it.
    fn show_definition(&mut self, definition_ref: &str, cx: &mut gpui::Context<Self>) -> bool {
        let Some(path) = self.model().path_to_definition(definition_ref) else {
            return false;
        };
        self.model_mut().set_path(path);
        self.sync(cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use shader_graph::graph::{Point, ROOT_ID};
    use shader_graph::nodes::IMAGE;

    use super::*;

    /// A root with a frame, a grouped image node, and a group node leading to its document.
    fn project() -> (GraphProject, String) {
        let mut model = GraphModel::default();
        let uv = model.add_node("uv", Point::new(0.0, 0.0)).expect("adds");
        let sin = model.add_node("sin", Point::new(220.0, 0.0)).expect("adds");
        let image = model.add_node(IMAGE, Point::new(0.0, 220.0)).expect("adds");
        model.frame_nodes(&[sin.clone(), uv]).expect("frames");
        model.group_selection(&[image]).expect("groups");
        let group = model
            .root()
            .nodes
            .iter()
            .find(|n| n.is("node-group"))
            .expect("a group node");
        let reference = group.definition_ref().expect("a definition").to_owned();
        (model.project().clone(), reference)
    }

    fn settle(cx: &mut gpui::VisualTestContext) {
        for _ in 0..6 {
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
    }

    fn assert_same(id: &str, drawn: &NodeGeometry, measured: &NodeGeometry) {
        // The open editor measures at its own zoom and the offscreen one at zoom 1, so
        // they agree to within a pixel at zoom 1.
        const EPSILON: f32 = 1.0;
        let close = |a: f32, b: f32| (a - b).abs() <= EPSILON;
        assert!(
            close(drawn.width, measured.width) && close(drawn.height, measured.height),
            "{id}: drawn {}x{}, measured {}x{}",
            drawn.width,
            drawn.height,
            measured.width,
            measured.height
        );
        for (drawn, measured) in [
            (&drawn.inputs, &measured.inputs),
            (&drawn.outputs, &measured.outputs),
        ] {
            let names =
                |list: &[(String, f32)]| list.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
            assert_eq!(names(drawn), names(measured), "{id}: handles");
            for ((_, a), (_, b)) in drawn.iter().zip(measured) {
                assert!(close(*a, *b), "{id}: handle offset {a} against {b}");
            }
        }
    }

    /// Measured documents match what the open editor draws for the same nodes, at the root
    /// and in a group document.
    #[gpui::test]
    fn measured_nodes_match_what_the_flow_draws(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (project, group_ref) = project();
        let (editor, cx) = cx.add_window_view(Editor::new);
        editor.update(cx, |e, cx| e.load_project(Some(project.clone()), None, cx));
        settle(cx);

        let measured = cx
            .update(|_, app| {
                measure_documents(&project, &[ROOT_ID.to_owned(), group_ref.clone()], app)
            })
            .expect("measures")
            .geometry;

        let root = editor.read_with(cx, |e, cx| e.drawn_geometry(cx));
        assert_eq!(root.len(), project.root().nodes.len());
        for (id, drawn) in &root {
            assert!(drawn.width > 0.0 && drawn.height > 0.0, "{id} is drawn");
            assert_same(id, drawn, &measured[ROOT_ID][id]);
        }

        let group_node = project
            .root()
            .nodes
            .iter()
            .find(|n| n.is("node-group"))
            .expect("a group node")
            .id
            .clone();
        editor.update(cx, |e, cx| {
            e.model_mut().set_path(vec![group_node]);
            e.sync(cx);
        });
        settle(cx);
        let inside = editor.read_with(cx, |e, cx| e.drawn_geometry(cx));
        assert!(!inside.is_empty(), "the group holds an image node");
        for (id, drawn) in &inside {
            assert_same(id, drawn, &measured[&group_ref][id]);
        }
    }
}
