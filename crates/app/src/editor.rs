use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    anchored, deferred, div, hsla, point, prelude::FluentBuilder as _, px,
};
use gpui_flow::{
    ControlsOptions, Edge as FlowEdge, Flow, FlowEvent, HandleType, Id, Node as FlowNode, NodeProps,
    Position, Xy,
};
use shader_graph::{DocId, Edge, Endpoint, NodeId, NodeKind, Project};

struct Menu {
    graph: Xy,
    window: Point<Pixels>,
}

/// What the shell needs from the editor.
#[derive(Clone, Debug)]
pub enum EditorEvent {
    /// The project's WGSL changed, or couldn't be generated.
    Shader(Result<String, String>),
}

pub struct Editor {
    project: Project,
    /// The documents entered, from the root down to the one shown.
    path: Vec<DocId>,
    flow: Entity<Flow>,
    shader: Result<String, String>,
    error: Option<SharedString>,
    menu: Option<Menu>,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Editor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let flow = cx.new(|cx| {
            let mut flow = Flow::new(cx);
            flow.options.fit_view_on_init = true;
            flow.options.delete_key = true;
            flow.options.controls = Some(ControlsOptions { show_zoom: false, show_interactive: false, ..Default::default() });
            flow.options.min_zoom = 0.1;
            flow.options.max_zoom = 3.0;
            flow
        });
        cx.subscribe(&flow, |this, _, event, cx| {
            this.on_flow_event(event.clone(), cx)
        })
        .detach();
        window.focus(&flow.focus_handle(cx));
        let mut editor = Self {
            project: Project::demo(),
            path: vec![DocId::Root],
            flow,
            shader: Err(String::new()),
            error: None,
            menu: None,
        };
        editor.sync(cx);
        editor.recompile(cx);
        editor
    }

    fn doc(&self) -> DocId {
        *self.path.last().expect("the root is always in the path")
    }

    fn on_flow_event(&mut self, event: FlowEvent, cx: &mut Context<Self>) {
        let doc = self.doc();
        let node_id = |id: &Id| id.parse::<u64>().ok().map(NodeId);
        let socket = |h: &Option<Id>| -> usize {
            h.as_ref()
                .and_then(|h| h.trim_start_matches(['i', 'o']).parse().ok())
                .unwrap_or(0)
        };
        let result = match event {
            FlowEvent::Connect(c) => {
                let (Some(from), Some(to)) = (node_id(&c.source), node_id(&c.target)) else {
                    return;
                };
                self.project.connect(
                    doc,
                    Edge {
                        from: Endpoint { node: from, socket: socket(&c.source_handle) },
                        to: Endpoint { node: to, socket: socket(&c.target_handle) },
                    },
                )
            }
            FlowEvent::NodeDragStop { nodes, .. } => {
                let positions: Vec<(Id, Xy)> = self
                    .flow
                    .read(cx)
                    .nodes()
                    .iter()
                    .filter(|n| nodes.contains(&n.id))
                    .map(|n| (n.id.clone(), n.position))
                    .collect();
                for (id, p) in positions {
                    if let Some(id) = node_id(&id) {
                        self.project.move_node(doc, id, [p.x, p.y]).ok();
                    }
                }
                return;
            }
            FlowEvent::Delete { nodes, .. } => {
                let ids: Vec<NodeId> = nodes.iter().filter_map(node_id).collect();
                self.project.remove_nodes(doc, &ids)
            }
            FlowEvent::NodeDoubleClick { id } => {
                let kind = node_id(&id)
                    .and_then(|id| self.project.document(doc).and_then(|d| d.node(id)))
                    .map(|n| n.kind);
                if let Some(NodeKind::Group(group)) = kind {
                    self.path.push(DocId::Group(group));
                    self.menu = None;
                    self.sync(cx);
                }
                return;
            }
            FlowEvent::PaneContextMenu { window, flow } => {
                self.menu = Some(Menu { graph: flow, window });
                cx.notify();
                return;
            }
            FlowEvent::NodesChange(changes) => {
                let nodes = gpui_flow::apply_node_changes(&changes, self.flow.read(cx).nodes().to_vec());
                self.flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
                return;
            }
            FlowEvent::EdgesChange(changes) => {
                let edges = gpui_flow::apply_edge_changes(&changes, self.flow.read(cx).edges().to_vec());
                self.flow.update(cx, |f, cx| f.set_edges(edges, cx));
                return;
            }
            _ => return,
        };
        self.error = result.err().map(|e| e.to_string().into());
        self.sync(cx);
        self.recompile(cx);
    }

    /// Shows the current document in the flow.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let doc = self.doc();
        let Some(document) = self.project.document(doc) else {
            return;
        };
        let selected: Vec<Id> = self.flow.read(cx).selected_nodes();
        let nodes = document
            .nodes
            .iter()
            .map(|node| {
                let signature = self.project.signature(doc, node.kind);
                let title = signature.title.clone();
                let tint = color(node.kind);
                let inputs: Vec<(String, f32)> = signature
                    .inputs
                    .iter()
                    .zip(&node.values)
                    .map(|(s, v)| (s.label.clone(), *v))
                    .collect();
                let outputs: Vec<String> = signature.outputs.iter().map(|s| s.label.clone()).collect();
                let id: Id = node.id.0.to_string().into();
                let mut n = FlowNode::new(id.clone(), Xy::new(node.position[0], node.position[1]))
                    .render(move |props, _, _| card(props, &title, tint, &inputs, &outputs))
                    .width(170.0);
                n.selected = selected.contains(&id);
                n
            })
            .collect();
        let edges = document
            .edges
            .iter()
            .map(|e| {
                FlowEdge::new(
                    format!("{}:{}-{}:{}", e.from.node.0, e.from.socket, e.to.node.0, e.to.socket),
                    e.from.node.0.to_string(),
                    Some(format!("o{}", e.from.socket).into()),
                    e.to.node.0.to_string(),
                    Some(format!("i{}", e.to.socket).into()),
                )
            })
            .collect();
        self.flow.update(cx, |flow, cx| {
            flow.set_nodes(nodes, cx);
            flow.set_edges(edges, cx);
        });
        cx.notify();
    }

    fn recompile(&mut self, cx: &mut Context<Self>) {
        self.shader = shader_graph::generate(&self.project).map_err(|e| e.to_string());
        cx.emit(EditorEvent::Shader(self.shader.clone()));
    }

    /// The WGSL of the project as it is now.
    pub fn shader(&self) -> &Result<String, String> {
        &self.shader
    }

    fn add_node(&mut self, kind: NodeKind, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let doc = self.doc();
        match self
            .project
            .add_node(doc, kind, [menu.graph.x, menu.graph.y])
        {
            Ok(id) => {
                self.sync(cx);
                {
                    let id: Id = id.0.to_string().into();
                    self.sync(cx);
                    self.flow.update(cx, |flow, cx| flow.set_selection(&[id], &[], cx));
                }
            }
            Err(e) => self.error = Some(e.to_string().into()),
        }
        self.recompile(cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "g" if keystroke.modifiers.platform || keystroke.modifiers.control => {
                let selection: Vec<NodeId> = self
                    .flow
                    .read(cx)
                    .selected_nodes()
                    .iter()
                    .filter_map(|id| id.parse().ok().map(NodeId))
                    .collect();
                match self.project.group_nodes(self.doc(), &selection) {
                    Ok(id) => {
                        self.sync(cx);
                        {
                    let id: Id = id.0.to_string().into();
                    self.sync(cx);
                    self.flow.update(cx, |flow, cx| flow.set_selection(&[id], &[], cx));
                }
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e.to_string().into()),
                }
                self.recompile(cx);
            }
            "escape" if self.menu.is_some() => {
                self.menu = None;
                cx.notify();
            }
            "escape" | "tab" if self.path.len() > 1 => self.leave_to(self.path.len() - 2, cx),
            _ => {}
        }
    }

    fn leave_to(&mut self, depth: usize, cx: &mut Context<Self>) {
        self.path.truncate(depth + 1);
        self.menu = None;
        self.sync(cx);
    }

    fn doc_label(&self, doc: DocId) -> SharedString {
        match doc {
            DocId::Root => "Root".into(),
            DocId::Group(id) => self
                .project
                .groups
                .get(&id)
                .map_or("Missing".into(), |g| g.label.clone().into()),
        }
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let doc = self.doc();
        let groups = self.project.groups.iter().filter(|(id, _)| match doc {
            DocId::Group(current) => !self.project.group_uses(**id, current),
            DocId::Root => true,
        });
        let entries: Vec<(NodeKind, SharedString)> = NodeKind::catalog()
            .map(|k| (k, k.label().into()))
            .chain(groups.map(|(id, g)| (NodeKind::Group(*id), g.label.clone().into())))
            .collect();
        let items = entries.into_iter().enumerate().map(|(i, (kind, label))| {
            div()
                .id(i)
                .px_3()
                .py_1()
                .rounded_sm()
                .hover(|s| s.bg(hsla(0.6, 0.5, 0.45, 1.0)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.add_node(kind, cx)))
                .child(label)
        });
        let list = div()
            .id("add-menu")
            .w(px(180.0))
            .max_h(px(420.0))
            .overflow_y_scroll()
            .p_1()
            .rounded_md()
            .bg(hsla(0.0, 0.0, 0.16, 1.0))
            .border_1()
            .border_color(hsla(0.0, 0.0, 0.3, 1.0))
            .shadow_lg()
            .text_sm()
            .text_color(hsla(0.0, 0.0, 0.9, 1.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .children(items);
        Some(
            deferred(
                anchored()
                    .position(menu.window)
                    .snap_to_window()
                    .child(list),
            )
            .into_any_element(),
        )
    }
}

fn card(
    props: &NodeProps,
    title: &str,
    tint: Hsla,
    inputs: &[(String, f32)],
    outputs: &[String],
) -> AnyElement {
    let s = |v: f32| props.scaled(v);
    let theme = props.theme;
    let row = || div().relative().h(s(22.0)).px(s(10.0)).flex().items_center();
    div()
        .relative()
        .rounded(s(6.0))
        .border(px(1.0))
        .border_color(if props.selected { theme.node_border_selected } else { theme.node_border })
        .bg(theme.node_background)
        .text_color(theme.node_text)
        .text_size(s(12.0))
        .child(div().h(s(26.0)).px(s(8.0)).flex().items_center().rounded_t(s(5.0)).bg(tint).child(title.to_string()))
        .children(outputs.iter().enumerate().map(|(i, label)| {
            row().justify_end().child(label.clone()).child(props.handle(
                Some(format!("o{i}").into()),
                HandleType::Source,
                Position::Right,
            ))
        }))
        .children(inputs.iter().enumerate().map(|(i, (label, value))| {
            let linked = props.is_connected(HandleType::Target, Some(&format!("i{i}").into()));
            row()
                .justify_between()
                .child(label.clone())
                .when(!linked, |d| d.child(format!("{value:.3}")))
                .child(props.handle(Some(format!("i{i}").into()), HandleType::Target, Position::Left))
        }))
        .child(div().h(s(6.0)))
        .into_any_element()
}

fn color(kind: NodeKind) -> Hsla {
    match kind {
        NodeKind::Uv | NodeKind::Time | NodeKind::Value => hsla(0.0, 0.45, 0.38, 1.0),
        NodeKind::Math(_) => hsla(0.6, 0.4, 0.38, 1.0),
        NodeKind::Group(_) => hsla(0.33, 0.35, 0.32, 1.0),
        NodeKind::Output | NodeKind::GroupInput | NodeKind::GroupOutput => hsla(0.0, 0.0, 0.3, 1.0),
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.flow.focus_handle(cx)
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = hsla(0.0, 0.0, 0.85, 1.0);
        let muted = hsla(0.0, 0.0, 0.55, 1.0);
        let crumbs = self.path.iter().enumerate().map(|(depth, doc)| {
            let current = depth + 1 == self.path.len();
            div()
                .id(("crumb", depth))
                .px_2()
                .rounded_sm()
                .text_color(if current { text } else { muted })
                .when(!current, |d| d.hover(|s| s.bg(hsla(0.0, 0.0, 0.25, 1.0))))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.leave_to(depth, cx)))
                .child(self.doc_label(*doc))
        });
        let breadcrumb = div()
            .flex()
            .gap_1()
            .items_center()
            .h(px(32.0))
            .px_2()
            .bg(hsla(0.0, 0.0, 0.15, 1.0))
            .border_b_1()
            .border_color(hsla(0.0, 0.0, 0.08, 1.0))
            .text_sm()
            .children(crumbs);

        div()
            .flex()
            .size_full()
            .bg(hsla(0.0, 0.0, 0.12, 1.0))
            .on_key_down(cx.listener(|this, e, _, cx| this.key_down(e, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(breadcrumb)
                    .child(div().flex_1().child(self.flow.clone())),
            )
            .children(self.error.clone().map(|e| {
                div()
                    .absolute()
                    .left_2()
                    .bottom_2()
                    .p_2()
                    .rounded_sm()
                    .bg(hsla(0.0, 0.5, 0.25, 1.0))
                    .text_xs()
                    .child(e)
            }))
            .children(self.render_menu(cx))
    }
}
