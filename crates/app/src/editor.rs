use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    anchored, deferred, div, hsla, point, prelude::FluentBuilder as _, px,
};
use gpui_flow::{Flow, FlowEdge, FlowEvent, FlowNode, FlowPort, PortRef};
use shader_graph::{DocId, Edge, Endpoint, NodeId, NodeKind, Project};

struct Menu {
    graph: Point<f32>,
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
        let flow = cx.new(Flow::new);
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
        let endpoint = |p: PortRef| Endpoint {
            node: NodeId(p.node),
            socket: p.port,
        };
        let result = match event {
            FlowEvent::Connected(FlowEdge { from, to }) => self.project.connect(
                doc,
                Edge {
                    from: endpoint(from),
                    to: endpoint(to),
                },
            ),
            FlowEvent::Disconnected(to) => self.project.disconnect(doc, endpoint(to)),
            FlowEvent::NodesMoved(moved) => {
                for (id, p) in moved {
                    self.project.move_node(doc, NodeId(id), [p.x, p.y]).ok();
                }
                return;
            }
            FlowEvent::Deleted(ids) => {
                let ids: Vec<NodeId> = ids.into_iter().map(NodeId).collect();
                self.project.remove_nodes(doc, &ids)
            }
            FlowEvent::ValueChanged { port, value } => {
                let result = self.project.set_value(doc, endpoint(port), value);
                // The flow already shows the value: only the shader changes.
                self.recompile(cx);
                cx.notify();
                return result.unwrap_or_default();
            }
            FlowEvent::Opened(id) => {
                let kind = self
                    .project
                    .document(doc)
                    .and_then(|d| d.node(NodeId(id)))
                    .map(|n| n.kind);
                if let Some(NodeKind::Group(group)) = kind {
                    self.path.push(DocId::Group(group));
                    self.menu = None;
                }
                Ok(())
            }
            FlowEvent::ContextMenu { graph, window } => {
                self.menu = Some(Menu { graph, window });
                cx.notify();
                return;
            }
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
        let nodes = document
            .nodes
            .iter()
            .map(|node| {
                let signature = self.project.signature(doc, node.kind);
                FlowNode {
                    id: node.id.0,
                    position: point(node.position[0], node.position[1]),
                    title: signature.title.into(),
                    color: color(node.kind),
                    inputs: signature
                        .inputs
                        .into_iter()
                        .zip(&node.values)
                        .map(|(socket, value)| FlowPort {
                            label: socket.label.into(),
                            value: Some(*value),
                        })
                        .collect(),
                    outputs: signature
                        .outputs
                        .into_iter()
                        .map(|socket| FlowPort {
                            label: socket.label.into(),
                            value: None,
                        })
                        .collect(),
                }
            })
            .collect();
        let edges = document
            .edges
            .iter()
            .map(|e| FlowEdge {
                from: PortRef {
                    node: e.from.node.0,
                    port: e.from.socket,
                },
                to: PortRef {
                    node: e.to.node.0,
                    port: e.to.socket,
                },
            })
            .collect();
        self.flow
            .update(cx, |flow, cx| flow.set_graph(nodes, edges, cx));
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
                self.flow.update(cx, |flow, cx| flow.select([id.0], cx));
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
                    .selection()
                    .into_iter()
                    .map(NodeId)
                    .collect();
                match self.project.group_nodes(self.doc(), &selection) {
                    Ok(id) => {
                        self.sync(cx);
                        self.flow.update(cx, |flow, cx| flow.select([id.0], cx));
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
