use guard::SafeListener as _;
use gpui::prelude::FluentBuilder as _;
use std::cell::Ref;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use handle::{ModelHandle, ModelMut};
use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, Styled as _, Subscription, Window, div,
    px,
};
use gpui_flow::{
    Connection as FlowConnection, ControlsOptions, Edge, EdgeStyle, FitViewOptions, Flow, FlowEvent, Id,
    Layers,
    Node, Viewport, Xy,
};
use shader_graph::catalog::{
    self, AddMenuEntry, Pending, add_entry, add_entry_connected, add_menu_entries_with,
    connectable_entries,
};
use shader_graph::codegen::Program;
use shader_graph::graph::{
    Connection, FRAG_OUTPUT, GraphModel, GraphNode, GraphProject, LAYOUT_FRAME, NodeClipboard,
    PlaybackChange, Point as GraphPoint, ProjectSettings, REROUTE, frame_note, frame_tint,
};
use shader_graph::layout::{LayoutError, LayoutOptions, NodeGeometry};
use shader_graph::types::{HandleType, InputValue, WgslType};
use shader_graph::UniformValue;
use theme::popup::{Action, Cascade, Row};
use theme::{ActiveTheme as _, popup};

use crate::assets::{self, AssetDrag};
use crate::graph::add_menu::{AddMenu, AddMenuEvent};
use crate::graph::fields::Edit;
use crate::graph::type_picker::{self, TypePicker};
use crate::graph::frames::{self, Drag, FrameLook, SharedDrag, Snapshot, SnapshotItem, ZoneBox};
use crate::graph::node::{node_width, render_node, view_of};
use crate::graph::popovers::{self, ColorPopup, SocketsPopup};
use crate::graph::widgets::{self, Widgets};
use crate::graph::sheet;
use crate::graph::socket::socket_color;
use crate::graph::zones::{self, Bubble};
use crate::layout::GraphView;
use crate::playback::Playback;
use crate::preferences;
use crate::project::AssetFiles;

mod handle;
mod widget_edits;

gpui::actions!(
    editor,
    [
        PruneUnusedGroups,
        ExportSelectedGroup,
        LayoutSelected,
        LayoutDocument,
        LayoutGraph,
        Undo,
        Redo,
        GroupSelected,
        UngroupSelected,
        PreviewSelectedNode,
        EnterNodeGroup,
        ExitNodeGroup,
        ToggleNodeSheet,
    ]
);

/// Keys of the editor's menu items. They apply while the graph has focus, not while typing in a field.
pub fn init(cx: &mut App) {
    const CONTEXT: &str = "GraphEditor && !Input";
    cx.bind_keys([
        KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-z", Redo, Some(CONTEXT)),
        KeyBinding::new("cmd-g", GroupSelected, Some(CONTEXT)),
        KeyBinding::new("alt-cmd-g", UngroupSelected, Some(CONTEXT)),
        KeyBinding::new("enter", EnterNodeGroup, Some(CONTEXT)),
        KeyBinding::new("escape", ExitNodeGroup, Some(CONTEXT)),
        KeyBinding::new("n", ToggleNodeSheet, Some(CONTEXT)),
    ]);
}

/// What another area's editor starts from.
pub struct Share {
    model: ModelHandle,
    viewports: HashMap<String, Viewport>,
    settings: ProjectSettings,
    playback: Option<Entity<Playback>>,
}

/// A breadcrumb of the group path: its depth, its label, and the group's node id when that group is locked.
pub(crate) struct Crumb {
    pub depth: usize,
    pub label: String,
    pub locked_group: Option<String>,
}

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
    /// The project's model was edited, after the edit is shown. Listeners compare what they show.
    Edited,
    /// File > Export selected group was chosen; the shell asks where to save it.
    ExportGroup,
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
    /// Entries of a submenu beside the entry, which `on_select` does not run for.
    pub submenu: Option<Vec<ListItem>>,
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
            submenu: None,
        }
    }

    /// A row that only opens `items` as a submenu.
    pub fn group(label: impl Into<SharedString>, items: Vec<ListItem>) -> Self {
        Self::new(label, None, Rc::new(|_: &mut Window, _: &mut App| {})).submenu(items)
    }

    pub fn submenu(mut self, items: Vec<ListItem>) -> Self {
        self.submenu = Some(items);
        self
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
    /// The submenus open when the list shows, as `Cascade::new` takes them.
    path: Vec<usize>,
    /// Which list this is: a list replaced by another must not dismiss the new one.
    serial: u64,
    /// Built from `items` on the first draw, which is when a window is at hand.
    menu: Option<(Entity<Cascade>, Subscription)>,
}

pub struct Editor {
    model: ModelHandle,
    /// Whether this is the editor the shell compiles for: the first area's.
    primary: bool,
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
    widgets: Widgets,
    /// The playback whose asset files the nodes show, once attached.
    playback: Option<Entity<Playback>>,
    /// The asset files the cards were last drawn with.
    files: AssetFiles,
    /// The widget edit last made, to fold a drag's steps into one undo step.
    widget_gesture: Option<(String, Instant)>,
    code_serial: u64,
    code_task: Option<gpui::Task<()>>,
    pub(crate) sockets: Option<SocketsPopup>,
    pub(crate) color_popup: Option<ColorPopup>,
}

/// Vertical distance between the tags of hidden links reaching one input.
const TAG_STEP: f32 = 12.0;

/// A reroute's side, in flow units.
const REROUTE_SIZE: f32 = 18.0;

/// Whether two sets of asset files hold the same files: the same ids, each the same bytes.
fn same_files(a: &AssetFiles, b: &AssetFiles) -> bool {
    a.len() == b.len() && b.iter().all(|(id, bytes)| a.get(id).is_some_and(|known| Arc::ptr_eq(known, bytes)))
}

/// Steps of a widget edit closer than this belong to one gesture.
const GESTURE_GAP: Duration = Duration::from_millis(400);
/// Typing in a code editor compiles the shader only once it pauses.
const CODE_COMMIT_DELAY: Duration = Duration::from_millis(400);

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
        Self::build(ModelHandle::new(GraphModel::new(demo_project())), true, window, cx)
    }

    /// What another area needs to edit the project this one edits.
    pub fn share(&self) -> Share {
        Share {
            model: self.model.another_view(),
            viewports: self.viewports.clone(),
            settings: self.settings,
            playback: self.playback.clone(),
        }
    }

    /// An editor of another area, entered at the root.
    pub fn another_area(share: Share, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut editor = Self::build(share.model, false, window, cx);
        editor.viewports = share.viewports;
        editor.settings = share.settings;
        if let Some(playback) = &share.playback {
            editor.attach_playback(playback, cx);
        }
        editor.sync_view(cx);
        editor
    }

    fn build(model: ModelHandle, primary: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let validator = model.clone();
        let flow = cx.new(|cx| {
            let mut flow = Flow::new(cx);
            flow.options.fit_view_on_init = true;
            flow.options.delete_key = false;
            flow.options.min_zoom = 0.1;
            flow.options.max_zoom = 3.0;
            flow.options.theme = gpui_flow::FlowTheme {
                background: gpui::hsla(0.0, 0.0, 0.0, 1.0),
                ..gpui_flow::FlowTheme::dark()
            };
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
        window.focus(&flow.focus_handle(cx), cx);
        model.register(cx.entity().downgrade());
        let mut editor = Self {
            model,
            primary,
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
            widgets: Widgets::default(),
            playback: None,
            files: AssetFiles::new(),
            widget_gesture: None,
            code_serial: 0,
            code_task: None,
            sockets: None,
            color_popup: None,
        };
        editor.sync_view(cx);
        if primary {
            editor.regenerate(cx);
        }
        editor
    }

    pub(crate) fn model(&self) -> Ref<'_, GraphModel> {
        self.model.borrow()
    }

    fn model_mut(&self) -> ModelMut<'_> {
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
        let me = cx.entity_id();
        for area in self.model.areas() {
            if area.entity_id() != me {
                let viewports = self.viewports.clone();
                area.update(cx, |other, cx| guard::guard("load project", || other.adopt_project(viewports, cx))).ok();
            }
        }
        let saved = self.viewports.get(&self.model.borrow().active_definition_ref()).copied();
        self.flow.update(cx, |flow, cx| {
            flow.options.fit_view_on_init = saved.is_none();
            match saved {
                Some(vp) => flow.set_viewport(vp, None, cx),
                None => flow.fit_view(Default::default(), cx),
            }
        });
    }

    /// Fits the nodes at exactly `zoom`, for `--zoom`.
    pub fn fit_at_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let options = FitViewOptions { min_zoom: Some(zoom), max_zoom: Some(zoom), ..Default::default() };
        self.flow.update(cx, |flow, cx| flow.fit_view(options, cx));
    }

    /// Puts this area in the group a saved layout left it in, at the camera it had.
    pub fn set_view(&mut self, view: &GraphView, cx: &mut Context<Self>) {
        let path = self.model.borrow().valid_path(&view.path);
        self.model_mut().set_path(path);
        let key = self.model.borrow().active_definition_ref();
        if let Some(v) = view.viewport {
            self.viewports.insert(key.clone(), flow_viewport(v));
        }
        self.sync_view(cx);
        let saved = self.viewports.get(&key).copied();
        self.flow.update(cx, |flow, cx| match saved {
            Some(vp) => flow.set_viewport(vp, None, cx),
            None => flow.fit_view(Default::default(), cx),
        });
    }

    /// Another area loaded a project: back at its root, with its cameras.
    fn adopt_project(&mut self, viewports: HashMap<String, Viewport>, cx: &mut Context<Self>) {
        self.model.set_path_to_root();
        self.viewports = viewports;
        self.popup = None;
        self.add_menu = None;
        self.sheet = None;
        self.sockets = None;
        self.color_popup = None;
        *self.frame_drag.borrow_mut() = None;
        self.sync_view(cx);
        let saved = self.viewports.get(&self.model.borrow().active_definition_ref()).copied();
        self.flow.update(cx, |flow, cx| match saved {
            Some(vp) => flow.set_viewport(vp, None, cx),
            None => flow.fit_view(Default::default(), cx),
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
        let space = self.settings.color_space;
        if self.model_mut().update_settings(settings) {
            // Only the colour space is drawn on the cards, so any other setting skips rebuilding them.
            if settings.color_space != space {
                self.sync(cx);
            } else {
                self.announce_settings(cx);
            }
        }
    }

    fn announce_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self.model.borrow().settings();
        if settings != self.settings {
            self.settings = settings;
            cx.emit(EditorEvent::Settings(settings));
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.model.borrow().has_unsaved_changes()
    }

    pub fn mark_saved(&self) {
        self.model_mut().mark_saved();
    }

    // --- Syncing the model into the flow and the shader ---

    /// Shows the model in this area and in the others.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.sync_view(cx);
        let me = cx.entity_id();
        for area in self.model.areas() {
            if area.entity_id() != me {
                area.update(cx, |other, cx| guard::guard("sync area", || other.sync_view(cx))).ok();
            }
        }
    }

    /// Shows the playback's asset files on the cards: a node whose file is gone warns, and
    /// an Image or Video node shows what it reads.
    pub(crate) fn attach_playback(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        self.playback = Some(playback.clone());
        cx.observe(playback, |this, playback, cx| {
            guard::guard("asset files", || {
                if !same_files(&this.files, playback.read(cx).asset_files()) {
                    this.sync(cx);
                }
            });
        })
        .detach();
        self.sync(cx);
    }

    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let _span = tracing::info_span!("sync view").entered();
        if let Some(playback) = &self.playback {
            self.files = playback.read(cx).asset_files().clone();
        }
        let editor = cx.entity().downgrade();
        let flow_entity = self.flow.downgrade();
        let model = self.model.borrow();
        let theme = cx.theme().clone();
        let doc = model.active_document();
        let selected = model.selected_node_ids();
        let strokes = !model.is_locked();
        let gamut = widgets::gamut_of(model.settings().color_space);
        self.widgets.retain(&doc.nodes.iter().map(|n| n.id.clone()).collect());
        let mut nodes: Vec<Node> = Vec::new();
        for n in &doc.nodes {
            let mut view = view_of(&model, n);
            view.attach_widgets(&mut self.widgets, &model, n, gamut, &self.files, cx);
            let view = Rc::new(view);
            let ed = editor.clone();
            let mut node = Node::new(n.id.clone(), flow_point(n.position))
                .width(node_width(&n.node_type))
                .render(move |props, window, cx| render_node(&view, props, &ed, window, cx));
            node.width = None;
            node.selected = selected.contains(&n.id);
            node.deletable = Some(n.deletable());
            nodes.push(node);
        }

        let zones = model.active_zones();
        let unavailable = model.unavailable_links(&model.active_definition_ref());
        let analysis = model.pass_analysis();
        let path = model.path().to_vec();
        let reveals = |id: &str| selected.iter().any(|s| s == id);
        let hides_links = |id: &str| {
            doc.node(id).is_some_and(|n| n.is(REROUTE) && n.data.hide_links == Some(true) && n.data.label.is_some())
        };
        let edges: Vec<Edge> = doc
            .edges
            .iter()
            .map(|e| {
                let source_ty = model.resolve_output_handle_type(&e.source, &e.source_handle);
                let target_ty = model.resolve_input_type(&e.target, &e.target_handle);
                let (source, target) = (socket_color(source_ty, &theme), socket_color(target_ty, &theme));
                let mut edge = Edge::new(
                    e.id.clone(),
                    e.source.clone(),
                    Some(e.source_handle.clone().into()),
                    e.target.clone(),
                    Some(e.target_handle.clone().into()),
                );
                edge.hidden = unavailable.contains(&e.id);
                let link = analysis.link(&path, e);
                let invalid = zones.invalid_edges.contains(&e.id)
                    || matches!(link, Some(shader_graph::passes::TextureLink::Invalid { .. }));
                let texture = source_ty.is_texture()
                    || matches!(link, Some(shader_graph::passes::TextureLink::Render { .. }));
                if hides_links(&e.source) {
                    let siblings: Vec<&shader_graph::graph::GraphEdge> = doc
                        .edges_into(&e.target)
                        .filter(|o| o.target_handle == e.target_handle && hides_links(&o.source))
                        .collect();
                    let index = siblings.iter().position(|o| o.id == e.id).unwrap_or(0);
                    edge.tag = Some(gpui_flow::EdgeTag {
                        text: model.node_label(&e.source),
                        source_color: source,
                        target_color: if invalid { theme.colors.destructive } else { target },
                        offset: (index as f32 - (siblings.len() as f32 - 1.0) / 2.0) * TAG_STEP,
                        revealed: reveals(&e.source) || reveals(&e.target),
                    });
                    return edge;
                }
                let reroutes = |id: &str| doc.node(id).is_some_and(|n| n.is(REROUTE));
                if !invalid && reroutes(&e.source) && reroutes(&e.target) {
                    edge.arrow = Some(source);
                }
                edge.style = Some(if invalid {
                    EdgeStyle::solid(theme.colors.destructive).width(1.5).dash(6.0, 4.0).opacity(0.8, 1.0)
                } else {
                    EdgeStyle::gradient(source, target).width(if texture { 2.5 } else { 1.0 }).opacity(0.6, 1.0)
                });
                match link {
                    Some(shader_graph::passes::TextureLink::Sample) if !invalid => {
                        edge.label = Some(conversion_label("sample", &theme));
                    }
                    Some(shader_graph::passes::TextureLink::Render { .. }) if !invalid => {
                        edge.label = Some(conversion_label("pass", &theme));
                    }
                    _ => {}
                }
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
            // A locked group can still be selected and viewed; nothing in it moves or links.
            flow.options.strokes = !locked;
            flow.options.nodes_draggable = !locked;
            flow.options.nodes_connectable = !locked;
            flow.options.nodes_keyboard_move = !locked;
            flow.set_nodes(nodes, cx);
            flow.set_edges(edges, cx);
            flow.set_layers(layers, cx);
        });
        self.place_frames(cx);
        self.settle_frames(cx);
        self.announce_settings(cx);
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
        if !self.primary {
            let first = self.model.areas().into_iter().next();
            if let Some(first) = first {
                first.update(cx, |first, cx| first.regenerate(cx)).ok();
            }
            return;
        }
        let result = {
            let _span = tracing::info_span!("generate program").entered();
            self.model.borrow().generate_hoisted().map_err(|e| e.to_string())
        };
        match result {
            Ok(program) => {
                if program.program.code == self.code {
                    if let Ok(current) = &mut self.generated {
                        current.uniforms = program.uniforms.clone();
                    }
                    cx.emit(EditorEvent::Uniforms(program.uniforms));
                } else {
                    preferences::log_generated_code(cx, &program.program.code);
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
        cx.notify();
        cx.emit(EditorEvent::Edited);
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
        // The path doesn't change what the project generates, so the program stays as it is.
        self.sync(cx);
        let saved = self.viewports.get(&key).copied();
        self.flow.update(cx, |flow, cx| match saved {
            Some(vp) => flow.set_viewport(vp, None, cx),
            None => flow.fit_view(Default::default(), cx),
        });
    }

    // --- Edits the node cards make ---

    pub(crate) fn set_node_asset(&mut self, node: &str, asset: Option<&str>, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_node_asset(node, asset));
    }

    /// The picker's Import entry: asks for a file and has the Image or Video node read it.
    pub(crate) fn import_for_node(&mut self, node: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(playback) = self.playback.clone() else { return };
        assets::pick_and_import_for_node(&cx.entity(), &playback, node, window, cx);
    }

    pub(crate) fn set_video_playback(&mut self, node: &str, change: PlaybackChange, edit: Edit, cx: &mut Context<Self>) {
        let first = !self.gesture;
        self.model_mut().set_video_playback_step(node, change, first);
        self.gesture = edit == Edit::Preview;
        self.sync(cx);
        self.regenerate(cx);
    }

    /// Assets dragged from the Assets area are dropped at `at` (window coordinates): onto
    /// the Image or Video node under the pointer, or as nodes of their own where they land.
    pub(crate) fn drop_assets(&mut self, asset_ids: &[String], at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.model.borrow().is_locked() {
            return;
        }
        let (flow_at, onto) = {
            let flow = self.flow.read(cx);
            let at = flow.screen_to_flow(at);
            let model = self.model.borrow();
            let onto = model
                .active_document()
                .nodes
                .iter()
                .find(|n| {
                    flow.node_rect(&Id::from(n.id.clone())).is_some_and(|r| {
                        at.x >= r.x && at.x <= r.x + r.width && at.y >= r.y && at.y <= r.y + r.height
                    })
                })
                .map(|n| n.id.clone());
            (at, onto)
        };
        let mut changed = Vec::new();
        self.edit(cx, |m| changed = m.drop_assets(asset_ids, GraphPoint::new(flow_at.x, flow_at.y), onto.as_deref()));
        if !changed.is_empty() {
            self.select(changed, cx);
        }
    }

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

    pub(crate) fn jump_to(&mut self, depth: usize, cx: &mut Context<Self>) {
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
        self.open_list_at(at, items, Vec::new(), cx);
    }

    /// Opens a list with the submenus along `path` (see `Cascade::new`) already showing.
    pub(crate) fn open_list_at(&mut self, at: Point<Pixels>, items: Vec<ListItem>, path: Vec<usize>, cx: &mut Context<Self>) {
        self.popup_serial += 1;
        self.popup = Some(Popup { at, items, path, serial: self.popup_serial, menu: None });
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
        let items = type_picker::items(&cx.entity().downgrade(), Rc::new(picker));
        self.open_list(at, items, cx);
    }

    fn add_inferred_item(&mut self, node: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            if let Some(zone) = m.active_zones().zone_of_boundary.get(node).cloned() {
                m.add_zone_item(&zone, HandleType::inferred(), None);
            }
        });
    }

    /// Edit › Layout selected nodes.
    pub(crate) fn layout_selected(&mut self, cx: &mut Context<Self>) {
        self.layout(cx, false, |m, options| m.layout_selection(options));
    }

    /// Edit › Layout document.
    pub(crate) fn layout_document(&mut self, cx: &mut Context<Self>) {
        if self.model.borrow().is_locked() {
            return;
        }
        self.layout(cx, true, |m, options| {
            let key = m.active_definition_ref();
            m.layout_document(&key, None, options)
        });
    }

    /// Edit › Layout graph.
    pub(crate) fn layout_graph(&mut self, cx: &mut Context<Self>) {
        self.layout(cx, true, |m, options| m.layout_project(options));
    }

    /// Runs a layout as one undo step, using what the flow drew of the active
    /// document's nodes. Other documents are estimated.
    fn layout(
        &mut self,
        cx: &mut Context<Self>,
        fit: bool,
        run: impl FnOnce(&mut GraphModel, &LayoutOptions<'_>) -> Result<bool, LayoutError>,
    ) {
        let active = self.model.borrow().active_definition_ref();
        let drawn = self.drawn_geometry(cx);
        let measure = |doc: &str, node: &GraphNode| {
            if doc == active { drawn.get(&node.id).cloned() } else { None }
        };
        let options = LayoutOptions { measure: Some(&measure), ..Default::default() };
        let result = self.model_mut().batched(|m| run(m, &options));
        let moved = match result {
            Ok(moved) => moved,
            Err(error) => {
                log::error!("layout failed: {error}");
                false
            }
        };
        self.after_edit(cx);
        if fit && moved {
            self.flow.update(cx, |f, cx| f.fit_view(Default::default(), cx));
        }
    }

    /// Each node of the flow's active document as drawn: its size and the
    /// vertical offset of each handle's centre from its top.
    fn drawn_geometry(&self, cx: &App) -> HashMap<String, NodeGeometry> {
        let flow = self.flow.read(cx);
        flow.nodes()
            .iter()
            .filter_map(|n| {
                let rect = flow.node_rect(&n.id)?;
                let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
                for (handle, kind, centre) in flow.handle_centres(&n.id) {
                    let entry = (handle.map(|h| h.to_string()).unwrap_or_default(), centre.y);
                    match kind {
                        gpui_flow::HandleType::Target => inputs.push(entry),
                        gpui_flow::HandleType::Source => outputs.push(entry),
                    }
                }
                Some((n.id.to_string(), NodeGeometry { width: rect.width, height: rect.height, inputs, outputs }))
            })
            .collect()
    }

    /// Makes an editable copy of a locked group for the node.
    pub(crate) fn clone_group(&mut self, node: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            m.deep_clone_group(node);
        });
    }

    /// Asks, listing the groups, before removing the definitions nothing in the
    /// project uses. Bound to `PruneUnusedGroups`.
    pub(crate) fn prune_unused_groups(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let groups = self.model().prunable_groups();
        if groups.is_empty() {
            return;
        }
        let listing: Vec<String> = groups
            .iter()
            .map(|g| {
                let nodes = if g.node_count == 1 { "node" } else { "nodes" };
                format!("{} ({} {nodes})", g.label, g.node_count)
            })
            .collect();
        let detail = format!(
            "These groups aren't used by any node in the project and will be permanently deleted:\n\n{}",
            listing.join("\n")
        );
        let remove = format!("Remove {}", groups.len());
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Remove unused group references?",
            Some(&detail),
            &[remove.as_str(), "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    guard::guard("prune unused groups", || {
                        this.edit(cx, |m| {
                            m.prune_unused_definitions();
                        });
                    });
                })
                .ok();
            }
        })
        .detach();
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
            FlowEvent::Stroke { kind, crossings } => {
                let ids: Vec<String> = crossings.iter().map(|(id, _)| id.to_string()).collect();
                match kind {
                    gpui_flow::StrokeKind::Cut => self.edit(cx, |m| {
                        m.delete_edges(&ids);
                    }),
                    gpui_flow::StrokeKind::Reroute => {
                        // Reroutes are placed by their top left corner.
                        let cuts: Vec<(String, GraphPoint)> = crossings
                            .iter()
                            .map(|(id, at)| (id.to_string(), GraphPoint::new(at.x - REROUTE_SIZE / 2.0, at.y - REROUTE_SIZE / 2.0)))
                            .collect();
                        self.edit(cx, |m| {
                            m.reroute_edges(&cuts);
                        });
                    }
                }
            }
            FlowEvent::NodeDoubleClick { id } => {
                let is_group = self.model.borrow().node(&id).is_some_and(|n| n.definition_ref().is_some());
                if is_group {
                    self.enter_group(&id, cx);
                }
            }
            FlowEvent::NodeClick { id, modifiers } => {
                if modifiers.shift && (modifiers.platform || modifiers.control) {
                    self.preview_node(id.as_ref(), cx);
                }
            }
            FlowEvent::PaneContextMenu { window, flow } => self.open_context_menu(window, flow, None, cx),
            FlowEvent::NodeContextMenu { id, window, flow } => {
                self.open_context_menu(window, flow, Some(id.to_string()), cx)
            }
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
            let all = add_menu_entries_with(&model, preferences::developer_tools_on(cx));
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
    fn open_context_menu(&mut self, at: Point<Pixels>, flow: Xy, on_node: Option<String>, cx: &mut Context<Self>) {
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
        if on_node.is_none() {
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
        if on_node.is_none() {
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
        if let Some(node) = on_node {
            let preview = ListItem::new("Preview", None, run(Rc::new(move |e, cx| e.preview_node(&node, cx))))
                .detail("⇧⌘Click")
                .disabled(locked);
            items.insert(0, preview);
            if let Some(next) = items.get_mut(1) {
                next.separated = true;
            }
        }
        self.open_list(at, items, cx);
    }

    // --- Commands ---

    /// The definition of the first selected group node, if any.
    pub(crate) fn selected_group_ref(&self) -> Option<String> {
        let model = self.model.borrow();
        self.selected().iter().find_map(|id| model.node(id)?.definition_ref().map(str::to_owned))
    }

    fn selected(&self) -> Vec<String> {
        self.model.borrow().selected_node_ids().to_vec()
    }

    fn select(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.model_mut().set_selected_node_ids(ids.clone());
        let flow_ids: Vec<Id> = ids.into_iter().map(Id::from).collect();
        self.flow.update(cx, |f, cx| f.set_selection(&flow_ids, &[], cx));
    }

    /// Previews a node and selects it, as the original does. Previewing it again
    /// moves the preview to its next output.
    pub(crate) fn preview_node(&mut self, node: &str, cx: &mut Context<Self>) {
        let previewed = self.model_mut().preview_node(node);
        if previewed {
            self.after_edit(cx);
            self.select(vec![node.to_owned()], cx);
        }
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

    fn preview_selected(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected();
        if let [node] = selected.as_slice() {
            self.preview_node(node, cx);
        }
    }

    /// Enters the first selected group, returning whether there was one.
    fn enter_selected_group(&mut self, cx: &mut Context<Self>) -> bool {
        let group = self
            .selected()
            .into_iter()
            .find(|id| self.model.borrow().node(id).is_some_and(|n| n.definition_ref().is_some()));
        match group {
            Some(id) => {
                self.enter_group(&id, cx);
                true
            }
            None => false,
        }
    }

    /// Escape closes the node sheet first, then leaves the group.
    fn close_sheet_or_exit_group(&mut self, cx: &mut Context<Self>) {
        if self.sheet.is_some() {
            self.sheet = None;
            cx.notify();
        } else {
            self.exit_group(cx);
        }
    }

    /// Opens the sheet of the selected node, or of the group entered, or closes the sheet when neither exists.
    fn toggle_node_sheet(&mut self, cx: &mut Context<Self>) {
        let target = self.selected().first().cloned().or_else(|| self.model.borrow().path().last().cloned());
        match target {
            Some(id) => self.open_sheet(&id, cx),
            None => {
                self.sheet = None;
                cx.notify();
            }
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let before = self.model.borrow().active_definition_ref();
        if self.model_mut().undo() {
            self.enter_document(cx, before);
            cx.notify();
            cx.emit(EditorEvent::Edited);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let before = self.model.borrow().active_definition_ref();
        if self.model_mut().redo() {
            self.enter_document(cx, before);
            cx.notify();
            cx.emit(EditorEvent::Edited);
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
            "h" if m.control && !m.platform => {
                let ids = self.selected();
                self.edit(cx, |model| model.toggle_unlinked_sockets(&ids));
            }
            "r" if !cmd && !locked => {
                let ids = self.selected();
                self.edit(cx, |model| model.reset_nodes(&ids));
            }
            "tab" => {
                let entered = self.enter_selected_group(cx);
                if !entered {
                    self.exit_group(cx);
                }
            }
            _ => {}
        }
    }

    /// The group path as crumbs, the root first, each with its depth, and whether the group is locked.
    pub(crate) fn breadcrumbs(&self) -> (Vec<Crumb>, bool) {
        let model = self.model.borrow();
        let mut crumbs = vec![Crumb { depth: 0, label: "Root".to_owned(), locked_group: None }];
        for (i, (id, label, locked)) in model.path_labels(model.path()).into_iter().enumerate() {
            crumbs.push(Crumb { depth: i + 1, label, locked_group: locked.then_some(id) });
        }
        (crumbs, model.is_locked())
    }

    fn render_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.as_mut()?;
        if popup.menu.is_none() {
            let serial = popup.serial;
            let rows = popup_rows(std::mem::take(&mut popup.items));
            let path = std::mem::take(&mut popup.path);
            let cascade = cx.new(|cx| Cascade::new(rows, px(200.0), path, cx));
            let focus = cascade.read(cx).focus_handle(cx);
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
            let dismissed = cx.subscribe_in(&cascade, window, move |e: &mut Editor, _, _: &DismissEvent, _, cx| {
                if e.popup.as_ref().is_some_and(|p| p.serial == serial) {
                    e.popup = None;
                    cx.notify();
                }
            });
            if let Some(popup) = self.popup.as_mut() {
                popup.menu = Some((cascade, dismissed));
            }
        }
        let popup = self.popup.as_ref()?;
        let (cascade, _) = popup.menu.as_ref()?;
        let weak = cascade.downgrade();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            weak.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
        };
        Some(popup::layer(window, cx, popup.at, dismiss, cascade.clone()).into_any_element())
    }
}

/// The rows of a list popup: a list entry with a submenu becomes a row that opens it.
fn popup_rows(items: Vec<ListItem>) -> Vec<Row> {
    items
        .into_iter()
        .map(|item| {
            let action = match item.submenu {
                Some(items) => Action::Submenu(popup_rows(items)),
                None => Action::Pick(item.on_select),
            };
            Row {
                label: item.label,
                detail: item.detail,
                checked: item.checked,
                disabled: item.disabled,
                destructive: item.destructive,
                separated: item.separated,
                action,
                trailing: None,
            }
        })
        .collect()
}

/// The graph canvas is black in dark mode and white in light mode.
fn canvas_theme(appearance: theme::Appearance) -> gpui_flow::FlowTheme {
    match appearance {
        theme::Appearance::Dark => gpui_flow::FlowTheme {
            background: gpui::hsla(0.0, 0.0, 0.0, 1.0),
            ..gpui_flow::FlowTheme::dark()
        },
        theme::Appearance::Light => gpui_flow::FlowTheme::light(),
    }
}

fn conversion_label(text: &'static str, theme: &theme::Theme) -> gpui_flow::EdgeLabel {
    gpui_flow::EdgeLabel {
        text: text.into(),
        color: theme.colors.muted_foreground,
        background: theme.colors.card,
        border: Some(socket_color(WgslType::Texture, theme)),
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

/// Which of the editor's menu actions apply now. gpui greys out an action that the focused element
/// doesn't handle, so each handler is attached only while its action applies.
#[derive(Clone, Copy)]
struct Enabled {
    undo: bool,
    redo: bool,
    group: bool,
    ungroup: bool,
    preview: bool,
    prune: bool,
    layout_selected: bool,
    layout_document: bool,
    enter: bool,
    exit: bool,
    sheet: bool,
    export_group: bool,
}

impl Editor {
    fn enabled(&self) -> Enabled {
        let model = self.model.borrow();
        let locked = model.is_locked();
        let selected = model.selected_node_ids();
        let has_selection = !selected.is_empty();
        let selected_group = selected.iter().any(|id| model.node(id).is_some_and(|n| n.definition_ref().is_some()));
        let can_exit = !model.path().is_empty();
        let idle = self.add_menu.is_none();
        Enabled {
            undo: model.can_undo() && idle,
            redo: model.can_redo() && idle,
            group: has_selection && !locked && idle,
            ungroup: selected_group && !locked && idle,
            preview: selected.len() == 1 && !locked,
            prune: !model.prunable_groups().is_empty(),
            layout_selected: has_selection && !locked,
            layout_document: !locked,
            enter: selected_group && idle,
            exit: (can_exit || self.sheet.is_some()) && idle,
            sheet: (has_selection || can_exit || self.sheet.is_some()) && idle,
            export_group: selected_group,
        }
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("graph editor", || {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let sheet = self.render_sheet(window, cx);
        let on = self.enabled();
        div()
            .flex()
            .size_full()
            .bg(c.background)
            .text_color(c.foreground)
            .on_key_down(cx.safe_listener(|this, e, w, cx| this.key_down(e, w, cx)))
            .key_context("GraphEditor")
            .when(on.undo, |d| d.on_action(cx.safe_listener(|this, _: &Undo, _, cx| this.undo(cx))))
            .when(on.redo, |d| d.on_action(cx.safe_listener(|this, _: &Redo, _, cx| this.redo(cx))))
            .when(on.group, |d| {
                d.on_action(cx.safe_listener(|this, _: &GroupSelected, _, cx| this.group_selection(cx)))
            })
            .when(on.ungroup, |d| {
                d.on_action(cx.safe_listener(|this, _: &UngroupSelected, _, cx| this.ungroup_selection(cx)))
            })
            .when(on.preview, |d| {
                d.on_action(cx.safe_listener(|this, _: &PreviewSelectedNode, _, cx| this.preview_selected(cx)))
            })
            .when(on.prune, |d| {
                d.on_action(cx.safe_listener(|this, _: &PruneUnusedGroups, window, cx| {
                    this.prune_unused_groups(window, cx);
                }))
            })
            .when(on.layout_selected, |d| {
                d.on_action(cx.safe_listener(|this, _: &LayoutSelected, _, cx| this.layout_selected(cx)))
            })
            .when(on.layout_document, |d| {
                d.on_action(cx.safe_listener(|this, _: &LayoutDocument, _, cx| this.layout_document(cx)))
            })
            .on_action(cx.safe_listener(|this, _: &LayoutGraph, _, cx| this.layout_graph(cx)))
            .when(on.export_group, |d| {
                d.on_action(cx.safe_listener(|_, _: &ExportSelectedGroup, _, cx| cx.emit(EditorEvent::ExportGroup)))
            })
            .when(on.enter, |d| {
                d.on_action(cx.safe_listener(|this, _: &EnterNodeGroup, _, cx| {
                    this.enter_selected_group(cx);
                }))
            })
            .when(on.exit, |d| {
                d.on_action(cx.safe_listener(|this, _: &ExitNodeGroup, _, cx| this.close_sheet_or_exit_group(cx)))
            })
            .when(on.sheet, |d| {
                d.on_action(cx.safe_listener(|this, _: &ToggleNodeSheet, _, cx| this.toggle_node_sheet(cx)))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .on_drop(cx.safe_listener(|this, dragged: &AssetDrag, window, cx| {
                                this.drop_assets(std::slice::from_ref(&dragged.id), window.mouse_position(), cx);
                            }))
                            .on_drop(cx.safe_listener(|this, paths: &ExternalPaths, window, cx| {
                                if let Some(playback) = this.playback.clone() {
                                    let at = window.mouse_position();
                                    assets::drop_files_on_graph(this, &playback, &paths.0, at, window, cx);
                                }
                            }))
                            .child(self.flow.clone()),
                    ),
            )
            .children(sheet)
            .children(self.add_menu.clone())
            .children(self.render_popup(window, cx))
            .children(popovers::render_color(self, cx))
            .children(popovers::render_sockets(self, window, cx))
    })
    }
}

impl Editor {
    fn render_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        sheet::render(self, window, cx)
    }

    pub(crate) fn edit_model(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut GraphModel)) {
        self.edit(cx, f);
    }

    #[cfg(test)]
    fn sheet_fields_expand(&mut self, ids: Vec<String>) {
        self.sheet_fields.expanded.extend(ids);
    }

    pub(crate) fn close_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet = None;
        cx.notify();
    }
}


#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, point};
    use shader_graph::nodes::IMAGE;

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

    /// Layout moves nodes, and one undo puts every node back.
    #[gpui::test]
    fn a_layout_is_one_undo_step(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let positions = |cx: &mut gpui::VisualTestContext| {
            scene.editor.read_with(cx, |e, _| {
                e.model().active_document().nodes.iter().map(|n| (n.id.clone(), n.position)).collect::<Vec<_>>()
            })
        };
        let before = positions(cx);
        scene.editor.update(cx, |e, cx| e.layout_graph(cx));
        render_frame(cx);
        assert_ne!(positions(cx), before, "the layout moves nodes");
        scene.editor.update(cx, |e, cx| e.undo(cx));
        render_frame(cx);
        assert_eq!(positions(cx), before);
    }

    /// Card heights grow with the zoom and stay within a device pixel or two of
    /// a straight line through the zoom-1 height, so zooming never makes a card
    /// grow then snap back. Run at a 2x display, as on a Retina screen.
    #[gpui::test]
    fn node_cards_scale_linearly_with_zoom(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        cx.simulate_scale_factor_change(2.0);
        let selector: &'static str = Box::leak(format!("node-card {}", scene.member).into_boxed_str());
        let mut heights = Vec::new();
        for step in 10..=40 {
            let zoom = step as f32 / 20.0;
            scene.editor.update(cx, |e, cx| {
                e.flow.update(cx, |f, cx| f.set_viewport(Viewport { x: 0.0, y: 0.0, zoom }, None, cx));
            });
            render_frame(cx);
            let bounds = cx.debug_bounds(selector).expect("the card is drawn");
            heights.push((zoom, f32::from(bounds.size.height)));
        }
        let unit = heights.iter().find(|(z, _)| *z == 1.0).expect("zoom 1 is sampled").1;
        for pair in heights.windows(2) {
            assert!(pair[1].1 >= pair[0].1, "card shrank from zoom {} to {}", pair[0].0, pair[1].0);
        }
        for (zoom, height) in &heights {
            assert!((height - zoom * unit).abs() <= 2.0, "zoom {zoom}: card is {height}, expected about {}", zoom * unit);
        }
    }

    /// The Remap node of the voronoi scene is a tall card of text rows and
    /// number fields. Stepping the zoom finely, its height must follow a
    /// straight line to within a device pixel: snapping each length and line
    /// height before layout makes it plateau and jump by several.
    #[gpui::test]
    fn a_tall_card_grows_smoothly_through_a_fine_zoom_sweep(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/voronoi.wgnodes");
        let loaded = crate::project::read(&path).expect("the scene loads");
        let (editor, cx) = cx.add_window_view(Editor::new);
        editor.update(cx, |e, cx| e.load_project(Some(loaded.project), None, cx));
        let remap = editor.read_with(cx, |e, _| {
            e.model()
                .active_document()
                .nodes
                .iter()
                .find(|n| n.definition_ref().is_some_and(|r| r.starts_with("std-remap")))
                .map(|n| n.id.to_string())
                .expect("the scene has a Remap node")
        });
        let selector: &'static str = Box::leak(format!("node-card {remap}").into_boxed_str());
        for scale in [1.0, 2.0] {
            cx.simulate_scale_factor_change(scale);
            let heights: Vec<(f32, f32)> = (0..=500)
                .map(|step| {
                    let zoom = 1.0 + step as f32 * 0.0004;
                    editor.update(cx, |e, cx| {
                        e.flow.update(cx, |f, cx| f.set_viewport(Viewport { x: 0.0, y: 0.0, zoom }, None, cx));
                    });
                    render_frame(cx);
                    let bounds = cx.debug_bounds(selector).expect("the card is drawn");
                    (zoom, f32::from(bounds.size.height))
                })
                .collect();
            let n = heights.len() as f32;
            let mean_zoom = heights.iter().map(|h| h.0).sum::<f32>() / n;
            let mean_height = heights.iter().map(|h| h.1).sum::<f32>() / n;
            let slope = heights.iter().map(|h| (h.0 - mean_zoom) * (h.1 - mean_height)).sum::<f32>()
                / heights.iter().map(|h| (h.0 - mean_zoom).powi(2)).sum::<f32>();
            for (zoom, height) in &heights {
                let line = mean_height + slope * (zoom - mean_zoom);
                let off = (height - line).abs() * scale;
                assert!(off <= 1.05, "scale {scale}, zoom {zoom}: card is {off} device pixels off a straight line");
            }
        }
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
    fn group_input_details_open_for_every_kind_of_input(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let group = scene.editor.update(cx, |e, cx| {
            let mut model = e.model_mut();
            let member = model.add_node("sin", GraphPoint::new(300.0, 300.0)).expect("adds");
            let group = model.group_selection(&[member]).expect("groups");
            let reference = model.node(&group).and_then(|n| n.definition_ref().map(str::to_owned)).expect("ref");
            let enum_variants = vec![shader_graph::types::EnumVariant::new(0, "A"), shader_graph::types::EnumVariant::new(1, "B")];
            model.add_handle(&reference, shader_graph::graph::GroupSide::Input, HandleType::Fixed(WgslType::U32), Some(enum_variants), None);
            model.add_handle(&reference, shader_graph::graph::GroupSide::Input, HandleType::Fixed(WgslType::Bool), Some(shader_graph::types::toggle_variants()), None);
            model.add_handle(&reference, shader_graph::graph::GroupSide::Input, HandleType::Fixed(WgslType::Vec3f), None, None);
            model.add_handle(&reference, shader_graph::graph::GroupSide::Input, HandleType::Fixed(WgslType::I32), None, None);
            let ids: Vec<String> = model.project().document(&reference).map(|d| d.inputs.iter().map(|h| h.id.clone()).collect()).unwrap_or_default();
            drop(model);
            e.sheet_fields_expand(ids);
            e.after_edit(cx);
            e.sheet = Some(group.clone());
            cx.notify();
            group
        });
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| e.sheet.clone()), Some(group));
    }

    #[gpui::test]
    fn areas_share_the_project_but_not_where_they_are(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let second = cx.update(|window, cx| {
            let share = scene.editor.read(cx).share();
            cx.new(|cx| Editor::another_area(share, window, cx))
        });
        let group = scene.editor.update(cx, |e, cx| {
            let id = e.model_mut().group_selection(std::slice::from_ref(&scene.member)).expect("groups");
            e.after_edit(cx);
            e.enter_group(&id, cx);
            id
        });
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| e.model().path().to_vec()), vec![group.clone()]);
        assert!(second.read_with(cx, |e, _| e.model().path().is_empty()));
        // The second area still sees the root, with the group node in it.
        assert!(second.read_with(cx, |e, _| e.model().active_document().node(&group).is_some()));

        second.update(cx, |e, cx| e.select(vec![group.clone()], cx));
        assert_eq!(second.read_with(cx, |e, _| e.selected()), vec![group.clone()]);
        assert!(scene.editor.read_with(cx, |e, _| e.selected().is_empty()));

        // An edit in one shows in the other.
        second.update(cx, |e, cx| e.delete_selection(cx));
        assert!(scene.editor.read_with(cx, |e, _| e.model().project().root().node(&group).is_none()));
        assert!(scene.editor.read_with(cx, |e, _| e.model().path().is_empty()));
    }

    #[gpui::test]
    fn the_context_menu_lists_what_applies(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let labels = |cx: &mut gpui::VisualTestContext, on_node: Option<String>| {
            scene.editor.update(cx, |e, cx| {
                e.open_context_menu(point(px(10.0), px(10.0)), Xy::new(0.0, 0.0), on_node, cx);
                e.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()).unwrap_or_default()
            })
        };
        scene.editor.update(cx, |e, cx| e.select(Vec::new(), cx));
        let canvas = labels(cx, None);
        assert!(canvas.contains(&"Add…".to_owned()) && canvas.contains(&"Paste".to_owned()));
        assert!(!canvas.contains(&"Cut".to_owned()), "{canvas:?}");
        assert!(!canvas.contains(&"Preview".to_owned()), "{canvas:?}");
        scene.editor.update(cx, |e, cx| e.select(vec![scene.member.clone()], cx));
        let on_member = labels(cx, Some(scene.member.clone()));
        assert_eq!(on_member.first().map(String::as_str), Some("Preview"), "{on_member:?}");
        for wanted in ["Cut", "Copy", "Move", "Duplicate", "Delete", "Dissolve", "Frame selected", "Remove from frame"] {
            assert!(on_member.contains(&wanted.to_owned()), "{wanted} in {on_member:?}");
        }
        assert!(!on_member.contains(&"Group selected".to_owned()));
        render_frame(cx);
    }

    /// The node whose output feeds the fragment output, if any.
    fn previewed(editor: &Editor) -> Option<String> {
        let model = editor.model();
        let root = model.root();
        let frag = root.nodes.iter().find(|n| n.is(FRAG_OUTPUT))?.id.clone();
        root.edges.iter().find(|e| e.target == frag).map(|e| e.source.clone())
    }

    #[gpui::test]
    fn shift_command_click_previews_the_node(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        render_frame(cx);
        let member = scene.member.clone();
        let at = scene.editor.read_with(cx, |e, cx| {
            let flow = e.flow.read(cx);
            let r = flow.node_rect(&Id::from(member.clone())).expect("the node is measured");
            flow.flow_to_screen(Xy::new(r.x + r.width / 2.0, r.y + r.height / 2.0))
        });
        let shader = |cx: &mut gpui::VisualTestContext| {
            scene.editor.read_with(cx, |e, _| e.shader().clone().map(|p| p.program.code))
        };
        let before = shader(cx);
        let source = scene.editor.read_with(cx, |e, _| previewed(e));

        cx.simulate_click(at, gpui::Modifiers::default());
        assert_eq!(scene.editor.read_with(cx, |e, _| previewed(e)), source);
        assert_eq!(shader(cx), before);

        let preview = gpui::Modifiers { shift: true, platform: true, ..Default::default() };
        cx.simulate_click(at, preview);
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| previewed(e)), Some(member.clone()));
        assert_ne!(shader(cx), before, "the emitted program follows the preview");
        assert_eq!(scene.editor.read_with(cx, |e, _| e.selected()), vec![member]);
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

    fn add_node(scene: &Scene, cx: &mut gpui::VisualTestContext, node_type: &str, y: f32) -> String {
        let id = scene.editor.update(cx, |e, cx| {
            let id = e.model_mut().add_node(node_type, GraphPoint::new(400.0, y)).expect("adds");
            e.after_edit(cx);
            id
        });
        render_frame(cx);
        id
    }

    #[gpui::test]
    fn widgets_draw_on_their_nodes(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        for (i, ty) in ["curve", "color-ramp", "render-texture", "index-switch", "custom-wgsl"].into_iter().enumerate() {
            add_node(&scene, cx, ty, 300.0 * i as f32);
        }
        render_frame(cx);
    }

    #[gpui::test]
    fn a_curve_drag_is_one_undo_step(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let id = add_node(&scene, cx, "curve", 300.0);
        let linear = shader_graph::curve::linear_curve();
        scene.editor.update(cx, |e, cx| {
            for y in [0.2, 0.4, 0.6] {
                let points = shader_graph::curve::move_curve_point(&linear, 1, 1.0, y);
                e.set_curve(&id, points, cx);
            }
        });
        let moved = |cx: &mut gpui::VisualTestContext| {
            scene.editor.read_with(cx, |e, _| e.model().node(&id).and_then(|n| n.data.curve.clone()))
        };
        assert_eq!(moved(cx).map(|c| c[1].y), Some(0.6));
        scene.editor.update(cx, |e, cx| e.undo(cx));
        assert_ne!(moved(cx).map(|c| c[1].y), Some(0.2));
        assert_ne!(moved(cx).map(|c| c[1].y), Some(0.4));
    }

    #[gpui::test]
    fn cases_and_items_edit_their_nodes(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let switch = add_node(&scene, cx, "index-switch", 300.0);
        let render = add_node(&scene, cx, "render-texture", 600.0);
        let before = scene.editor.read_with(cx, |e, _| {
            let model = e.model();
            (shader_graph::nodes::index_switch_cases(&model.node(&switch).expect("node").data), model.render_items_of(&render).len())
        });
        scene.editor.update(cx, |e, cx| {
            e.set_cases(&switch, i64::from(before.0) + 1, cx);
            e.add_render_item(&render, cx);
        });
        let after = scene.editor.read_with(cx, |e, _| {
            let model = e.model();
            (shader_graph::nodes::index_switch_cases(&model.node(&switch).expect("node").data), model.render_items_of(&render).len())
        });
        assert_eq!((after.0, after.1), (before.0 + 1, before.1 + 1));
        render_frame(cx);
    }

    #[gpui::test]
    fn a_cut_stroke_deletes_the_links_it_crosses(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        let edge = scene.editor.read_with(cx, |e, _| e.model().root().edges.first().map(|x| x.id.clone())).expect("the demo has a link");
        scene.editor.update(cx, |e, cx| {
            e.on_flow_event(
                FlowEvent::Stroke { kind: gpui_flow::StrokeKind::Cut, crossings: vec![(edge.into(), Xy::new(0.0, 0.0))] },
                cx,
            )
        });
        assert!(scene.editor.read_with(cx, |e, _| e.model().root().edges.is_empty()));
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

    fn png_of_width(width: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, 1, image::Rgba([9, 9, 9, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    /// An editor attached to a playback, with one imported image in the project and no files loaded.
    fn media_scene(cx: &mut TestAppContext) -> (Entity<Editor>, Entity<Playback>, String, &mut gpui::VisualTestContext) {
        cx.update(theme::init);
        let (editor, vcx) = cx.add_window_view(Editor::new);
        let playback = vcx.new(|_| Playback::new());
        editor.update(vcx, |e, cx| e.attach_playback(&playback, cx));
        let mut asset = String::new();
        let bytes = png_of_width(2);
        editor.update(vcx, |e, cx| {
            e.edit_model(cx, |m| {
                let file = shader_graph::assets::ImportFile { name: "Photo.png", mime: "image/png", bytes: &bytes };
                asset = m.import_file(file, &runtime::SystemVideoProbe).expect("imports").asset.id().to_owned();
            })
        });
        vcx.run_until_parked();
        (editor, playback, asset, vcx)
    }

    fn set_files(playback: &Entity<Playback>, files: AssetFiles, vcx: &mut gpui::VisualTestContext) {
        vcx.update(|window, cx| playback.update(cx, |p, cx| p.set_asset_files(files, window, cx)));
        vcx.run_until_parked();
    }

    fn node_asset(editor: &Entity<Editor>, node: &str, vcx: &mut gpui::VisualTestContext) -> Option<String> {
        editor.read_with(vcx, |e, _| e.model().node(node).and_then(|n| n.data.asset.clone()))
    }

    #[gpui::test]
    fn picking_an_asset_sets_the_node_and_is_one_undo_step(cx: &mut TestAppContext) {
        let (editor, _playback, asset, vcx) = media_scene(cx);
        let node = editor.update(vcx, |e, cx| {
            let id = e.model_mut().add_node(IMAGE, GraphPoint::new(0.0, 0.0)).expect("adds");
            e.after_edit(cx);
            id
        });
        editor.update(vcx, |e, cx| e.set_node_asset(&node, Some(&asset), cx));
        assert_eq!(node_asset(&editor, &node, vcx), Some(asset.clone()));

        editor.update(vcx, |e, cx| e.undo(cx));
        assert_eq!(node_asset(&editor, &node, vcx), None);
    }

    #[gpui::test]
    fn dropping_an_asset_adds_a_node_reading_it_where_it_lands(cx: &mut TestAppContext) {
        let (editor, _playback, asset, vcx) = media_scene(cx);
        let at = gpui::point(px(300.0), px(200.0));
        let expected = editor.read_with(vcx, |e, cx| e.flow.read(cx).screen_to_flow(at));
        editor.update(vcx, |e, cx| e.drop_assets(std::slice::from_ref(&asset), at, cx));

        let added = editor.read_with(vcx, |e, _| {
            let model = e.model();
            let doc = model.active_document();
            doc.nodes.iter().find(|n| n.data.asset.as_deref() == Some(asset.as_str())).map(|n| (n.node_type.clone(), n.position))
        });
        let (node_type, position) = added.expect("a node reads the dropped asset");
        assert_eq!(node_type, IMAGE);
        assert_eq!((position.x, position.y), (expected.x, expected.y));
    }

    #[gpui::test]
    fn a_finder_drop_on_the_graph_imports_and_adds_a_node_where_it_lands(cx: &mut TestAppContext) {
        let (editor, _playback, _asset, vcx) = media_scene(cx);
        let dir = std::env::temp_dir().join(format!("wgnodes-finder-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("Dropped.png");
        std::fs::write(&path, png_of_width(4)).expect("writes");

        let at = point(px(300.0), px(200.0));
        let expected = editor.read_with(vcx, |e, cx| e.flow.read(cx).screen_to_flow(at));
        let paths = gpui::ExternalPaths(std::iter::once(path).collect());
        vcx.simulate_event(gpui::FileDropEvent::Entered { position: at, paths });
        vcx.simulate_event(gpui::FileDropEvent::Submit { position: at });
        vcx.run_until_parked();

        let added = editor.read_with(vcx, |e, _| {
            let model = e.model();
            let doc = model.active_document();
            doc.nodes
                .iter()
                .find(|n| n.node_type == IMAGE && n.data.asset.is_some())
                .map(|n| (model.asset(n.data.asset.as_deref().unwrap_or_default()).map(|a| a.name().to_owned()), n.position))
        });
        let (name, position) = added.expect("a node reads the dropped file");
        assert_eq!(name.as_deref(), Some("Dropped"));
        assert_eq!((position.x, position.y), (expected.x, expected.y));
        std::fs::remove_dir_all(dir).ok();
    }

    #[gpui::test]
    fn a_node_whose_asset_file_is_missing_shows_the_warning(cx: &mut TestAppContext) {
        let (editor, playback, asset, vcx) = media_scene(cx);
        editor.update(vcx, |e, cx| {
            let id = e.model_mut().add_node(IMAGE, GraphPoint::new(0.0, 0.0)).expect("adds");
            e.set_node_asset(&id, Some(&asset), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("missing-file").is_some(), "no file, so the node warns");

        let mut files = AssetFiles::new();
        files.insert(asset.clone(), Arc::new(png_of_width(2)));
        set_files(&playback, files, vcx);
        assert!(vcx.debug_bounds("missing-file").is_none(), "the file arrived, so the warning is gone");
    }

    /// Focuses the flow and draws, as the window has them when a menu item is chosen.
    fn focus_flow(scene: &Scene, cx: &mut gpui::VisualTestContext) {
        let handle = scene.editor.read_with(cx, |e, cx| e.flow.focus_handle(cx));
        cx.update(|window, cx| window.focus(&handle, cx));
        render_frame(cx);
    }

    fn enabled(cx: &mut gpui::VisualTestContext, action: &dyn gpui::Action) -> bool {
        cx.update(|window, cx| window.is_action_available(action, cx))
    }

    fn select_nodes(scene: &Scene, cx: &mut gpui::VisualTestContext, ids: Vec<String>) {
        scene.editor.update(cx, |e, cx| e.select(ids, cx));
        render_frame(cx);
    }

    #[gpui::test]
    fn menu_actions_follow_the_selection_as_the_original_enables_them(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        select_nodes(&scene, cx, Vec::new());
        assert!(enabled(cx, &LayoutGraph) && enabled(cx, &LayoutDocument));
        assert!(!enabled(cx, &LayoutSelected) && !enabled(cx, &GroupSelected) && !enabled(cx, &PreviewSelectedNode));

        select_nodes(&scene, cx, vec![scene.member.clone()]);
        assert!(enabled(cx, &LayoutSelected) && enabled(cx, &GroupSelected) && enabled(cx, &ToggleNodeSheet));
        assert!(enabled(cx, &PreviewSelectedNode), "one selected node can be previewed");
        assert!(!enabled(cx, &UngroupSelected) && !enabled(cx, &EnterNodeGroup) && !enabled(cx, &ExitNodeGroup));

        select_nodes(&scene, cx, vec![scene.member.clone(), scene.loose.clone()]);
        assert!(!enabled(cx, &PreviewSelectedNode), "only a single selected node can be previewed");
    }

    #[gpui::test]
    fn layout_graph_is_one_undo_step(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        cx.dispatch_action(LayoutGraph);
        render_frame(cx);
        assert!(scene.editor.read_with(cx, |e, _| e.model().can_undo()), "the layout is an edit");

        cx.dispatch_action(Undo);
        render_frame(cx);
        assert!(scene.editor.read_with(cx, |e, _| e.model().can_redo()), "undo leaves the layout to redo");
    }

    #[gpui::test]
    fn remove_unused_groups_is_enabled_only_with_unused_groups_and_asks_first(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        assert!(!enabled(cx, &PruneUnusedGroups));

        let group = scene.editor.update(cx, |e, cx| {
            let id = e.model_mut().create_group(GraphPoint::new(0.0, 0.0)).expect("creates");
            e.after_edit(cx);
            id
        });
        render_frame(cx);
        assert!(!enabled(cx, &PruneUnusedGroups), "a group the project uses isn't unused");

        scene.editor.update(cx, |e, cx| {
            assert!(e.model_mut().delete_nodes(std::slice::from_ref(&group), &[]));
            e.after_edit(cx);
        });
        render_frame(cx);
        assert!(enabled(cx, &PruneUnusedGroups));

        cx.dispatch_action(PruneUnusedGroups);
        render_frame(cx);
        assert!(
            !scene.editor.read_with(cx, |e, _| e.model().prunable_groups().is_empty()),
            "the groups stay until the user confirms the prompt"
        );
    }

    #[gpui::test]
    fn preview_selected_node_previews_the_single_selection(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        select_nodes(&scene, cx, vec![scene.loose.clone()]);
        cx.dispatch_action(PreviewSelectedNode);
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| e.selected()), vec![scene.loose.clone()]);
    }

    #[gpui::test]
    fn group_ungroup_enter_and_exit_act_as_their_menu_items(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        select_nodes(&scene, cx, vec![scene.member.clone(), scene.loose.clone()]);

        cx.dispatch_action(GroupSelected);
        render_frame(cx);
        let group = scene.editor.read_with(cx, |e, _| e.selected());
        assert_eq!(group.len(), 1, "the group is selected once made");
        assert!(enabled(cx, &UngroupSelected) && enabled(cx, &EnterNodeGroup));

        cx.dispatch_action(EnterNodeGroup);
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| e.model().path().to_vec()), group);

        cx.dispatch_action(ExitNodeGroup);
        render_frame(cx);
        assert!(scene.editor.read_with(cx, |e, _| e.model().path().is_empty()));

        select_nodes(&scene, cx, group.clone());
        cx.dispatch_action(UngroupSelected);
        render_frame(cx);
        assert!(scene.editor.read_with(cx, |e, _| e.model().node(&group[0]).is_none()));
    }

    #[gpui::test]
    fn export_selected_group_is_enabled_only_with_a_group_selected(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        select_nodes(&scene, cx, vec![scene.loose.clone()]);
        assert!(!enabled(cx, &ExportSelectedGroup));

        select_nodes(&scene, cx, vec![scene.member.clone(), scene.loose.clone()]);
        cx.dispatch_action(GroupSelected);
        render_frame(cx);
        assert!(enabled(cx, &ExportSelectedGroup));
        assert!(scene.editor.read_with(cx, |e, _| e.selected_group_ref().is_some()));
    }

    #[gpui::test]
    fn toggle_node_sheet_opens_the_selection_and_escape_closes_it(cx: &mut TestAppContext) {
        let (scene, cx) = scene(cx);
        focus_flow(&scene, cx);
        select_nodes(&scene, cx, vec![scene.member.clone()]);

        cx.dispatch_action(ToggleNodeSheet);
        render_frame(cx);
        assert_eq!(scene.editor.read_with(cx, |e, _| e.sheet.clone()), Some(scene.member.clone()));

        cx.dispatch_action(ExitNodeGroup);
        render_frame(cx);
        assert!(scene.editor.read_with(cx, |e, _| e.sheet.is_none()), "escape closes the sheet first");
    }
}
