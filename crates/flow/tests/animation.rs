use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext,
    Window, div,
};
use gpui_flow::{FitViewOptions, Flow, HandleType, Node, NodeProps, Position, Viewport, Xy};

fn card_a(props: &NodeProps, _: &mut Window, _: &mut App) -> AnyElement {
    div()
        .relative()
        .debug_selector(|| "node-a".into())
        .w(props.scaled(160.0))
        .h(props.scaled(80.0))
        .bg(props.theme.node_background)
        .child(props.handle(Some("in".into()), HandleType::Target, Position::Left))
        .child(props.handle(Some("out".into()), HandleType::Source, Position::Right))
        .into_any_element()
}

fn card(props: &NodeProps, _: &mut Window, _: &mut App) -> AnyElement {
    div()
        .relative()
        .debug_selector(|| "node-b".into())
        .w(props.scaled(160.0))
        .h(props.scaled(80.0))
        .bg(props.theme.node_background)
        .child(props.handle(Some("in".into()), HandleType::Target, Position::Left))
        .child(props.handle(Some("out".into()), HandleType::Source, Position::Right))
        .into_any_element()
}

/// The flow sits inside a cached view, as it does in the app, so only a
/// notification of the flow itself reaches it.
struct Inner {
    flow: Entity<Flow>,
}

impl Render for Inner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.flow.clone())
    }
}

struct Host {
    flow: Entity<Flow>,
    inner: Entity<Inner>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.inner
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

/// One frame: the frame callbacks run, then the window is drawn if any ran.
fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        if window.simulate_next_frame(cx) > 0 {
            let _ = window.draw(cx);
        }
    });
}

/// The platform draws a window that input or code dirtied before the next frame.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..6 {
        cx.run_until_parked();
        frame(cx);
    }
}

fn close(a: Viewport, b: Viewport) -> bool {
    (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01 && (a.zoom - b.zoom).abs() < 0.0001
}

#[gpui::test]
fn fit_animation_advances_without_input(cx: &mut TestAppContext) {
    let flow = cx.update(|cx| {
        cx.new(|cx| {
            let mut flow = Flow::new(cx);
            flow.options.fit_view_on_init = false;
            flow.set_nodes(
                vec![
                    Node::new("a", Xy::new(0.0, 0.0)).render(card_a),
                    Node::new("b", Xy::new(900.0, 400.0)).render(card),
                ],
                cx,
            );
            flow
        })
    });
    let inner = cx.update(|cx| cx.new(|_| Inner { flow: flow.clone() }));
    let (host, cx) = cx.add_window_view(|_, _| Host { flow, inner });
    let flow = host.read_with(cx, |host, _| host.flow.clone());
    settle(cx);

    flow.update(cx, |f, cx| f.fit_view(FitViewOptions::default(), cx));
    settle(cx);
    let target = flow.read_with(cx, |f, _| f.viewport());
    assert!(
        !close(
            target,
            Viewport {
                x: 0.0,
                y: 0.0,
                zoom: 1.0
            }
        ),
        "the fit must move the viewport"
    );

    flow.update(cx, |f, cx| {
        f.set_viewport(
            Viewport {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            None,
            cx,
        )
    });
    settle(cx);

    flow.update(cx, |f, cx| {
        f.fit_view(
            FitViewOptions {
                duration: Some(Duration::from_millis(200)),
                ..Default::default()
            },
            cx,
        )
    });
    cx.run_until_parked();
    draw(cx);

    let mid = flow.read_with(cx, |f, _| f.viewport());
    assert!(
        !close(mid, target),
        "the animation should still be running after its first frame"
    );

    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(10));
        frame(cx);
    }

    let end = flow.read_with(cx, |f, _| f.viewport());
    let painted = cx.debug_bounds("node-a").expect("node a painted");
    assert!(
        (f32::from(painted.origin.x) - end.x).abs() < 0.5,
        "node a is painted at {:?} but the viewport is at {end:?}",
        painted.origin.x
    );
    assert!(
        close(end, target),
        "animation stalled at {end:?}, expected {target:?}"
    );
}
