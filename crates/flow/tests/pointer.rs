use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Entity, IntoElement as _, Modifiers, MouseButton,
    ParentElement as _, ScrollDelta, Styled as _, TestAppContext, VisualTestContext, Window, div,
    point, px,
};
use gpui_flow::{
    Edge, Flow, FlowEvent, HandleType, Node, NodeProps, Position, Xy, apply_edge_changes,
    apply_node_changes,
};

fn card(props: &NodeProps, _: &mut Window, _: &mut App) -> AnyElement {
    div()
        .relative()
        .w(props.scaled(160.0))
        .h(props.scaled(80.0))
        .bg(props.theme.node_background)
        .child(props.handle(Some("in".into()), HandleType::Target, Position::Left))
        .child(props.handle(Some("out".into()), HandleType::Source, Position::Right))
        .into_any_element()
}

fn nodes() -> Vec<Node> {
    vec![
        Node::new("a", Xy::new(0.0, 0.0)).render(card),
        Node::new("b", Xy::new(400.0, 100.0)).render(card),
    ]
}

fn setup(cx: &mut TestAppContext, with_edge: bool) -> (Entity<Flow>, &mut VisualTestContext) {
    let (flow, cx) = cx.add_window_view(|_, cx| {
        let mut flow = Flow::new(cx);
        flow.options.fit_view_on_init = true;
        flow
    });
    cx.update(|_, cx| {
        flow.update(cx, |f, cx| {
            f.set_nodes(nodes(), cx);
            if with_edge {
                f.set_edges(vec![Edge::new("e", "a", Some("out".into()), "b", Some("in".into()))], cx);
            }
        });
    });
    cx.subscribe_events(&flow);
    settle(cx);
    (flow, cx)
}

trait Subscribe {
    fn subscribe_events(&mut self, flow: &Entity<Flow>);
}

impl Subscribe for VisualTestContext {
    /// Applies changes to the flow the way an owner does.
    fn subscribe_events(&mut self, flow: &Entity<Flow>) {
        let flow = flow.clone();
        self.update(|_, cx| {
            cx.subscribe(&flow.clone(), |flow, event: &FlowEvent, cx| match event {
                FlowEvent::NodesChange(changes) => {
                    let nodes = apply_node_changes(changes, flow.read(cx).nodes().to_vec());
                    flow.update(cx, |f, cx| f.set_nodes(nodes, cx));
                }
                FlowEvent::EdgesChange(changes) => {
                    let edges = apply_edge_changes(changes, flow.read(cx).edges().to_vec());
                    flow.update(cx, |f, cx| f.set_edges(edges, cx));
                }
                _ => {}
            })
            .detach();
        });
    }
}

/// Draws until measurements and the initial fit have settled.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..6 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

/// Sets a flag when the flow emits an event matching `wanted`.
fn record(cx: &mut VisualTestContext, flow: &Entity<Flow>, wanted: fn(&FlowEvent) -> bool) -> Rc<Cell<bool>> {
    let seen = Rc::new(Cell::new(false));
    let flag = seen.clone();
    cx.update(|_, cx| {
        cx.subscribe(flow, move |_, event: &FlowEvent, _| {
            if wanted(event) {
                flag.set(true);
            }
        })
        .detach();
    });
    seen
}

fn screen_of(flow: &Entity<Flow>, cx: &mut VisualTestContext, id: &'static str, local: Xy) -> gpui::Point<gpui::Pixels> {
    let id: gpui_flow::Id = id.into();
    let rect = cx.read(|cx| flow.read(cx).node_rect(&id)).expect("node measured");
    cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(rect.x + local.x, rect.y + local.y)))
}

#[gpui::test]
fn the_initial_fit_pads_the_measured_nodes(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let (viewport, (w, h)) = cx.read(|cx| (flow.read(cx).viewport(), flow.read(cx).size()));
    assert!(w > 0.0 && h > 0.0);
    let a = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    let b = cx.read(|cx| flow.read(cx).node_rect(&"b".into())).expect("measured");
    let left = viewport.x + a.x * viewport.zoom;
    let right = viewport.x + (b.x + b.width) * viewport.zoom;
    assert!(left > w * 0.04, "left edge is flush: {left} of {w}");
    assert!(w - right > w * 0.04, "right edge is flush: {} of {w}", w - right);
    assert!(((left + right) / 2.0 - w / 2.0).abs() < 2.0, "content is centred");
}

#[gpui::test]
fn click_and_drag_on_the_empty_pane(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let viewport = cx.read(|cx| flow.read(cx).viewport());
    // Below node a and left of node b, so the press lands on the pane.
    let start = screen_of(&flow, cx, "a", Xy::new(0.0, 220.0));
    let end = point(start.x + px(40.0), start.y + px(30.0));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    cx.simulate_click(end, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    // The drag pans the pane by its offset and leaves the zoom alone.
    let panned = cx.read(|cx| flow.read(cx).viewport());
    assert!((panned.x - viewport.x - 40.0).abs() < 0.5, "pan x {} from {}", panned.x, viewport.x);
    assert!((panned.y - viewport.y - 30.0).abs() < 0.5, "pan y {} from {}", panned.y, viewport.y);
    assert_eq!(panned.zoom, viewport.zoom);
}

#[gpui::test]
fn box_selection_selects_nodes_inside(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let shift = Modifiers { shift: true, ..Default::default() };
    cx.simulate_mouse_down(point(px(2.0), px(2.0)), MouseButton::Left, shift);
    cx.simulate_mouse_move(point(px(1900.0), px(1070.0)), Some(MouseButton::Left), shift);
    cx.simulate_mouse_up(point(px(1900.0), px(1070.0)), MouseButton::Left, shift);
    settle(cx);
    assert_eq!(guard::recovered(), before);
    let (w, h) = cx.read(|cx| flow.read(cx).size());
    assert_eq!(cx.read(|cx| flow.read(cx).selected_nodes().len()), 2, "canvas {w}x{h}");
}

#[gpui::test]
fn dragging_a_node_moves_it(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let from_rect = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    let from = screen_of(&flow, cx, "a", Xy::new(80.0, 40.0));
    let to = point(from.x + px(120.0), from.y + px(60.0));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(from.x + px(60.0), from.y + px(30.0)), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    let moved = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    assert!(moved.x > from_rect.x && moved.y > from_rect.y);
}

#[gpui::test]
fn connecting_two_handles_adds_an_edge(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let connected = record(cx, &flow, |event| matches!(event, FlowEvent::Connect(_)));
    let a = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    let b = cx.read(|cx| flow.read(cx).node_rect(&"b".into())).expect("measured");
    let source = cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(a.x + a.width, a.y + 40.0)));
    let target = cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(b.x, b.y + 40.0)));
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(source.x + px(30.0), source.y), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(connected.get(), "no connection was made");
}

#[gpui::test]
fn dropping_a_connection_on_empty_space(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let ended = record(cx, &flow, |event| matches!(event, FlowEvent::ConnectEnd(_)));
    let a = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    let source = cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(a.x + a.width, a.y + 40.0)));
    let empty = point(px(900.0), px(900.0));
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(source.x + px(30.0), source.y), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_move(empty, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(empty, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(ended.get());
}

#[gpui::test]
fn scrolling_zooms(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let zoom = cx.read(|cx| flow.read(cx).viewport().zoom);
    cx.simulate_event(gpui::ScrollEvent {
        position: point(px(600.0), px(400.0)),
        delta: ScrollDelta::Lines(point(0.0, -2.0)),
        modifiers: Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert_ne!(cx.read(|cx| flow.read(cx).viewport().zoom), zoom);
}

#[gpui::test]
fn delete_key_reports_the_selection(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, true);
    let before = guard::recovered();
    let deleted = record(cx, &flow, |event| matches!(event, FlowEvent::Delete { .. }));
    let on_node = screen_of(&flow, cx, "a", Xy::new(80.0, 40.0));
    cx.simulate_click(on_node, Modifiers::default());
    cx.simulate_keystrokes("delete");
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(deleted.get());
}

fn position_of(flow: &Entity<Flow>, cx: &mut VisualTestContext, id: &'static str) -> Xy {
    let id: gpui_flow::Id = id.into();
    cx.read(|cx| flow.read(cx).nodes().iter().find(|n| n.id == id).map(|n| n.position)).expect("node exists")
}

#[gpui::test]
fn a_grab_follows_the_pointer_until_a_click(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let start = position_of(&flow, cx, "a");
    let at = screen_of(&flow, cx, "a", Xy::new(80.0, 40.0));
    cx.simulate_mouse_move(at, None, Modifiers::default());
    assert!(cx.update(|_, cx| flow.update(cx, |f, cx| f.begin_grab(&["a".into()], cx))));
    let to = point(at.x + px(100.0), at.y + px(50.0));
    cx.simulate_mouse_move(to, None, Modifiers::default());
    cx.simulate_mouse_down(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(!cx.read(|cx| flow.read(cx).is_grabbing()));
    let end = position_of(&flow, cx, "a");
    assert!(end.x > start.x && end.y > start.y);
}

#[gpui::test]
fn escape_cancels_a_grab(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let start = position_of(&flow, cx, "a");
    let at = screen_of(&flow, cx, "a", Xy::new(80.0, 40.0));
    cx.simulate_mouse_move(at, None, Modifiers::default());
    cx.simulate_click(at, Modifiers::default());
    assert!(cx.update(|_, cx| flow.update(cx, |f, cx| f.begin_grab(&["a".into()], cx))));
    cx.simulate_mouse_move(point(at.x + px(100.0), at.y), None, Modifiers::default());
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(!cx.read(|cx| flow.read(cx).is_grabbing()));
    assert_eq!(position_of(&flow, cx, "a"), start);
}

#[gpui::test]
fn clicking_two_handles_connects_them(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let connected = record(cx, &flow, |event| matches!(event, FlowEvent::Connect(_)));
    let a = cx.read(|cx| flow.read(cx).node_rect(&"a".into())).expect("measured");
    let b = cx.read(|cx| flow.read(cx).node_rect(&"b".into())).expect("measured");
    let source = cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(a.x + a.width, a.y + 40.0)));
    let target = cx.read(|cx| flow.read(cx).flow_to_screen(Xy::new(b.x, b.y + 40.0)));
    cx.simulate_click(source, Modifiers::default());
    assert!(!connected.get());
    cx.simulate_mouse_move(point(target.x + px(40.0), target.y), None, Modifiers::default());
    cx.simulate_mouse_move(target, None, Modifiers::default());
    cx.simulate_click(target, Modifiers::default());
    settle(cx);
    assert_eq!(guard::recovered(), before);
    assert!(connected.get(), "no connection was made");
}

#[gpui::test]
fn arrow_keys_nudge_the_selection(cx: &mut TestAppContext) {
    let (flow, cx) = setup(cx, false);
    let before = guard::recovered();
    let start = position_of(&flow, cx, "a");
    let on_node = screen_of(&flow, cx, "a", Xy::new(80.0, 40.0));
    cx.simulate_click(on_node, Modifiers::default());
    cx.simulate_keystrokes("right");
    cx.simulate_keystrokes("shift-down");
    settle(cx);
    assert_eq!(guard::recovered(), before);
    let end = position_of(&flow, cx, "a");
    assert_eq!((end.x - start.x, end.y - start.y), (5.0, 20.0));
}
