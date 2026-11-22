//! Floating surfaces drawn at a window position: gpui-component's popup menus, cascading menus
//! whose rows open submenus beside them, and panels such as a colour picker. gpui-component's
//! `Popover` anchors to a trigger element in the layout, which these don't have, so they are
//! placed here.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, anchored, deferred,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ThemeStyled as _,
    menu::{PopupMenu, PopupMenuItem},
};

use crate::{ActiveTheme as _, Icon, IconName};

/// The layer gpui-component draws its popups on, so the submenus a menu nests stay above it.
const LAYER: usize = 100;

/// How long the pointer must rest on a row before its submenu opens or closes. Crossing other
/// rows on the way into a submenu then leaves the open one alone.
const HOVER_INTENT: Duration = Duration::from_millis(150);

/// Submenus draw above the layer of the menu they open from, one step per level.
const SUBMENU_LAYER: usize = LAYER + 2;

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

/// What a row does when picked.
pub type Pick = Rc<dyn Fn(&mut Window, &mut App)>;

pub enum Action {
    Pick(Pick),
    Submenu(Vec<Row>),
}

/// A small button at the end of a row. Pressing it runs `run` and closes the menu; the row is not picked.
pub struct Trailing {
    pub label: SharedString,
    pub run: Pick,
}

pub struct Row {
    pub label: SharedString,
    /// Text on the right, muted: a shortcut, or what the row holds.
    pub detail: Option<SharedString>,
    /// `Some` when the row is a choice: whether it is the one chosen.
    pub checked: Option<bool>,
    pub disabled: bool,
    pub destructive: bool,
    /// A separator goes above the row.
    pub separated: bool,
    pub action: Action,
    pub trailing: Option<Trailing>,
}

impl Row {
    pub fn pick(label: impl Into<SharedString>, run: Pick) -> Self {
        Self::new(label, Action::Pick(run))
    }

    pub fn submenu(label: impl Into<SharedString>, rows: Vec<Row>) -> Self {
        Self::new(label, Action::Submenu(rows))
    }

    fn new(label: impl Into<SharedString>, action: Action) -> Self {
        Self {
            label: label.into(),
            detail: None,
            checked: None,
            disabled: false,
            destructive: false,
            separated: false,
            action,
            trailing: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
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

    pub fn trailing(mut self, label: impl Into<SharedString>, run: Pick) -> Self {
        self.trailing = Some(Trailing { label: label.into(), run });
        self
    }
}

/// A menu whose rows open submenus beside them. A submenu opens on hover once the pointer has
/// rested on its row for `HOVER_INTENT`, on click, or with the keys: right opens, left closes one
/// level, enter picks, escape closes one level. Picking a row or escaping the top level emits
/// `DismissEvent`.
///
/// The root rows are drawn by `column`, so the owner can put them under a search field and
/// `layer` positions it.
pub struct Cascade {
    rows: Vec<Row>,
    width: Pixels,
    /// The row of each open level whose submenu is open at the next level.
    path: Vec<usize>,
    /// The keyboard-highlighted row of each open level; one more entry than `path`.
    highlight: Vec<Option<usize>>,
    /// A change of open submenu waiting out `HOVER_INTENT`.
    intent: Option<Intent>,
    focus: FocusHandle,
}

struct Intent {
    level: usize,
    _apply: Task<()>,
}

impl EventEmitter<DismissEvent> for Cascade {}

impl Focusable for Cascade {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The next enabled row from `from` in the direction, wrapping around.
fn step(rows: &[Row], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = rows.len();
    if n == 0 {
        return None;
    }
    let start = from.unwrap_or(if forward { n - 1 } else { 0 });
    (1..=n)
        .map(|k| if forward { (start + k) % n } else { (start + n - k % n) % n })
        .find(|&i| rows.get(i).is_some_and(|r| !r.disabled))
}

impl Cascade {
    /// `path` opens submenus from the start: each entry is the row of the level it opens from.
    pub fn new(rows: Vec<Row>, width: Pixels, path: Vec<usize>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            rows,
            width,
            path: Vec::new(),
            highlight: vec![None],
            intent: None,
            focus: cx.focus_handle(),
        };
        for row in path {
            let level = this.path.len();
            if !this.is_submenu(level, row) {
                break;
            }
            this.open(level, row);
        }
        this
    }

    /// The rows of each open submenu, outermost first, as indices into the rows of their level.
    pub fn open_path(&self) -> &[usize] {
        &self.path
    }

    fn rows_at(&self, level: usize) -> &[Row] {
        let mut rows = self.rows.as_slice();
        for &row in self.path.iter().take(level) {
            match rows.get(row).map(|r| &r.action) {
                Some(Action::Submenu(sub)) => rows = sub,
                _ => return &[],
            }
        }
        rows
    }

    fn is_submenu(&self, level: usize, row: usize) -> bool {
        matches!(self.rows_at(level).get(row).map(|r| &r.action), Some(Action::Submenu(_)))
    }

    fn highlighted(&self, level: usize) -> Option<usize> {
        self.highlight.get(level).copied().flatten()
    }

    fn set_highlight(&mut self, level: usize, row: Option<usize>) {
        if let Some(slot) = self.highlight.get_mut(level) {
            *slot = row;
        }
    }

    /// Opens the submenu of `row` at `level`, closing any deeper one, and highlights its first row.
    fn open(&mut self, level: usize, row: usize) {
        self.set_highlight(level, Some(row));
        if self.path.get(level) == Some(&row) {
            return;
        }
        self.path.truncate(level);
        self.path.push(row);
        self.highlight.truncate(level + 1);
        self.highlight.resize(level + 2, None);
        let first = step(self.rows_at(level + 1), None, true);
        self.set_highlight(level + 1, first);
    }

    /// Points at `row` of `level`: its submenu opens, or a submenu open beside the level closes.
    fn select(&mut self, level: usize, row: usize) {
        if level > self.path.len() {
            return;
        }
        if self.is_submenu(level, row) {
            self.open(level, row);
        } else {
            self.path.truncate(level);
            self.highlight.truncate(level + 1);
            self.set_highlight(level, Some(row));
        }
    }

    fn hover(&mut self, level: usize, row: usize, cx: &mut Context<Self>) {
        if self.rows_at(level).get(row).is_none_or(|r| r.disabled) {
            return;
        }
        // The pointer has moved into a deeper panel, so a switch pending above it is moot.
        if self.intent.as_ref().is_some_and(|i| i.level < level) {
            self.intent = None;
        }
        self.set_highlight(level, Some(row));
        if self.path.get(level) == Some(&row) {
            self.intent = None;
            return;
        }
        let apply = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_INTENT).await;
            this.update(cx, |this, cx| {
                this.intent = None;
                this.select(level, row);
                cx.notify();
            })
            .ok();
        });
        self.intent = Some(Intent { level, _apply: apply });
    }

    fn click(&mut self, level: usize, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.intent = None;
        self.activate(level, row, window, cx);
    }

    /// Runs the row's pick and dismisses, or opens its submenu.
    fn activate(&mut self, level: usize, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.rows_at(level).get(row) else { return };
        if r.disabled {
            return;
        }
        let pick = match &r.action {
            Action::Pick(pick) => Some(pick.clone()),
            Action::Submenu(_) => None,
        };
        match pick {
            Some(pick) => {
                self.intent = None;
                pick(window, cx);
                cx.emit(DismissEvent);
            }
            None => self.select(level, row),
        }
    }

    /// Handles a key press; returns whether the menu uses it.
    pub fn key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let level = self.path.len();
        self.intent = None;
        match key {
            "down" | "up" => {
                let next = step(self.rows_at(level), self.highlighted(level), key == "down");
                self.set_highlight(level, next);
            }
            "right" => {
                if let Some(row) = self.highlighted(level).filter(|&row| self.is_submenu(level, row)) {
                    self.open(level, row);
                }
            }
            "left" => {
                if self.path.pop().is_some() {
                    self.highlight.pop();
                }
            }
            "enter" => {
                if let Some(row) = self.highlighted(level) {
                    self.activate(level, row, window, cx);
                }
            }
            "escape" => {
                if self.path.is_empty() {
                    cx.emit(DismissEvent);
                } else {
                    self.path.pop();
                    self.highlight.pop();
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn rows_column(&self, level: usize, rows: &[Row], cx: &mut Context<Self>) -> AnyElement {
        let checks = rows.iter().any(|r| r.checked.is_some());
        div()
            .flex()
            .flex_col()
            .w(self.width)
            .children(rows.iter().enumerate().map(|(index, row)| self.row(level, index, row, checks, cx)))
            .into_any_element()
    }

    fn submenu_panel(&self, level: usize, rows: &[Row], cx: &mut Context<Self>) -> AnyElement {
        deferred(anchored().snap_to_window_with_margin(px(8.0)).child(
            div()
                .occlude()
                .top(px(-4.0))
                .left(self.width - px(4.0))
                .popover_style(cx)
                .p_1()
                .child(self.rows_column(level, rows, cx)),
        ))
        .with_priority(SUBMENU_LAYER + level)
        .into_any_element()
    }

    fn row(&self, level: usize, index: usize, row: &Row, checks: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let open = self.path.get(level) == Some(&index);
        let selected = open || self.highlighted(level) == Some(index);
        let (accent, accent_fg, muted) = (c.accent, c.accent_foreground, c.muted_foreground);
        let destructive = c.destructive;
        let text = if row.destructive { destructive } else { c.popover_foreground };
        let disabled = row.disabled;

        let check = checks.then(|| {
            div()
                .flex_none()
                .w(px(12.0))
                .when(row.checked == Some(true), |d| d.child(Icon::new(IconName::Check).size(px(12.0))))
        });
        let caret = matches!(row.action, Action::Submenu(_)).then(|| {
            div().flex_none().text_color(muted).child(Icon::new(IconName::CaretRight).size(px(12.0)))
        });
        let detail = row.detail.clone().map(|d| div().flex_none().text_color(muted).child(d));
        let trailing = row.trailing.as_ref().map(|t| {
            let run = t.run.clone();
            div()
                .id(SharedString::from(format!("cascade-{level}-{index}-trailing")))
                .flex_none()
                .px_1()
                .rounded(theme.radius.sm)
                .text_size(theme.text.xs)
                .text_color(destructive)
                .hover(move |s| s.bg(destructive.opacity(0.15)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.intent = None;
                    run(window, cx);
                    cx.emit(DismissEvent);
                }))
                .child(t.label.clone())
        });
        let submenu = match (&row.action, open) {
            (Action::Submenu(rows), true) => Some(self.submenu_panel(level + 1, rows, cx)),
            _ => None,
        };
        let label = row.label.to_string();

        div()
            .id(SharedString::from(format!("cascade-{level}-{index}")))
            .debug_selector(move || format!("menu-row {label}"))
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .w_full()
            .px_1p5()
            .py_1()
            .rounded(theme.radius.md)
            .text_size(theme.text.sm)
            .text_color(if selected { accent_fg } else { text })
            .when(selected, |d| d.bg(accent))
            .when(row.separated, |d| d.mt_1().border_t_1().border_color(c.border))
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| d.cursor_pointer())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover(level, index, cx);
                }
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.click(level, index, window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .children(check)
                    .child(div().truncate().child(row.label.clone())),
            )
            .children(detail)
            .children(caret)
            .children(trailing)
            .children(submenu)
            .into_any_element()
    }
}

impl Render for Cascade {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("cascading menu", || {
            div()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.key(&event.keystroke.key, window, cx);
                }))
                .child(self.rows_column(0, &self.rows, cx))
        })
    }
}

/// Draws `content` with its top-left corner at `position`, above the window. A press outside it,
/// with either button, calls `on_dismiss`. The submenus of a cascading menu in `content` sit
/// outside its bounds, so they are not outside the content.
pub fn layer(
    window: &Window,
    cx: &App,
    position: Point<Pixels>,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    content: impl IntoElement,
) -> impl IntoElement {
    let size = window.bounds().size;
    let dismiss = Rc::new(on_dismiss);
    let (left, right) = (dismiss.clone(), dismiss);
    div()
        .child(
            deferred(anchored().child(
                div()
                    .w(size.width)
                    .h(size.height)
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| left(window, cx))
                    .on_mouse_down(MouseButton::Right, move |_: &MouseDownEvent, window, cx| right(window, cx)),
            ))
            .with_priority(LAYER),
        )
        .child(
            deferred(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(px(8.0))
                    .child(div().occlude().popover_style(cx).child(content)),
            )
            .with_priority(LAYER + 1),
        )
}

#[cfg(test)]
mod cascade_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{AppContext as _, Modifiers, Render, TestAppContext, VisualTestContext, point};

    use super::*;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    fn pick(log: &Log, name: &'static str) -> Pick {
        let log = log.clone();
        Rc::new(move |_, _| log.borrow_mut().push(name))
    }

    fn rows(log: &Log) -> Vec<Row> {
        vec![
            Row::submenu(
                "Math",
                vec![
                    Row::pick("Sine", pick(log, "sine")),
                    Row::submenu(
                        "Helpers",
                        vec![Row::pick("Fold", pick(log, "fold")), Row::pick("Wrap", pick(log, "wrap")).disabled(true)],
                    ),
                ],
            ),
            Row::submenu("Vector", vec![Row::pick("Dot", pick(log, "dot"))]),
            Row::pick("Reset", pick(log, "reset")).separated(),
        ]
    }

    struct Host(Entity<Cascade>);

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(layer(window, cx, point(px(10.0), px(10.0)), |_, _| {}, self.0.clone()))
        }
    }

    fn frame(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    /// Lets the hover intent elapse and draws the result.
    fn settle(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(HOVER_INTENT * 2);
        frame(cx);
    }

    fn hover(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"));
        cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<Cascade>, Log, &mut VisualTestContext) {
        cx.update(crate::init);
        let log: Log = Rc::default();
        let rows = rows(&log);
        let menu = cx.update(|cx| cx.new(|cx| Cascade::new(rows, px(200.0), Vec::new(), cx)));
        let dismissed = log.clone();
        cx.update(|cx| {
            cx.subscribe(&menu, move |_, _: &DismissEvent, _| dismissed.borrow_mut().push("dismissed")).detach();
        });
        let host = menu.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host(host));
        let focus = cx.read(|cx| menu.read(cx).focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        frame(cx);
        (menu, log, cx)
    }

    fn path(menu: &Entity<Cascade>, cx: &mut VisualTestContext) -> Vec<usize> {
        cx.read(|cx| menu.read(cx).path.clone())
    }

    #[gpui::test]
    fn hovering_a_category_opens_its_submenu_and_keeps_the_parent(cx: &mut TestAppContext) {
        let (menu, _, cx) = setup(cx);
        hover(cx, "menu-row Math");
        settle(cx);
        assert_eq!(path(&menu, cx), [0]);
        assert!(cx.debug_bounds("menu-row Sine").is_some(), "the submenu is drawn");
        assert!(cx.debug_bounds("menu-row Vector").is_some(), "the parent's other rows stay");
    }

    #[gpui::test]
    fn sub_categories_open_beside_their_parent_submenu(cx: &mut TestAppContext) {
        let (menu, _, cx) = setup(cx);
        hover(cx, "menu-row Math");
        settle(cx);
        hover(cx, "menu-row Helpers");
        settle(cx);
        assert_eq!(path(&menu, cx), [0, 1]);
        assert!(cx.debug_bounds("menu-row Fold").is_some());
        assert!(cx.debug_bounds("menu-row Sine").is_some(), "the parent submenu stays");
    }

    #[gpui::test]
    fn crossing_a_row_within_the_intent_delay_keeps_the_open_submenu(cx: &mut TestAppContext) {
        let (menu, _, cx) = setup(cx);
        hover(cx, "menu-row Math");
        settle(cx);
        hover(cx, "menu-row Vector");
        cx.executor().advance_clock(HOVER_INTENT / 3);
        frame(cx);
        hover(cx, "menu-row Sine");
        settle(cx);
        assert_eq!(path(&menu, cx), [0]);
        assert!(cx.debug_bounds("menu-row Dot").is_none(), "Vector's submenu never opened");
    }

    #[gpui::test]
    fn the_keys_move_open_and_close_submenus_and_pick(cx: &mut TestAppContext) {
        let (menu, log, cx) = setup(cx);
        cx.simulate_keystrokes("down right");
        frame(cx);
        assert_eq!(path(&menu, cx), [0]);
        cx.simulate_keystrokes("down right");
        frame(cx);
        assert_eq!(path(&menu, cx), [0, 1]);
        cx.simulate_keystrokes("down");
        frame(cx);
        assert_eq!(cx.read(|cx| menu.read(cx).highlight.clone()), [Some(0), Some(1), Some(0)], "disabled Wrap is skipped");
        cx.simulate_keystrokes("left left");
        frame(cx);
        assert_eq!(path(&menu, cx), Vec::<usize>::new());

        cx.simulate_keystrokes("down enter");
        frame(cx);
        assert_eq!(path(&menu, cx), [1]);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["dot", "dismissed"]);
    }

    #[gpui::test]
    fn escape_closes_one_level_then_dismisses(cx: &mut TestAppContext) {
        let (menu, log, cx) = setup(cx);
        cx.simulate_keystrokes("down enter");
        frame(cx);
        assert_eq!(path(&menu, cx), [0]);
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert_eq!(path(&menu, cx), Vec::<usize>::new());
        assert!(log.borrow().is_empty(), "the first escape only closes the submenu");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["dismissed"]);
    }
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
