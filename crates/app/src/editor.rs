use guard::SafeListener as _;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_flow::{
    Connection as FlowConnection, ControlsOptions, Edge, EdgeStyle, Flow, FlowEvent, Id, Layers,
    Node, Viewport, Xy,
};
use shader_graph::catalog::{
    self, AddMenuEntry, Pending, add_entry, add_entry_connected, add_menu_entries,
    connectable_entries,
};
use shader_graph::codegen::Program;
use shader_graph::graph::{
    Connection, FRAG_OUTPUT, GraphModel, GraphProject, LAYOUT_FRAME, NodeClipboard,
    Point as GraphPoint, ProjectSettings, REROUTE, frame_note, frame_tint,
};
use shader_graph::types::{HandleType, InputValue, WgslType};
use shader_graph::UniformValue;
use theme::{ActiveTheme as _, Menu, MenuItem};

use crate::graph::add_menu::{AddMenu, AddMenuEvent};
use crate::graph::fields::Edit;
use crate::graph::type_picker::{self, TypePicker};
use crate::graph::frames::{self, Drag, FrameLook, SharedDrag, Snapshot, SnapshotItem, ZoneBox};
use crate::graph::node::{node_width, render_node, view_of};
use crate::graph::sheet;
use crate::graph::socket::socket_color;
use crate::graph::zones::{self, Bubble};
use crate::layout::GraphView;

/// What the shell needs from the editor.
#[derive(Clone, Debug)]
pub enum EditorEvent {
    /// The project's program changed in a way that needs compiling again,
    /// or couldn't be generated.
    Shader(Result<Program, String>),
    /// Only values the program reads from uniforms changed.
    Uniforms(Vec<(String, UniformValue)>),
    /// The project's settings changed, by an edit, a load, undo or redo.
    Settings(ProjectSettings),
}

/// What a menu entry does to the editor.
type Handler = Rc<dyn Fn(&mut Editor, &mut Context<Editor>)>;

/// An entry of a small list popup: an enum's variants, a type to pick.
pub type OnSelect = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct ListItem {
    pub label: SharedString,
    pub checked: Option<bool>,
    pub on_select: OnSelect,
    /// Text on the right: a shortcut, or what the entry holds.
    pub detail: Option<SharedString>,
    pub disabled: bool,
    pub destructive: bool,
    /// A separator goes above the entry.
    pub separated: bool,
}

impl ListItem {
    pub fn new(
        label: impl Into<SharedString>,
        checked: Option<bool>,
        on_select: OnSelect,
    ) -> Self {
        Self {
            label: label.into(),
            checked,
            on_select,
            detail: None,
            disabled: false,
            destructive: false,
            separated: false,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn separated(mut self) -> Self {
        self.separated = true;
        self
    }
}

struct Popup {
    at: Point<Pixels>,
    items: Vec<ListItem>,
    /// Which list this is: a selection can open another, which the first's dismissal mustn't close.
    serial: u64,
}

pub struct Editor {
    model: Rc<RefCell<GraphModel>>,
    flow: Entity<Flow>,
    code: String,
    generated: Result<Program, String>,
    popup: Option<Popup>,
    popup_serial: u64,
    add_menu: Option<Entity<AddMenu>>,
    viewports: HashMap<String, Viewport>,
    gesture: bool,
    /// Duplicates being grabbed with the nodes they came from, which a cancel restores.
    duplicating: Option<(Vec<String>, Vec<String>)>,
    clipboard: Option<NodeClipboard>,
    pub(crate) sheet: Option<String>,
    pub(crate) sheet_fields: sheet::Fields,
    frame_snapshot: Rc<Snapshot>,
    frame_drag: SharedDrag,
    /// Where nodes were last put in the flow, frame headers included.
    placed: HashMap<String, GraphPoint>,
    frame_settle: Option<gpui::Task<()>>,
    settings: ProjectSettings,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.flow.focus_handle(cx)
    }
}

fn demo_project() -> GraphProject {
    let mut model = GraphModel::default();
    let uv = model.add_node("uv", GraphPoint::new(0.0, 0.0));
    let output = model.root().nodes.iter().find(|n| n.is(FRAG_OUTPUT)).map(|n| n.id.clone());
    if let (Some(uv), Some(output)) = (uv, output) {
        model.try_connect(Connection::new(uv, "uv", output, "color"));
    }
    model.project().clone()
}

fn flow_viewport(v: shader_graph::graph::Viewport) -> Viewport {
    Viewport { x: v.x, y: v.y, zoom: v.zoom }
}

fn graph_viewport(v: Viewport) -> shader_graph::graph::Viewport {
    shader_graph::graph::Viewport { x: v.x, y: v.y, zoom: v.zoom }
}

fn flow_point(p: GraphPoint) -> Xy {
    Xy::new(p.x, p.y)
}

fn graph_point(p: Xy) -> GraphPoint {
    GraphPoint::new(p.x, p.y)
}

impl Editor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = Rc::new(RefCell::new(GraphModel::new(demo_project())));
        let validator = model.clone();
        let flow = cx.new(|cx| {
            let mut flow = Flow::new(cx);
            flow.options.fit_view_on_init = true;
            flow.options.delete_key = false;
            flow.options.min_zoom = 0.1;
            flow.options.max_zoom = 3.0;
            flow.options.theme = gpui_flow::FlowTheme::dark();
            flow.options.controls = Some(ControlsOptions {
                show_zoom: false,
                show_interactive: false,
                ..Default::default()
            });
            flow.options.is_valid_connection = Some(Rc::new(move |c: &FlowConnection| {
                validator.borrow().is_valid_connection(&to_model(c))
            }));
            flow
        });
        cx.subscribe(&flow, |this, _, event, cx| {
            guard::guard("flow event", || this.on_flow_event(event.clone(), cx));
        })
        .detach();
        window.focus(&flow.focus_handle(cx));
        let mut editor = Self {
            model,
            flow,
            code: String::new(),
            generated: Err(String::new()),
            popup: None,
            popup_serial: 0,
            add_menu: None,
            viewports: HashMap::new(),
            gesture: false,
            duplicating: None,
            clipboard: None,
            sheet: None,
            sheet_fields: sheet::Fields::default(),
            frame_snapshot: Rc::default(),
            frame_drag: SharedDrag::default(),
            placed: HashMap::new(),
            frame_settle: None,
            settings: ProjectSettings::default(),
        };
        editor.sync(cx);
        editor.regenerate(cx);
        editor
    }

    pub(crate) fn model(&self) -> Ref<'_, GraphModel> {
        self.model.borrow()
    }

    fn model_mut(&self) -> RefMut<'_, GraphModel> {
        self.model.borrow_mut()
    }

    /// The program of the project as it is now.
    pub fn shader(&self) -> &Result<Program, String> {
        &self.generated
    }

    /// Replaces the whole project, or starts over from the demo graph. A saved
    /// view puts the editor in the group it was left in, at the camera it had.
    pub fn load_project(&mut self, project: Option<GraphProject>, view: Option<&GraphView>, cx: &mut Context<Self>) {
        self.model_mut().set_project(project.unwrap_or_else(demo_project));
        self.viewports = self
            .model
            .borrow()
            .project()
            .definitions
            .iter()
            .filter_map(|(key, doc)| Some((key.clone(), flow_viewport(doc.viewport?))))
            .collect();
        if let Some(view) = view {
            let path = self.model.borrow().valid_path(&view.path);
            self.model_mut().set_path(path);
            if let Some(v) = view.viewport {
                let key = self.model.borrow().active_definition_ref();
                self.viewports.insert(key, flow_viewport(v));
            }
        }
        self.popup = None;
        self.add_menu = None;
        self.sheet = None;
        self.code.clear();
        *self.frame_drag.borrow_mut() = None;
        self.after_edit(cx);
        let saved = self.viewports.get(&self.model.borrow().active_definition_ref()).copied();
        self.flow.update(cx, |flow, cx| {
            flow.options.fit_view_on_init = saved.is_none();
            match saved {
                Some(vp) => flow.set_viewport(vp, None, cx),
                None => flow.fit_view(Default::default(), cx),
            }
        });
    }

    /// The project as it is now, with each group's camera as it was left.
    pub fn project(&self, cx: &App) -> GraphProject {
        let mut project = self.model.borrow().project().clone();
        let mut cameras = self.viewports.clone();
        cameras.insert(self.model.borrow().active_definition_ref(), self.flow.read(cx).viewport());
        for (key, viewport) in cameras {
            if let Some(doc) = project.definitions.get_mut(&key) {
                doc.viewport = Some(graph_viewport(viewport));
            }
        }
        project
    }

    /// The group entered and the camera on it, for saving with the layout.
    pub fn view(&self, cx: &App) -> GraphView {
        GraphView {
            path: self.model.borrow().path().to_vec(),
            viewport: Some(graph_viewport(self.flow.read(cx).viewport())),
        }
    }

    pub fn settings(&self) -> ProjectSettings {
        self.model.borrow().settings()
    }

    pub fn update_settings(&mut self, settings: ProjectSettings, cx: &mut Context<Self>) {
        if self.model_mut().update_settings(settings) {
            self.sync(cx);
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.model.borrow().has_unsaved_changes()
    }

    pub fn mark_saved(&self) {
        self.model_mut().mark_saved();
    }

    // --- Syncing the model into the flow and the shader ---

    fn sync(&mut self, cx: &mut Context<Self>) {
        let editor = cx.entity().downgrade();
        let flow_entity = self.flow.downgrade();
        let model = self.model.borrow();
        let theme = cx.theme().clone();
        let doc = model.active_document();
        let selected = model.selected_node_ids();
        let nodes: Vec<Node> = doc
            .nodes
            .iter()
            .map(|n| {
                let view = Rc::new(view_of(&model, n));
                let ed = editor.clone();
                let mut node = Node::new(n.id.clone(), flow_point(n.position))
                    .width(node_width(&n.node_type))
                    .render(move |props, window, cx| render_node(&view, props, &ed, window, cx));
                node.width = None;
                node.selected = selected.contains(&n.id);
                node.deletable = Some(n.deletable());
                node
            })
            .collect();

        let zones = model.active_zones();
        let unavailable = model.unavailable_links(&model.active_definition_ref());
        let edges: Vec<Edge> = doc
            .edges
            .iter()
            .map(|e| {
                let source = socket_color(model.resolve_output_handle_type(&e.source, &e.source_handle), &theme);
                let target = socket_color(model.resolve_input_type(&e.target, &e.target_handle), &theme);
                let mut edge = Edge::new(
                    e.id.clone(),
                    e.source.clone(),
                    Some(e.source_handle.clone().into()),
                    e.target.clone(),
                    Some(e.target_handle.clone().into()),
                );
                edge.hidden = unavailable.contains(&e.id);
                edge.style = Some(if zones.invalid_edges.contains(&e.id) {
                    EdgeStyle::solid(theme.colors.destructive).width(1.5).dash(6.0, 4.0).opacity(0.8, 1.0)
                } else {
                    EdgeStyle::gradient(source, target).opacity(0.6, 1.0)
                });
                edge
            })
            .collect();

        let zone_list: Vec<&shader_graph::zones::Zone> = zones.zones.values().collect();
        let paddings = zones::paddings(&zone_list);
        let mut ordered = zone_list.clone();
        ordered.sort_by_key(|z| z.depth);
        let bubbles: Vec<Bubble> = ordered
            .iter()
            .map(|z| Bubble {
                kind: z.kind,
                input: z.input_id.clone().into(),
                output: z.output_id.clone().into(),
                members: z.members.iter().map(|m| Id::from(m.clone())).collect(),
                padding: paddings.iter().find(|(id, _)| *id == z.id).map_or(28.0, |p| p.1),
            })
            .collect();
        let snapshot = Rc::new(Snapshot {
            items: doc
                .nodes
                .iter()
                .map(|n| SnapshotItem {
                    id: n.id.clone(),
                    is_frame: n.is(LAYOUT_FRAME),
                    frame: n.frame.clone(),
                })
                .collect(),
            analysis: model.frame_analysis(doc),
            zones: ordered
                .iter()
                .map(|z| ZoneBox {
                    input: z.input_id.clone(),
                    output: z.output_id.clone(),
                    members: z.members.iter().cloned().collect(),
                    padding: paddings.iter().find(|(id, _)| *id == z.id).map_or(28.0, |p| p.1),
                })
                .collect(),
            looks: doc
                .nodes
                .iter()
                .filter(|n| n.is(LAYOUT_FRAME))
                .map(|n| {
                    let look = FrameLook {
                        tint: frame_tint(&n.data),
                        label: model.node_label(&n.id),
                        note: frame_note(&n.data).map(str::to_owned),
                    };
                    (n.id.clone(), look)
                })
                .collect(),
        });
        drop(model);
        self.frame_snapshot = snapshot.clone();

        let layers = Layers {
            back: vec![
                frames::layer(flow_entity.clone(), snapshot, self.frame_drag.clone()),
                zones::layer(flow_entity, bubbles),
            ],
            front: Vec::new(),
            portals: Vec::new(),
        };
        self.flow.update(cx, |flow, cx| {
            flow.set_nodes(nodes, cx);
            flow.set_edges(edges, cx);
            flow.set_layers(layers, cx);
        });
        self.place_frames(cx);
        self.settle_frames(cx);
        let settings = self.model.borrow().settings();
        if settings != self.settings {
            self.settings = settings;
            cx.emit(EditorEvent::Settings(settings));
        }
        cx.notify();
    }

    /// Puts each frame's header at the top-left of its box.
    fn place_frames(&mut self, cx: &mut Context<Self>) {
        if self.frame_snapshot.is_empty() {
            self.placed.clear();
            return;
        }
        let loose = self.frame_drag.borrow().as_ref().map(|d| d.loose.clone()).unwrap_or_default();
        let flow = self.flow.read(cx);
        let (boxes, _) = frames::boxes(&self.frame_snapshot, flow, &loose);
        let mut nodes = flow.nodes().to_vec();
        let mut moved = false;
        for node in &mut nodes {
            if let Some(b) = boxes.get(node.id.as_ref())
                && ((node.position.x - b.x).abs() > 0.01 || (node.position.y - b.y).abs() > 0.01)
            {
                node.position = Xy::new(b.x, b.y);
                moved = true;
            }
        }
        if moved {
            self.flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
        }
        if self.frame_drag.borrow().is_none() {
            self.placed = self.flow_positions(cx);
        }
    }

    /// Frame boxes depend on measured node sizes, which only exist after the
    /// flow has painted: place the headers again once they do.
    fn settle_frames(&mut self, cx: &mut Context<Self>) {
        if self.frame_snapshot.is_empty() {
            self.frame_settle = None;
            return;
        }
        self.frame_settle = Some(cx.spawn(async move |this, cx| {
            for ms in [40, 150, 500] {
                cx.background_executor().timer(Duration::from_millis(ms)).await;
                this.update(cx, |editor, cx| {
                    guard::guard("place frames", || editor.place_frames(cx));
                })
                .ok();
            }
        }));
    }

    fn flow_positions(&self, cx: &App) -> HashMap<String, GraphPoint> {
        self.flow
            .read(cx)
            .nodes()
            .iter()
            .map(|n| (n.id.to_string(), graph_point(n.position)))
            .collect()
    }

    fn start_frame_drag(&mut self, dragged: &[Id], cx: &mut Context<Self>) {
        if self.frame_snapshot.is_empty() {
            return;
        }
        let dragged: Vec<String> = dragged.iter().map(ToString::to_string).collect();
        let positions = self.flow_positions(cx);
        *self.frame_drag.borrow_mut() =
            Some(Drag::start(&self.frame_snapshot.analysis, &positions, &dragged));
    }

    /// Frames drag what they hold along, and show which frame a drop would join.
    fn frame_drag_moved(&mut self, anchor: &Id, cx: &mut Context<Self>) {
        let (delta, inside, loose, moving) = {
            let drag = self.frame_drag.borrow();
            let Some(drag) = drag.as_ref() else { return };
            let flow = self.flow.read(cx);
            let (Some(from), Some(now)) =
                (drag.from.get(anchor.as_ref()), flow.nodes().iter().find(|n| n.id == *anchor))
            else {
                return;
            };
            (
                Xy::new(now.position.x - from.x, now.position.y - from.y),
                drag.inside.clone(),
                drag.loose.clone(),
                drag.moving.clone(),
            )
        };
        if !inside.is_empty() {
            let mut nodes = self.flow.read(cx).nodes().to_vec();
            for node in &mut nodes {
                if let Some(p) = inside.get(node.id.as_ref()) {
                    node.position = Xy::new(p.x + delta.x, p.y + delta.y);
                }
            }
            self.flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
        }
        let target = {
            let flow = self.flow.read(cx);
            let (boxes, analysis) = frames::boxes(&self.frame_snapshot, flow, &loose);
            let rects = frames::node_rects(flow);
            boxes.get(anchor.as_ref()).or_else(|| rects.get(anchor.as_ref())).and_then(|rect| {
                let c = rect.center();
                frames::frame_at(&boxes, &analysis, c, &moving)
            })
        };
        if let Some(drag) = self.frame_drag.borrow_mut().as_mut() {
            drag.drop_frame = target;
        }
        self.place_frames(cx);
    }

    /// Commits a drag: positions, frames dragged along, and the frames the
    /// nodes were dropped in, as one edit.
    fn finish_drag(&mut self, anchor: &Id, dragged: &[Id], cx: &mut Context<Self>) {
        let dragged: Vec<String> = dragged.iter().map(ToString::to_string).collect();
        let now = self.flow_positions(cx);
        let analysis = self.model.borrow().active_frames();
        let drag = self
            .frame_drag
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Drag::start(&analysis, &self.placed, &dragged));
        let delta = match (drag.from.get(anchor.as_ref()), now.get(anchor.as_ref())) {
            (Some(from), Some(now)) => GraphPoint::new(now.x - from.x, now.y - from.y),
            _ => GraphPoint::default(),
        };
        let mut positions: BTreeMap<String, GraphPoint> =
            dragged.iter().filter_map(|id| Some((id.clone(), *now.get(id)?))).collect();
        for (id, p) in &drag.inside {
            positions.insert(id.clone(), GraphPoint::new(p.x + delta.x, p.y + delta.y));
        }
        let targets = {
            let flow = self.flow.read(cx);
            let (boxes, _) = frames::boxes(&self.frame_snapshot, flow, &drag.loose);
            frames::drop_targets(&analysis, &boxes, &frames::node_rects(flow), &dragged, &drag.moving)
        };
        self.model_mut().drop_nodes(&positions, &targets);
        self.sync(cx);
        self.regenerate(cx);
    }

    fn regenerate(&mut self, cx: &mut Context<Self>) {
        let result = self.model.borrow().generate().map_err(|e| e.to_string());
        match result {
            Ok(program) => {
                if program.program.code == self.code {
                    if let Ok(current) = &mut self.generated {
                        current.uniforms = program.uniforms.clone();
                    }
                    cx.emit(EditorEvent::Uniforms(program.uniforms));
                } else {
                    self.code = program.program.code.clone();
                    self.generated = Ok(program.clone());
                    cx.emit(EditorEvent::Shader(Ok(program)));
                }
            }
            Err(message) => {
                self.code.clear();
                self.generated = Err(message.clone());
                cx.emit(EditorEvent::Shader(Err(message)));
            }
        }
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.sync(cx);
        self.regenerate(cx);
    }

    /// Runs an edit of the model, then shows it.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut GraphModel)) {
        f(&mut self.model_mut());
        self.after_edit(cx);
    }

    fn enter_document(&mut self, cx: &mut Context<Self>, before: String) {
        let key = self.model.borrow().active_definition_ref();
        if key == before {
            self.after_edit(cx);
            return;
        }
        self.sync(cx);
        self.regenerate(cx);
        let saved = self.viewports.get(&key).copied();
        self.flow.update(cx, |flow, cx| match saved {
            Some(vp) => flow.set_viewport(vp, None, cx),
            None => flow.fit_view(Default::default(), cx),
        });
    }

    // --- Edits the node cards make ---

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_component(
        &mut self,
        node: &str,
        input: &str,
        ty: WgslType,
        index: usize,
        value: f64,
        edit: Edit,
        cx: &mut Context<Self>,
    ) {
        let stored = {
            let model = self.model.borrow();
            model.node(node).and_then(|n| n.data.values.get(input).copied())
        };
        let next = InputValue::with_component(stored.as_ref(), ty, index, value);
        let first = !self.gesture;
        {
            let mut model = self.model_mut();
            let (node, input) = (node.to_owned(), input.to_owned());
            // A step of a scrub replaces the history step the gesture began with.
            model.edit_gesture(first, |project| {
                if let Some(key) = project.definition_ref_of(&node).map(str::to_owned)
                    && let Some(n) = project.definitions.get_mut(&key).and_then(|d| d.node_mut(&node))
                {
                    n.data.values.insert(input, next);
                }
            });
        }
        self.gesture = edit == Edit::Preview;
        self.sync(cx);
        self.regenerate(cx);
    }

    pub(crate) fn set_enum(&mut self, node: &str, input: &str, value: i64, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_input_value(node, input, Some(InputValue::from(value as f64))));
    }

    pub(crate) fn set_setting(&mut self, node: &str, key: &str, value: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_setting(node, key, value));
    }

    pub(crate) fn set_node_type(&mut self, node: &str, ty: Option<WgslType>, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_node_type(node, ty));
    }

    pub(crate) fn enter_group(&mut self, node: &str, cx: &mut Context<Self>) {
        self.save_viewport(cx);
        let before = self.model.borrow().active_definition_ref();
        self.model_mut().enter_group(node);
        self.enter_document(cx, before);
    }

    fn exit_group(&mut self, cx: &mut Context<Self>) {
        if self.model.borrow().path().is_empty() {
            return;
        }
        self.save_viewport(cx);
        let before = self.model.borrow().active_definition_ref();
        self.model_mut().exit_group();
        self.enter_document(cx, before);
    }

    fn jump_to(&mut self, depth: usize, cx: &mut Context<Self>) {
        self.save_viewport(cx);
        let before = self.model.borrow().active_definition_ref();
        let path: Vec<String> = self.model.borrow().path()[..depth].to_vec();
        self.model_mut().jump_to_path(path);
        self.enter_document(cx, before);
    }

    fn save_viewport(&mut self, cx: &App) {
        let key = self.model.borrow().active_definition_ref();
        self.viewports.insert(key, self.flow.read(cx).viewport());
    }

    pub(crate) fn open_sheet(&mut self, node: &str, cx: &mut Context<Self>) {
        self.sheet = if self.sheet.as_deref() == Some(node) { None } else { Some(node.to_owned()) };
        cx.notify();
    }

    pub(crate) fn open_list(&mut self, at: Point<Pixels>, items: Vec<ListItem>, cx: &mut Context<Self>) {
        self.popup_serial += 1;
        self.popup = Some(Popup { at, items, serial: self.popup_serial });
        cx.notify();
    }

    /// The types a new socket or item can have, picked from the type menu.
    pub(crate) fn open_type_list(&mut self, at: Point<Pixels>, node: String, boundary: bool, cx: &mut Context<Self>) {
        let (kind, extra) = {
            let model = self.model.borrow();
            (model.node(&node).and_then(|n| shader_graph::zones::zone_kind_of(&n.node_type)), !boundary)
        };
        // A simulation keeps textures too; a repeat zone's items are values.
        let types: Vec<WgslType> = if kind == Some(shader_graph::zones::ZoneKind::Repeat) {
            WgslType::ALL.to_vec()
        } else {
            shader_graph::types::SOCKET_TYPES.to_vec()
        };
        let weak = cx.entity().downgrade();
        let inferred = extra.then(|| {
            let (w, node) = (weak.clone(), node.clone());
            ListItem::new(
                "Inferred",
                None,
                type_picker::on_editor(&w, move |e, cx| e.add_inferred_item(&node, cx)),
            )
            .detail("Initial value")
        });
        self.open_type_picker(
            at,
            TypePicker {
                types,
                value: None,
                inferred: None,
                on_pick: Rc::new(move |ty, e, cx| e.add_socket(&node, boundary, ty, cx)),
                on_reset: None,
                extra: inferred.into_iter().collect(),
            },
            cx,
        );
    }

    pub(crate) fn open_type_picker(&mut self, at: Point<Pixels>, picker: TypePicker, cx: &mut Context<Self>) {
        let items = type_picker::items(&cx.entity().downgrade(), at, Rc::new(picker));
        self.open_list(at, items, cx);
    }

    fn add_inferred_item(&mut self, node: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            if let Some(zone) = m.active_zones().zone_of_boundary.get(node).cloned() {
                m.add_zone_item(&zone, HandleType::inferred(), None);
            }
        });
    }

    /// Makes an editable copy of a locked group for the node.
    pub(crate) fn clone_group(&mut self, node: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            m.make_group_unique(node);
        });
    }

    pub(crate) fn toggle_unlinked_sockets(&mut self, node: &str, cx: &mut Context<Self>) {
        let ids = vec![node.to_owned()];
        self.edit(cx, |m| m.toggle_unlinked_sockets(&ids));
    }

    pub(crate) fn set_variable_type(&mut self, node: &str, variable: &str, ty: Option<WgslType>, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_variable_type(node, variable, ty));
    }

    fn add_socket(&mut self, node: &str, boundary: bool, ty: WgslType, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            if boundary {
                m.add_boundary_handle(node, HandleType::Fixed(ty));
            } else if let Some(zone) = m.active_zones().zone_of_boundary.get(node).cloned() {
                m.add_zone_item(&zone, HandleType::Fixed(ty), None);
            }
        });
    }

    // --- Flow events ---

    fn on_flow_event(&mut self, event: FlowEvent, cx: &mut Context<Self>) {
        match event {
            FlowEvent::NodesChange(changes) => {
                let nodes = gpui_flow::apply_node_changes(&changes, self.flow.read(cx).nodes().to_vec());
                self.flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
            }
            FlowEvent::EdgesChange(changes) => {
                let edges = gpui_flow::apply_edge_changes(&changes, self.flow.read(cx).edges().to_vec());
                self.flow.update(cx, |f, cx| f.set_edges(edges, cx));
            }
            FlowEvent::SelectionChange { nodes, .. } => {
                self.model_mut().set_selected_node_ids(nodes.iter().map(|n| n.to_string()).collect());
                cx.notify();
            }
            FlowEvent::GrabCancel { .. } => {
                *self.frame_drag.borrow_mut() = None;
                if let Some((copies, originals)) = self.duplicating.take() {
                    self.edit(cx, |m| {
                        m.delete_nodes(&copies, &[]);
                    });
                    self.select(originals, cx);
                } else {
                    self.sync(cx);
                }
            }
            FlowEvent::NodeDragStart { nodes, .. } => self.start_frame_drag(&nodes, cx),
            FlowEvent::NodeDrag { id, .. } => self.frame_drag_moved(&id, cx),
            FlowEvent::NodeDragStop { id, nodes } => {
                self.duplicating = None;
                self.finish_drag(&id, &nodes, cx);
            }
            FlowEvent::Connect(c) => {
                self.model_mut().try_connect(to_model(&c));
                self.after_edit(cx);
            }
            FlowEvent::ConnectEnd(state) => {
                if state.to_node.is_none() && state.reconnecting.is_none() && !self.over_node(state.to, cx) {
                    let pending = Pending {
                        node: state.from_node.to_string(),
                        handle: state.from_handle.map(|h| h.to_string()).unwrap_or_default(),
                        from_output: state.from_type == gpui_flow::HandleType::Source,
                    };
                    let at = self.flow.read(cx).flow_to_screen(state.to);
                    self.open_add_menu(at, Some(state.to), Some(pending), cx);
                }
            }
            FlowEvent::Move(vp) => {
                let key = self.model.borrow().active_definition_ref();
                self.viewports.insert(key, vp);
            }
            FlowEvent::NodeDoubleClick { id } => {
                let is_group = self.model.borrow().node(&id).is_some_and(|n| n.definition_ref().is_some());
                if is_group {
                    self.enter_group(&id, cx);
                }
            }
            FlowEvent::PaneContextMenu { window, flow } => self.open_context_menu(window, flow, false, cx),
            FlowEvent::NodeContextMenu { window, flow, .. } => self.open_context_menu(window, flow, true, cx),
            FlowEvent::EdgeContextMenu { id, window, .. } => {
                let weak = cx.entity().downgrade();
                let id = id.to_string();
                self.open_list(
                    window,
                    vec![ListItem::new(
                        "Delete link",
                        None,
                        Rc::new(move |_, cx| {
                            let id = id.clone();
                            weak.update(cx, |e, cx| e.edit(cx, |m| {
                                m.delete_edges(&[id]);
                            }))
                            .ok();
                        }),
                    )],
                    cx,
                );
            }
            _ => {}
        }
    }

    fn over_node(&self, flow: Xy, cx: &App) -> bool {
        let flow_ref = self.flow.read(cx);
        flow_ref
            .nodes()
            .iter()
            .any(|n| flow_ref.node_rect(&n.id).is_some_and(|r| r.contains(flow)))
    }

    // --- Add menu ---

    fn open_add_menu(
        &mut self,
        at: Point<Pixels>,
        flow_position: Option<Xy>,
        pending: Option<Pending>,
        cx: &mut Context<Self>,
    ) {
        if self.model.borrow().is_locked() {
            return;
        }
        let position = flow_position.unwrap_or_else(|| self.flow.read(cx).screen_to_flow(at));
        let (entries, placeholder): (Vec<AddMenuEntry>, String) = {
            let model = self.model.borrow();
            let all = add_menu_entries(&model);
            match &pending {
                Some(p) => {
                    let ty = if p.from_output {
                        model.resolve_output_handle_type(&p.node, &p.handle)
                    } else {
                        model.resolve_input_type(&p.node, &p.handle)
                    };
                    let hint = if p.from_output { format!("Nodes taking a {}…", ty.id()) } else { format!("Nodes producing a {}…", ty.id()) };
                    (connectable_entries(&model, &all, p), hint)
                }
                None => (all, "Search nodes…".to_owned()),
            }
        };
        let model = self.model.clone();
        let accepts_pending = pending.clone();
        let accepts: Box<dyn Fn(&AddMenuEntry) -> bool> = Box::new(move |entry| match &accepts_pending {
            Some(p) => catalog::entry_handle_for(&model.borrow(), entry, p).is_some(),
            None => true,
        });
        let menu = cx.new(|cx| AddMenu::new(entries, at, placeholder, accepts, cx));
        cx.subscribe(&menu, move |this, _, event: &AddMenuEvent, cx| match event {
            AddMenuEvent::Picked(entry) => {
                this.add_menu = None;
                this.add_picked((**entry).clone(), position, pending.clone(), cx);
            }
            AddMenuEvent::DeleteGroup(key) => {
                if let Some(reference) = key.strip_prefix("group:") {
                    let reference = reference.to_owned();
                    this.edit(cx, |m| m.delete_definition(&reference));
                    this.add_menu = None;
                }
            }
            AddMenuEvent::Dismissed => {
                this.add_menu = None;
                cx.notify();
            }
        })
        .detach();
        self.add_menu = Some(menu);
        cx.notify();
    }

    fn add_picked(&mut self, entry: AddMenuEntry, position: Xy, pending: Option<Pending>, cx: &mut Context<Self>) {
        let created = {
            let mut model = self.model_mut();
            let at = graph_point(position);
            match &pending {
                Some(p) => add_entry_connected(&mut model, &entry, at, p),
                None => add_entry(&mut model, &entry, at),
            }
        };
        if let Some(id) = created {
            self.model_mut().set_selected_node_ids(vec![id.clone()]);
            self.after_edit(cx);
            self.flow.update(cx, |f, cx| f.set_selection(&[id.into()], &[], cx));
        }
    }

    fn pointer_window(&self, cx: &App) -> Point<Pixels> {
        let flow = self.flow.read(cx);
        let at = flow.pointer_flow().unwrap_or_else(|| {
            let (w, h) = flow.size();
            flow.viewport().to_flow(Xy::new(w / 2.0, h / 2.0))
        });
        flow.flow_to_screen(at)
    }

    /// The context menu: on a node, or on the canvas, with what applies to the selection.
    fn open_context_menu(&mut self, at: Point<Pixels>, flow: Xy, on_node: bool, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let selected = self.selected();
        let (locked, reroutes, all_hidden, all_links_hidden, in_frame, ungroupable, single) = {
            let model = self.model.borrow();
            let doc = model.active_document();
            let nodes: Vec<&shader_graph::graph::GraphNode> =
                selected.iter().filter_map(|id| doc.node(id)).collect();
            let reroutes: Vec<&&shader_graph::graph::GraphNode> =
                nodes.iter().filter(|n| n.is(REROUTE)).collect();
            let frames = model.frame_analysis(doc);
            (
                model.is_locked(),
                reroutes.len(),
                !nodes.is_empty() && nodes.iter().all(|n| n.data.hide_unlinked == Some(true)),
                reroutes.iter().all(|n| n.data.hide_links == Some(true)),
                selected.iter().any(|id| frames.frame_of.contains_key(id)),
                nodes.iter().any(|n| n.definition_ref().is_some()),
                (nodes.len() == 1).then(|| nodes[0].id.clone()),
            )
        };
        let can_paste = self.clipboard.is_some();
        let mut items: Vec<ListItem> = Vec::new();
        let run = |f: Handler| -> OnSelect {
            let weak = weak.clone();
            Rc::new(move |_, cx| {
                let f = f.clone();
                weak.update(cx, |e, cx| f(e, cx)).ok();
            })
        };
        let mut push = |label: &str, detail: Option<&str>, disabled: bool, f: Handler| {
            let mut item = ListItem::new(label.to_owned(), None, run(f)).disabled(disabled);
            if let Some(d) = detail {
                item = item.detail(d.to_owned());
            }
            items.push(item);
        };
        if !on_node {
            push("Add…", Some("⇧A"), locked, Rc::new(move |e, cx| e.open_add_menu(at, Some(flow), None, cx)));
        }
        if let Some(id) = single.clone() {
            push("Settings", Some("N"), false, Rc::new(move |e, cx| e.open_sheet(&id, cx)));
        }
        if !selected.is_empty() {
            push("Cut", Some("⌘X"), locked, Rc::new(|e, cx| e.cut(cx)));
            push("Copy", Some("⌘C"), false, Rc::new(|e, cx| e.copy(cx)));
        }
        push("Paste", Some("⌘V"), locked || !can_paste, Rc::new(move |e, cx| e.paste(Some(flow), cx)));
        if !selected.is_empty() {
            push("Move", Some("G"), locked, Rc::new(|e, cx| e.grab(cx)));
            push("Duplicate", Some("⇧D"), locked, Rc::new(|e, cx| e.duplicate(cx)));
            if reroutes > 0 {
                push(
                    if all_links_hidden { "Show links" } else { "Hide links" },
                    None,
                    locked,
                    Rc::new(|e, cx| e.toggle_hidden_links(cx)),
                );
            }
            push(
                if all_hidden { "Show all sockets" } else { "Hide unlinked sockets" },
                Some("⌃H"),
                locked,
                Rc::new(|e, cx| {
                    let ids = e.selected();
                    e.edit(cx, |m| m.toggle_unlinked_sockets(&ids));
                }),
            );
        }
        let mut separated = Vec::new();
        if !selected.is_empty() {
            separated.push(ListItem::new("Delete", None, run(Rc::new(|e, cx| e.delete_selection(cx)))).detail("⌫").disabled(locked).destructive().separated());
            separated.push(ListItem::new("Dissolve", None, run(Rc::new(|e, cx| e.dissolve_selection(cx)))).detail("⌘⌫").disabled(locked));
            separated.push(ListItem::new("Frame selected", None, run(Rc::new(|e, cx| e.frame_selection(cx)))).detail("⌘J").disabled(locked).separated());
            if in_frame {
                separated.push(ListItem::new("Remove from frame", None, run(Rc::new(|e, cx| e.remove_from_frame(cx)))).detail("⌥P").disabled(locked));
            }
            if selected.len() > 1 {
                separated.push(ListItem::new("Group selected", None, run(Rc::new(|e, cx| e.group_selection(cx)))).detail("⌘G").disabled(locked));
            }
            if ungroupable {
                separated.push(ListItem::new("Ungroup", None, run(Rc::new(|e, cx| e.ungroup_selection(cx)))).detail("⌘⌥G").disabled(locked));
            }
        }
        items.extend(separated);
        if !on_node {
            let w = weak.clone();
            items.push(
                ListItem::new("Fit view", None, Rc::new(move |_, cx| {
                    w.update(cx, |e, cx| e.flow.update(cx, |f, cx| f.fit_view(Default::default(), cx))).ok();
                }))
                .separated(),
            );
            if !self.model.borrow().path().is_empty() {
                let w = weak.clone();
                items.push(ListItem::new("Exit group", None, Rc::new(move |_, cx| {
                    w.update(cx, |e, cx| e.exit_group(cx)).ok();
                })).detail("⇥"));
            }
        }
        self.open_list(at, items, cx);
    }

    // --- Commands ---

    fn selected(&self) -> Vec<String> {
        self.model.borrow().selected_node_ids().to_vec()
    }

    fn select(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.model_mut().set_selected_node_ids(ids.clone());
        let flow_ids: Vec<Id> = ids.into_iter().map(Id::from).collect();
        self.flow.update(cx, |f, cx| f.set_selection(&flow_ids, &[], cx));
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let nodes = self.selected();
        let edges = self.flow.read(cx).selected_edges().into_iter().map(|e| e.to_string()).collect::<Vec<_>>();
        self.edit(cx, |m| {
            m.delete_nodes(&nodes, &edges);
        });
        self.model_mut().set_selected_node_ids(Vec::new());
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        self.copy(cx);
        self.delete_selection(cx);
    }

    fn toggle_hidden_links(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected();
        self.edit(cx, |m| m.toggle_hidden_links(&ids));
        let unnamed = {
            let model = self.model.borrow();
            ids.iter()
                .find(|id| {
                    model.node(id).is_some_and(|n| {
                        n.is(REROUTE) && n.data.hide_links == Some(true) && n.data.label.is_none()
                    })
                })
                .cloned()
        };
        // A reroute whose links get hidden is named by its tags: an unnamed one asks for a name.
        if let Some(id) = unnamed {
            self.sheet = Some(id);
            cx.notify();
        }
    }

    fn dissolve_selection(&mut self, cx: &mut Context<Self>) {
        let nodes = self.selected();
        self.edit(cx, |m| {
            m.dissolve_nodes(&nodes);
        });
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        self.clipboard = self.model.borrow().copy_nodes(&self.selected()).or(self.clipboard.take());
        cx.notify();
    }

    fn paste(&mut self, at: Option<Xy>, cx: &mut Context<Self>) {
        let Some(clipboard) = self.clipboard.clone() else { return };
        let at = at.or_else(|| self.flow.read(cx).pointer_flow());
        let ids = self.model_mut().paste_nodes(&clipboard, at.map(graph_point));
        self.after_edit(cx);
        self.select(ids, cx);
    }

    fn grab(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<Id> = self.selected().into_iter().map(Id::from).collect();
        self.flow.update(cx, |f, cx| f.begin_grab(&ids, cx));
    }

    fn duplicate(&mut self, cx: &mut Context<Self>) {
        let originals = self.selected();
        let ids = self.model_mut().duplicate_nodes(&originals);
        self.after_edit(cx);
        self.select(ids.clone(), cx);
        let flow_ids: Vec<Id> = ids.iter().cloned().map(Id::from).collect();
        if self.flow.update(cx, |f, cx| f.begin_grab(&flow_ids, cx)) {
            self.duplicating = Some((ids, originals));
        }
    }

    fn frame_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected();
        let id = self.model_mut().frame_nodes(&selected);
        self.after_edit(cx);
        if let Some(id) = id {
            self.select(vec![id], cx);
        }
    }

    fn remove_from_frame(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected();
        self.edit(cx, |m| {
            m.remove_from_frame(&selected);
        });
    }

    fn group_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected();
        let id = self.model_mut().group_selection(&selected);
        self.after_edit(cx);
        if let Some(id) = id {
            self.select(vec![id], cx);
        }
    }

    fn ungroup_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected();
        let ids = self.model_mut().ungroup_selection(&selected);
        self.after_edit(cx);
        self.select(ids, cx);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let before = self.model.borrow().active_definition_ref();
        if self.model_mut().undo() {
            self.enter_document(cx, before);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let before = self.model.borrow().active_definition_ref();
        if self.model_mut().redo() {
            self.enter_document(cx, before);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flow.focus_handle(cx).is_focused(window) || self.add_menu.is_some() {
            return;
        }
        let k = &event.keystroke;
        let (key, m) = (k.key.as_str(), k.modifiers);
        let cmd = m.platform || m.control;
        let locked = self.model.borrow().is_locked();
        match key {
            "z" if cmd && m.shift => self.redo(cx),
            "z" if cmd => self.undo(cx),
            "y" if cmd => self.redo(cx),
            "c" if cmd => self.copy(cx),
            "x" if cmd && !locked => self.cut(cx),
            "v" if cmd && !locked => self.paste(None, cx),
            "a" if m.shift && !cmd && !locked => {
                let at = self.pointer_window(cx);
                self.open_add_menu(at, None, None, cx);
            }
            "d" if m.shift && !cmd && !locked => self.duplicate(cx),
            "j" if cmd && !m.alt && !m.shift && !locked => self.frame_selection(cx),
            "p" if m.alt && !cmd && !m.shift && !locked => self.remove_from_frame(cx),
            "g" if !cmd && !m.alt && !m.shift && !locked => self.grab(cx),
            "backspace" | "delete" if !locked => {
                if cmd {
                    self.dissolve_selection(cx);
                } else {
                    self.delete_selection(cx);
                }
            }
            "g" if cmd && m.alt && !locked => self.ungroup_selection(cx),
            "g" if cmd && !locked => self.group_selection(cx),
            "h" if m.control && !m.platform => {
                let ids = self.selected();
                self.edit(cx, |model| model.toggle_unlinked_sockets(&ids));
            }
            "r" if !cmd && !locked => {
                let ids = self.selected();
                self.edit(cx, |model| model.reset_nodes(&ids));
            }
            "n" if !cmd => {
                let target = self.selected().first().cloned().or_else(|| self.model.borrow().path().last().cloned());
                match target {
                    Some(id) => self.open_sheet(&id, cx),
                    None => {
                        self.sheet = None;
                        cx.notify();
                    }
                }
            }
            "enter" | "tab" => {
                let group = self
                    .selected()
                    .into_iter()
                    .find(|id| self.model.borrow().node(id).is_some_and(|n| n.definition_ref().is_some()));
                match group {
                    Some(id) => self.enter_group(&id, cx),
                    None if key == "tab" => self.exit_group(cx),
                    None => {}
                }
            }
            "escape" => {
                if self.sheet.is_some() {
                    self.sheet = None;
                    cx.notify();
                } else {
                    self.exit_group(cx);
                }
            }
            _ => {}
        }
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let model = self.model.borrow();
        let mut crumbs: Vec<(usize, String)> = vec![(0, "Root".to_owned())];
        for (i, (_, label, _)) in model.path_labels(model.path()).into_iter().enumerate() {
            crumbs.push((i + 1, label));
        }
        let last = crumbs.len() - 1;
        let locked = model.is_locked();
        drop(model);
        div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(32.0))
            .px_2()
            .border_b_1()
            .border_color(c.border)
            .bg(c.card)
            .text_size(theme.text.sm)
            .children(crumbs.into_iter().enumerate().flat_map(|(i, (depth, label))| {
                let current = i == last;
                let crumb = div()
                    .id(("crumb", depth))
                    .px_2()
                    .rounded(theme.radius.sm)
                    .text_color(if current { c.foreground } else { c.muted_foreground })
                    .when(!current, |d| d.cursor_pointer().hover(|s| s.bg(c.muted)))
                    .on_click(cx.safe_listener(move |this, _: &ClickEvent, _, cx| this.jump_to(depth, cx)))
                    .child(label)
                    .into_any_element();
                let sep: Vec<AnyElement> = if i > 0 { vec![div().text_color(c.muted_foreground).child("›").into_any_element()] } else { Vec::new() };
                sep.into_iter().chain([crumb])
            }))
            .when(locked, |d| d.child(div().ml_2().text_size(theme.text.xs).text_color(c.muted_foreground).child("locked")))
    }

    fn render_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let weak = cx.entity().downgrade();
        let serial = popup.serial;
        let mut menu = Menu::new(SharedString::from(format!("editor-list-{serial}"))).at(popup.at).on_dismiss(move |_, cx| {
            weak.update(cx, |e, cx| {
                if e.popup.as_ref().is_some_and(|p| p.serial == serial) {
                    e.popup = None;
                    cx.notify();
                }
            })
            .ok();
        });
        for item in &popup.items {
            let on_select = item.on_select.clone();
            let mut entry = MenuItem::new(item.label.clone())
                .disabled(item.disabled)
                .on_select(move |w, cx| on_select(w, cx));
            if let Some(checked) = item.checked {
                entry = entry.checked(checked);
            }
            if let Some(detail) = &item.detail {
                entry = entry.shortcut(detail.clone());
            }
            if item.destructive {
                entry = entry.destructive();
            }
            if item.separated {
                menu = menu.separator();
            }
            menu = menu.item(entry);
        }
        Some(menu.into_any_element())
    }
}

fn to_model(c: &FlowConnection) -> Connection {
    Connection::new(
        c.source.to_string(),
        c.source_handle.as_ref().map(|h| h.to_string()).unwrap_or_default(),
        c.target.to_string(),
        c.target_handle.as_ref().map(|h| h.to_string()).unwrap_or_default(),
    )
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let sheet = self.render_sheet(window, cx);
        div()
            .flex()
            .size_full()
            .bg(c.background)
            .text_color(c.foreground)
            .on_key_down(cx.safe_listener(|this, e, w, cx| this.key_down(e, w, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_breadcrumbs(cx))
                    .child(div().flex_1().relative().child(self.flow.clone())),
            )
            .children(sheet)
            .children(self.add_menu.clone())
            .children(self.render_popup(cx))
    }
}

impl Editor {
    fn render_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        sheet::render(self, window, cx)
    }

    pub(crate) fn edit_model(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut GraphModel)) {
        self.edit(cx, f);
    }

    pub(crate) fn close_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet = None;
        cx.notify();
    }
}


#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, point};

    use super::*;

    struct Scene {
        editor: Entity<Editor>,
        frame: String,
        member: String,
        loose: String,
    }

    fn scene(cx: &mut TestAppContext) -> (Scene, &mut gpui::VisualTestContext) {
        cx.update(theme::init);
        let (editor, cx) = cx.add_window_view(Editor::new);
        let (frame, member, loose) = editor.update(cx, |e, cx| {
            let (member, loose, frame) = {
                let mut model = e.model_mut();
                let member = model.add_node("sin", GraphPoint::new(100.0, 100.0)).expect("adds");
                let loose = model.add_node("sin", GraphPoint::new(2000.0, 2000.0)).expect("adds");
                let frame = model.frame_nodes(std::slice::from_ref(&member)).expect("frames");
                (member, loose, frame)
            };
            e.after_edit(cx);
            (frame, member, loose)
        });
        cx.run_until_parked();
        (Scene { editor, frame, member, loose }, cx)
    }

    /// Moves a node in the flow as the pointer would, then drops it.
    fn drag(scene: &Scene, cx: &mut gpui::VisualTestContext, id: &str, to: Xy) {
        scene.editor.update(cx, |e, cx| {
            let id = Id::from(id.to_owned());
            e.start_frame_drag(std::slice::from_ref(&id), cx);
            let mut nodes = e.flow.read(cx).nodes().to_vec();
            for node in nodes.iter_mut().filter(|n| n.id == id) {
                node.position = to;
            }
            e.flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
            e.frame_drag_moved(&id, cx);
            e.finish_drag(&id, std::slice::from_ref(&id), cx);
        });
    }

    fn node(scene: &Scene, cx: &gpui::VisualTestContext, id: &str) -> shader_graph::graph::GraphNode {
        scene.editor.read_with(cx, |e, _| e.model().node(id).cloned().expect("node"))
    }

    #[gpui::test]
    fn dragging_a_frame_takes_what_it_holds_along(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let header = scene.editor.read_with(cx, |e, cx| e.flow_positions(cx)[&scene.frame]);
        drag(&scene, cx, &scene.frame, Xy::new(header.x + 50.0, header.y + 30.0));
        let member = node(&scene, cx, &scene.member);
        assert_eq!((member.position.x, member.position.y), (150.0, 130.0));
        assert_eq!(member.frame.as_deref(), Some(scene.frame.as_str()));
    }

    #[gpui::test]
    fn dropping_a_node_on_a_frame_joins_it_and_dragging_it_out_leaves(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        drag(&scene, cx, &scene.loose, Xy::new(110.0, 110.0));
        assert_eq!(node(&scene, cx, &scene.loose).frame.as_deref(), Some(scene.frame.as_str()));
        drag(&scene, cx, &scene.loose, Xy::new(3000.0, 3000.0));
        assert_eq!(node(&scene, cx, &scene.loose).frame, None);
    }

    #[gpui::test]
    fn a_drag_is_one_undo_step(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let header = scene.editor.read_with(cx, |e, cx| e.flow_positions(cx)[&scene.frame]);
        drag(&scene, cx, &scene.frame, Xy::new(header.x + 50.0, header.y));
        scene.editor.update(cx, |e, cx| e.undo(cx));
        let member = node(&scene, cx, &scene.member);
        assert_eq!((member.position.x, member.position.y), (100.0, 100.0));
    }

    fn render_frame(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_sheet_opens_on_every_kind_of_node(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let (group, zone_input, reroute) = scene.editor.update(cx, |e, cx| {
            let mut model = e.model_mut();
            let member = model.add_node("sin", GraphPoint::new(300.0, 300.0)).expect("adds");
            let group = model.group_selection(&[member]).expect("groups");
            let reference = model.node(&group).and_then(|n| n.definition_ref().map(str::to_owned)).expect("ref");
            model.add_type_variable(&reference, &[WgslType::F32, WgslType::Vec2f]);
            let zone = model.create_zone(shader_graph::zones::ZoneKind::Repeat, GraphPoint::new(0.0, 500.0)).expect("zone");
            let reroute = model.add_node("reroute", GraphPoint::new(0.0, 900.0)).expect("adds");
            drop(model);
            e.after_edit(cx);
            (group, zone, reroute)
        });
        for id in [scene.frame.clone(), group, zone_input, reroute] {
            scene.editor.update(cx, |e, cx| {
                e.sheet = Some(id);
                cx.notify();
            });
            render_frame(cx);
            assert!(scene.editor.read_with(cx, |e, _| e.sheet.is_some()));
        }
    }

    #[gpui::test]
    fn the_context_menu_lists_what_applies(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let labels = |cx: &mut gpui::VisualTestContext, on_node: bool| {
            scene.editor.update(cx, |e, cx| {
                e.open_context_menu(point(px(10.0), px(10.0)), Xy::new(0.0, 0.0), on_node, cx);
                e.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()).unwrap_or_default()
            })
        };
        scene.editor.update(cx, |e, cx| e.select(Vec::new(), cx));
        let canvas = labels(cx, false);
        assert!(canvas.contains(&"Add…".to_owned()) && canvas.contains(&"Paste".to_owned()));
        assert!(!canvas.contains(&"Cut".to_owned()), "{canvas:?}");
        scene.editor.update(cx, |e, cx| e.select(vec![scene.member.clone()], cx));
        let on_member = labels(cx, true);
        for wanted in ["Cut", "Copy", "Move", "Duplicate", "Delete", "Dissolve", "Frame selected", "Remove from frame"] {
            assert!(on_member.contains(&wanted.to_owned()), "{wanted} in {on_member:?}");
        }
        assert!(!on_member.contains(&"Group selected".to_owned()));
        render_frame(cx);
    }

    #[gpui::test]
    fn cutting_and_pasting_moves_nodes(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        scene.editor.update(cx, |e, cx| {
            e.select(vec![scene.loose.clone()], cx);
            e.cut(cx);
        });
        assert!(scene.editor.read_with(cx, |e, _| e.model().node(&scene.loose).is_none()));
        scene.editor.update(cx, |e, cx| e.paste(Some(Xy::new(50.0, 50.0)), cx));
        let count = scene.editor.read_with(cx, |e, _| e.model().root().nodes.iter().filter(|n| n.is("sin")).count());
        assert_eq!(count, 2);
    }

    #[gpui::test]
    fn type_menus_group_types_by_kind(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        scene.editor.update(cx, |e, cx| {
            e.open_type_list(point(px(10.0), px(10.0)), "none".to_owned(), true, cx);
            let labels: Vec<String> = e.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.to_string()).collect()).unwrap_or_default();
            assert_eq!(labels, ["Float", "Integer", "Unsigned integer", "Boolean", "texture"]);
        });
    }
}
