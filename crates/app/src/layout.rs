//! The screen's layout as the project file keeps it: the areas, their
//! rectangles and what each shows, with a graph area's group path and camera.

use std::collections::HashSet;

use screen::{AreaId, Rect, Screen};
use serde_json::{Value, json};
use shader_graph::graph::{GraphProject, Viewport};

use crate::shell::Panel;

/// The key of the layout in the project's JSON.
const KEY: &str = "layout";
/// What a path may hold, as the original's schema limits it.
const MAX_PATH: usize = 64;
const MAX_AREAS: usize = 256;

/// Which group a graph area was entered, and where its camera was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphView {
    pub path: Vec<String>,
    pub viewport: Option<Viewport>,
}

pub struct Layout {
    pub screen: Screen<Panel>,
    /// Where each graph area was.
    pub graphs: Vec<(AreaId, GraphView)>,
}

fn editor_json(panel: &Panel, graph: &GraphView) -> Value {
    match panel {
        Panel::Graph => {
            let mut editor = json!({ "type": "graph", "path": graph.path });
            if let (Some(v), Some(map)) = (&graph.viewport, editor.as_object_mut()) {
                map.insert("viewport".to_owned(), json!({ "x": v.x, "y": v.y, "zoom": v.zoom }));
            }
            editor
        }
        Panel::Preview => json!({ "type": "preview" }),
        Panel::Timeline => json!({ "type": "timeline" }),
        Panel::Code => json!({ "type": "code" }),
        Panel::Assets => json!({ "type": "assets" }),
    }
}

/// Stores the layout in the project, replacing what it had. `graph` says where a graph area is.
pub fn store(project: &mut GraphProject, screen: &Screen<Panel>, graph: &dyn Fn(AreaId) -> GraphView) {
    let areas: Vec<Value> = screen
        .areas()
        .iter()
        .map(|area| {
            json!({
                "id": format!("area-{}", area.id.0),
                "rect": {
                    "left": area.rect.left,
                    "top": area.rect.top,
                    "right": area.rect.right,
                    "bottom": area.rect.bottom,
                },
                "editor": editor_json(&area.editor, &graph(area.id)),
            })
        })
        .collect();
    project.extra.insert(KEY.to_owned(), json!({ "areas": areas }));
}

fn fraction(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite() && (0.0..=1.0).contains(f))
}

fn viewport(v: &Value) -> Option<Viewport> {
    let number = |key: &str| v.get(key)?.as_f64().filter(|f| f.is_finite());
    let zoom = number("zoom").filter(|z| *z > 0.0)?;
    Some(Viewport { x: number("x")? as f32, y: number("y")? as f32, zoom: zoom as f32 })
}

fn editor_from_json(v: &Value) -> Option<(Panel, Option<GraphView>)> {
    match v.get("type")?.as_str()? {
        "graph" => {
            let path: Vec<String> = v
                .get("path")?
                .as_array()?
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect::<Option<_>>()?;
            (path.len() <= MAX_PATH).then_some(())?;
            let view = GraphView { path, viewport: v.get("viewport").and_then(viewport) };
            Some((Panel::Graph, Some(view)))
        }
        "preview" => Some((Panel::Preview, None)),
        "timeline" => Some((Panel::Timeline, None)),
        "code" => Some((Panel::Code, None)),
        "assets" => Some((Panel::Assets, None)),
        _ => None,
    }
}

/// The layout a project was saved with. `None` when it has none or it isn't a valid tiling of the window.
pub fn load(project: &GraphProject) -> Option<Layout> {
    let areas = project.extra.get(KEY)?.get("areas")?.as_array()?;
    if areas.is_empty() || areas.len() > MAX_AREAS {
        return None;
    }
    let mut ids = HashSet::new();
    let mut views = Vec::new();
    let mut parsed = Vec::new();
    for area in areas {
        let id = area.get("id")?.as_str()?;
        ids.insert(id);
        let rect = area.get("rect")?;
        let side = |key: &str| fraction(rect.get(key)?);
        let rect = Rect::new(side("left")?, side("top")?, side("right")?, side("bottom")?);
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return None;
        }
        let (panel, view) = editor_from_json(area.get("editor")?)?;
        views.push(view);
        parsed.push((rect, panel));
    }
    if ids.len() != areas.len() {
        return None;
    }
    let screen = Screen::new(parsed);
    screen.validate().ok()?;
    let graphs = screen.areas().iter().zip(views).filter_map(|(area, view)| Some((area.id, view?))).collect();
    Some(Layout { screen, graphs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen<Panel> {
        Screen::new([
            (Rect::new(0.0, 0.0, 0.3, 1.0), Panel::Preview),
            (Rect::new(0.3, 0.0, 1.0, 0.8), Panel::Graph),
            (Rect::new(0.3, 0.8, 1.0, 1.0), Panel::Timeline),
        ])
    }

    #[test]
    fn a_stored_layout_loads_back() {
        let mut project = GraphProject::default();
        let view = GraphView {
            path: vec!["a".into(), "b".into()],
            viewport: Some(Viewport { x: 1.0, y: -2.0, zoom: 0.5 }),
        };
        let shown = view.clone();
        store(&mut project, &screen(), &move |_| shown.clone());
        let layout = load(&project).expect("loads");
        assert_eq!(layout.graphs.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), [view]);
        let editors: Vec<&Panel> = layout.screen.areas().iter().map(|a| &a.editor).collect();
        assert_eq!(editors, [&Panel::Preview, &Panel::Graph, &Panel::Timeline]);
        assert_eq!(layout.screen.areas()[1].rect, Rect::new(0.3, 0.0, 1.0, 0.8));
    }

    #[test]
    fn each_graph_area_keeps_its_own_view() {
        let screen = Screen::new([
            (Rect::new(0.0, 0.0, 0.5, 1.0), Panel::Graph),
            (Rect::new(0.5, 0.0, 1.0, 1.0), Panel::Graph),
        ]);
        let ids: Vec<AreaId> = screen.areas().iter().map(|a| a.id).collect();
        let first = GraphView { path: vec!["a".into()], viewport: None };
        let second = GraphView { path: Vec::new(), viewport: Some(Viewport { x: 3.0, y: 4.0, zoom: 2.0 }) };
        let (f, s) = (first.clone(), second.clone());
        let first_id = ids[0];
        let mut project = GraphProject::default();
        store(&mut project, &screen, &move |id| if id == first_id { f.clone() } else { s.clone() });
        let layout = load(&project).expect("loads");
        let views: Vec<GraphView> = layout.graphs.into_iter().map(|(_, v)| v).collect();
        assert_eq!(views, [first, second]);
    }

    #[test]
    fn the_layout_survives_a_project_round_trip_in_the_originals_shape() {
        let mut project = GraphProject::default();
        store(&mut project, &screen(), &|_| GraphView::default());
        let json = project.to_json();
        let loaded = GraphProject::from_json(&json).expect("loads");
        let layout = loaded.extra.get("layout").expect("kept");
        assert_eq!(layout["areas"][1]["editor"], json!({ "type": "graph", "path": [] }));
        assert_eq!(layout["areas"][0]["rect"]["right"], json!(0.3));
        assert!(load(&loaded).is_some());
    }

    #[test]
    fn a_project_without_a_layout_has_none() {
        assert!(load(&GraphProject::default()).is_none());
    }

    #[test]
    fn broken_layouts_are_refused() {
        let broken = |layout: Value| {
            let mut project = GraphProject::default();
            project.extra.insert("layout".to_owned(), layout);
            load(&project).is_none()
        };
        assert!(broken(json!({ "areas": [] })));
        assert!(broken(json!({ "areas": "none" })));
        let area = |id: &str, right: f64, kind: &str| {
            json!({ "id": id, "rect": { "left": 0, "top": 0, "right": right, "bottom": 1 }, "editor": { "type": kind } })
        };
        // Not covering the window.
        assert!(broken(json!({ "areas": [area("a", 0.5, "preview")] })));
        // Out of range, empty, an unknown editor, a repeated id.
        assert!(broken(json!({ "areas": [area("a", 2.0, "preview")] })));
        assert!(broken(json!({ "areas": [area("a", 0.0, "preview")] })));
        assert!(broken(json!({ "areas": [area("a", 1.0, "nonsense")] })));
        assert!(!broken(json!({ "areas": [area("a", 1.0, "preview")] })));
        let doubled = json!({ "areas": [
            { "id": "a", "rect": { "left": 0, "top": 0, "right": 0.5, "bottom": 1 }, "editor": { "type": "preview" } },
            { "id": "a", "rect": { "left": 0.5, "top": 0, "right": 1, "bottom": 1 }, "editor": { "type": "timeline" } },
        ] });
        assert!(broken(doubled));
        // A graph area needs its path.
        assert!(broken(json!({ "areas": [area("a", 1.0, "graph")] })));
    }

    #[test]
    fn assets_areas_load_as_the_assets_panel() {
        let mut project = GraphProject::default();
        project.extra.insert(
            "layout".to_owned(),
            json!({ "areas": [
                { "id": "a", "rect": { "left": 0, "top": 0, "right": 1, "bottom": 1 }, "editor": { "type": "assets" } },
            ] }),
        );
        assert_eq!(load(&project).map(|l| l.screen.areas()[0].editor), Some(Panel::Assets));
    }
}
