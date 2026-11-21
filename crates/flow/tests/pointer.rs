use gpui::{AppContext as _, Modifiers, MouseButton, TestAppContext, point, px};
use gpui_flow::Flow;

/// Pointer handlers run outside paint, where some window APIs panic.
#[gpui::test]
fn dragging_on_the_empty_pane_does_not_panic(cx: &mut TestAppContext) {
    let (_flow, cx) = cx.add_window_view(|_, cx| Flow::new(cx));
    let before = guard::recovered();
    let start = point(px(200.0), px(200.0));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(px(240.0), px(230.0)), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(point(px(240.0), px(230.0)), MouseButton::Left, Modifiers::default());
    cx.simulate_click(start, Modifiers::default());
    assert_eq!(guard::recovered(), before);
}
