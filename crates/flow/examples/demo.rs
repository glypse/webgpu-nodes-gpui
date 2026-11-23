use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, IntoElement,
    ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_flow::{
    ControlsOptions, Edge, Flow, FlowEvent, HandleType, MiniMapOptions, Node, NodeProps, Position,
    Xy, add_edge, apply_edge_changes, apply_node_changes,
};

struct Demo {
    flow: Entity<Flow>,
}

fn card(
    label: &'static str,
    inputs: usize,
    outputs: usize,
) -> impl Fn(&NodeProps, &mut Window, &mut App) -> AnyElement {
    move |props, _, _| {
        let s = |v: f32| props.scaled(v);
        let theme = props.theme;
        div()
            .relative()
            .w(s(140.0))
            .rounded(s(4.0))
            .border(px(1.0))
            .border_color(if props.selected {
                theme.node_border_selected
            } else {
                theme.node_border
            })
            .bg(theme.node_background)
            .text_color(theme.node_text)
            .text_size(s(12.0))
            .child(div().p(s(8.0)).child(label))
            .children((0..outputs).map(|i| {
                div()
                    .relative()
                    .h(s(22.0))
                    .px(s(8.0))
                    .flex()
                    .justify_end()
                    .items_center()
                    .child(format!("out {i}"))
                    .child(props.handle(
                        Some(format!("out{i}").into()),
                        HandleType::Source,
                        Position::Right,
                    ))
            }))
            .children((0..inputs).map(|i| {
                div()
                    .relative()
                    .h(s(22.0))
                    .px(s(8.0))
                    .flex()
                    .items_center()
                    .child(format!("in {i}"))
                    .child(props.handle(
                        Some(format!("in{i}").into()),
                        HandleType::Target,
                        Position::Left,
                    ))
            }))
            .into_any_element()
    }
}

impl Demo {
    fn new(cx: &mut Context<Self>) -> Self {
        let flow = cx.new(|cx| {
            let mut flow = Flow::new(cx);
            flow.options.fit_view_on_init = true;
            flow.options.controls = Some(ControlsOptions::default());
            flow.options.minimap = Some(MiniMapOptions::default());
            flow.options.snap_grid = Some((10.0, 10.0));
            flow.options.edges_reconnectable = true;
            flow
        });
        flow.update(cx, |flow, cx| {
            flow.set_nodes(
                vec![
                    Node::new("a", Xy::new(0.0, 0.0)).render(card("Time", 0, 1)),
                    Node::new("b", Xy::new(260.0, 40.0)).render(card("Add", 2, 1)),
                    Node::new("c", Xy::new(520.0, 0.0)).render(card("Output", 1, 0)),
                    Node::new("d", Xy::new(0.0, 160.0)).label("Default"),
                ],
                cx,
            );
            flow.set_edges(
                vec![Edge::new("e1", "a", Some("out0".into()), "b", Some("in0".into()))],
                cx,
            );
        });
        cx.subscribe(&flow, |_, flow, event: &FlowEvent, cx| match event {
            FlowEvent::NodesChange(changes) => {
                let nodes = apply_node_changes(changes, flow.read(cx).nodes().to_vec());
                flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
            }
            FlowEvent::EdgesChange(changes) => {
                let edges = apply_edge_changes(changes, flow.read(cx).edges().to_vec());
                flow.update(cx, |f, cx| f.set_edges(edges, cx));
            }
            FlowEvent::Connect(c) => {
                let edges = add_edge(c, flow.read(cx).edges().to_vec());
                flow.update(cx, |f, cx| f.set_edges(edges, cx));
            }
            FlowEvent::Delete { nodes, edges } => {
                let mut n = flow.read(cx).nodes().to_vec();
                n.retain(|x| !nodes.contains(&x.id));
                let mut e = flow.read(cx).edges().to_vec();
                e.retain(|x| !edges.contains(&x.id));
                flow.update(cx, |f, cx| {
                    f.set_nodes(n, cx);
                    f.set_edges(e, cx);
                });
            }
            _ => {}
        })
        .detach();
        Self { flow }
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.flow.clone())
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(Demo::new),
        )
        .expect("open the demo window");
        cx.activate(true);
    });
}
