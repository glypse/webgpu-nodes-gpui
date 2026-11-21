use guard::SafeListener as _;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

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
    Connection, FRAG_OUTPUT, GraphModel, GraphProject, NodeClipboard, Point as GraphPoint,
};
use shader_graph::types::{HandleType, InputValue, WgslType};
use shader_graph::UniformValue;
use theme::{ActiveTheme as _, Menu, MenuItem};

use crate::graph::add_menu::{AddMenu, AddMenuEvent};
use crate::graph::fields::Edit;
use crate::graph::node::{node_width, render_node, view_of};
use crate::graph::sheet;
use crate::graph::socket::socket_color;
use crate::graph::zones::{self, Bubble};

/// What the shell needs from the editor.
#[derive(Clone, Debug)]
pub enum EditorEvent {
    /// The project's program changed in a way that needs compiling again,
    /// or couldn't be generated.
    Shader(Result<Program, String>),
    /// Only values the program reads from uniforms changed.
    Uniforms(Vec<(String, UniformValue)>),
}

/// An entry of a small list popup: an enum's variants, a type to pick.
pub type OnSelect = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct ListItem {
    pub label: SharedString,
    pub checked: Option<bool>,
    pub on_select: OnSelect,
}

impl ListItem {
    pub fn new(
        label: impl Into<SharedString>,
        checked: Option<bool>,
        on_select: OnSelect,
    ) -> Self {
        Self { label: label.into(), checked, on_select }
    }
}

struct Popup {
    at: Point<Pixels>,
    items: Vec<ListItem>,
}

pub struct Editor {
    model: Rc<RefCell<GraphModel>>,
    flow: Entity<Flow>,
    code: String,
    generated: Result<Program, String>,
    popup: Option<Popup>,
    add_menu: Option<Entity<AddMenu>>,
    viewports: HashMap<String, Viewport>,
    gesture: bool,
    /// Duplicates being grabbed with the nodes they came from, which a cancel restores.
    duplicating: Option<(Vec<String>, Vec<String>)>,
    clipboard: Option<NodeClipboard>,
    pub(crate) sheet: Option<String>,
    pub(crate) sheet_fields: sheet::Fields,
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
            add_menu: None,
            viewports: HashMap::new(),
            gesture: false,
            duplicating: None,
            clipboard: None,
            sheet: None,
            sheet_fields: sheet::Fields::default(),
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

    /// Replaces the whole project, or starts over from the demo graph.
    pub fn load_project(&mut self, project: Option<GraphProject>, cx: &mut Context<Self>) {
        self.model_mut().set_project(project.unwrap_or_else(demo_project));
        self.viewports.clear();
        self.popup = None;
        self.add_menu = None;
        self.sheet = None;
        self.code.clear();
        self.after_edit(cx);
        self.flow.update(cx, |flow, cx| flow.fit_view(Default::default(), cx));
    }

    pub fn project(&self) -> GraphProject {
        self.model.borrow().project().clone()
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
        drop(model);

        let layers = Layers {
            back: vec![zones::layer(flow_entity, bubbles)],
            front: Vec::new(),
            portals: Vec::new(),
        };
        self.flow.update(cx, |flow, cx| {
            flow.set_nodes(nodes, cx);
            flow.set_edges(edges, cx);
            flow.set_layers(layers, cx);
        });
        cx.notify();
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
        self.popup = Some(Popup { at, items });
        cx.notify();
    }

    /// A list of the types a new socket or item can have.
    pub(crate) fn open_type_list(&mut self, at: Point<Pixels>, node: String, boundary: bool, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let items = WgslType::ALL
            .iter()
            .map(|ty| {
                let (weak, node) = (weak.clone(), node.clone());
                let ty = *ty;
                ListItem::new(
                    ty.id(),
                    None,
                    Rc::new(move |_, cx| {
                        weak.update(cx, |e, cx| e.add_socket(&node, boundary, ty, cx)).ok();
                    }),
                )
            })
            .collect();
        self.open_list(at, items, cx);
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
                if let Some((copies, originals)) = self.duplicating.take() {
                    self.edit(cx, |m| {
                        m.delete_nodes(&copies, &[]);
                    });
                    self.select(originals, cx);
                } else {
                    self.sync(cx);
                }
            }
            FlowEvent::NodeDragStop { nodes, .. } => {
                self.duplicating = None;
                let moved: std::collections::BTreeMap<String, GraphPoint> = self
                    .flow
                    .read(cx)
                    .nodes()
                    .iter()
                    .filter(|n| nodes.contains(&n.id))
                    .map(|n| (n.id.to_string(), graph_point(n.position)))
                    .collect();
                {
                    let mut model = self.model_mut();
                    for (id, p) in &moved {
                        model.set_node_position(id, *p);
                    }
                }
                self.sync(cx);
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
            FlowEvent::PaneContextMenu { window, flow } => self.open_pane_menu(window, flow, cx),
            FlowEvent::NodeContextMenu { window, flow, .. } => self.open_node_menu(window, flow, cx),
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

    fn open_pane_menu(&mut self, at: Point<Pixels>, flow: Xy, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let locked = self.model.borrow().is_locked();
        let mut items = Vec::new();
        if !locked {
            let w = weak.clone();
            items.push(ListItem::new("Add…", None, Rc::new(move |_, cx| {
                w.update(cx, |e, cx| e.open_add_menu(at, Some(flow), None, cx)).ok();
            })));
            if self.clipboard.is_some() {
                let w = weak.clone();
                items.push(ListItem::new("Paste", None, Rc::new(move |_, cx| {
                    w.update(cx, |e, cx| e.paste(Some(flow), cx)).ok();
                })));
            }
        }
        let w = weak.clone();
        items.push(ListItem::new("Fit view", None, Rc::new(move |_, cx| {
            w.update(cx, |e, cx| e.flow.update(cx, |f, cx| f.fit_view(Default::default(), cx))).ok();
        })));
        if !self.model.borrow().path().is_empty() {
            let w = weak.clone();
            items.push(ListItem::new("Exit group", None, Rc::new(move |_, cx| {
                w.update(cx, |e, cx| e.exit_group(cx)).ok();
            })));
        }
        self.open_list(at, items, cx);
    }

    fn open_node_menu(&mut self, at: Point<Pixels>, _flow: Xy, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let locked = self.model.borrow().is_locked();
        let mut items = Vec::new();
        let mut push = |label: &'static str, f: fn(&mut Editor, &mut Context<Editor>)| {
            let w = weak.clone();
            items.push(ListItem::new(label, None, Rc::new(move |_, cx| {
                w.update(cx, f).ok();
            })));
        };
        push("Settings", |e, cx| {
            if let Some(id) = e.selected().first().cloned() {
                e.open_sheet(&id, cx);
            }
        });
        if !locked {
            push("Duplicate", Editor::duplicate);
            push("Copy", Editor::copy);
            push("Delete", Editor::delete_selection);
            push("Group", Editor::group_selection);
            push("Ungroup", Editor::ungroup_selection);
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
            "x" if cmd && !locked => {
                self.copy(cx);
                self.delete_selection(cx);
            }
            "v" if cmd && !locked => self.paste(None, cx),
            "a" if m.shift && !cmd && !locked => {
                let at = self.pointer_window(cx);
                self.open_add_menu(at, None, None, cx);
            }
            "d" if m.shift && !cmd && !locked => self.duplicate(cx),
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
        let mut menu = Menu::new("editor-list").at(popup.at).on_dismiss(move |_, cx| {
            weak.update(cx, |e, cx| {
                e.popup = None;
                cx.notify();
            })
            .ok();
        });
        for item in &popup.items {
            let on_select = item.on_select.clone();
            let mut entry = MenuItem::new(item.label.clone()).on_select(move |w, cx| on_select(w, cx));
            if let Some(checked) = item.checked {
                entry = entry.checked(checked);
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

