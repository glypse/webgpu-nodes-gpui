//! Floating surfaces drawn at a window position: gpui-component's popup menus, and panels such as
//! a colour picker. gpui-component's `Popover` anchors to a trigger element in the layout, which
//! these don't have, so they are placed here.

use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, SharedString, Styled as _, Subscription,
    Window, anchored, deferred, div, px,
};
use gpui_component::{
    ThemeStyled as _,
    menu::{PopupMenu, PopupMenuItem},
};

use crate::ActiveTheme as _;

/// The layer gpui-component draws its popups on, so the submenus a menu nests stay above it.
const LAYER: usize = 100;

/// A menu entry with `detail` (a shortcut, or the value it holds) in muted text on the right.
pub fn row(label: impl Into<SharedString>, detail: Option<SharedString>, destructive: bool) -> PopupMenuItem {
    let label = label.into();
    PopupMenuItem::element(move |_, cx| {
        let colors = cx.theme().colors.clone();
        let text = if destructive { colors.destructive } else { colors.popover_foreground };
        let detail = detail.clone().map(|d| div().flex_none().text_color(colors.muted_foreground).child(d));
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .w_full()
            .text_color(text)
            .child(label.clone())
            .children(detail)
    })
}

/// Adapts an action that doesn't use the click to a menu item's `on_click`.
pub fn on_select(f: impl Fn(&mut Window, &mut App) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| f(window, cx)
}

/// An open popup menu. Dropping it drops its dismissal subscription, so the owner keeps it while
/// the menu is shown.
pub struct Open {
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// Opens `menu` with its keys focused. `on_dismiss` runs when it closes, after a pick or a press
/// outside it; the owner should drop the `Open` then.
pub fn open<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    menu: Entity<PopupMenu>,
    on_dismiss: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Open {
    let handle = menu.read(cx).focus_handle(cx);
    window.defer(cx, move |window, cx| window.focus(&handle, cx));
    let dismissed = cx.subscribe_in(&menu, window, move |this, _, _: &DismissEvent, _, cx| {
        on_dismiss(this, cx);
    });
    Open { menu, _dismissed: dismissed }
}

impl Open {
    /// Draws the menu with its top-left corner at `position`.
    pub fn draw(&self, window: &Window, position: Point<Pixels>) -> impl IntoElement {
        let size = window.bounds().size;
        deferred(anchored().child(
            div()
                .w(size.width)
                .h(size.height)
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(anchored().position(position).snap_to_window_with_margin(px(8.0)).child(self.menu.clone())),
        ))
        .with_priority(LAYER)
    }
}

/// Draws `child` as a panel with its top-left corner at `position`, calling `on_dismiss` on a press
/// outside it.
pub fn panel(
    cx: &App,
    position: Point<Pixels>,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    child: impl IntoElement,
) -> impl IntoElement {
    deferred(anchored().position(position).snap_to_window_with_margin(px(8.0)).child(
        div()
            .occlude()
            .popover_style(cx)
            .on_mouse_down_out(move |_, window, cx| on_dismiss(window, cx))
            .child(child),
    ))
    .with_priority(LAYER)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Context, Render, TestAppContext, VisualTestContext, point};

    use super::*;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    struct Host {
        open: Option<Open>,
    }

    impl Host {
        fn new(log: &Log, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let (first, second) = (log.clone(), log.clone());
            let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                menu.item(
                    PopupMenuItem::new("One")
                        .on_click(on_select(move |_, _| first.borrow_mut().push("one"))),
                )
                .item(PopupMenuItem::new("Off").disabled(true))
                .item(
                    PopupMenuItem::new("Two")
                        .on_click(on_select(move |_, _| second.borrow_mut().push("two"))),
                )
            });
            let dismissed = log.clone();
            let open = open(window, cx, menu, move |this: &mut Self, cx| {
                this.open = None;
                dismissed.borrow_mut().push("dismissed");
                cx.notify();
            });
            Self { open: Some(open) }
        }
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .children(self.open.as_ref().map(|open| open.draw(window, point(px(10.0), px(10.0)))))
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Log, &mut VisualTestContext) {
        cx.update(crate::init);
        let log: Log = Rc::default();
        let shared = log.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| Host::new(&shared, window, cx));
        cx.run_until_parked();
        cx.run_until_parked();
        (log, cx)
    }

    #[gpui::test]
    fn the_arrows_skip_disabled_entries_and_enter_picks(cx: &mut TestAppContext) {
        let (log, cx) = setup(cx);
        cx.simulate_keystrokes("down down enter");
        assert_eq!(*log.borrow(), ["two", "dismissed"]);
    }

    #[gpui::test]
    fn escape_dismisses_without_picking(cx: &mut TestAppContext) {
        let (log, cx) = setup(cx);
        cx.simulate_keystrokes("down escape");
        assert_eq!(*log.borrow(), ["dismissed"]);
    }
}
