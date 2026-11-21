use guard::SafeListener as _;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, ClickEvent, Context, CursorStyle,
    DragMoveEvent, Empty, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, StatefulInteractiveElement as _, Styled as _, StyleRefinement, Window,
    canvas, div, point, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{Sizable as _, Selectable as _, button::{Button, ButtonVariants as _}};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use theme::{ActiveTheme as _, IconName, popup};

use crate::gestures::{self, Outcome, Pixels as ScreenPixels};
use crate::{Area, AreaId, Axis, EdgeSelection, MinSize, Part, Screen};

/// The kinds of panel an area can show.
pub trait Editor: Clone + PartialEq + 'static {
    fn all() -> &'static [Self];
    fn label(&self) -> &'static str;
    fn icon(&self) -> IconName;
}

/// What an area shows: its panel's controls, drawn in the header after the
/// type picker, and its body. Both are cached views, drawn again only when
/// their entity is notified, so each must stay correct at the size the area
/// gives it.
pub struct AreaContent {
    pub toolbar: Option<AnyView>,
    pub body: Option<AnyView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenEvent {
    /// The areas, their sizes or their editors changed.
    Changed,
}

/// Smallest area width, in pixels.
const MIN_AREA_WIDTH: f32 = 96.0;
/// How far from an edge the pointer grabs it.
const EDGE_GRAB: f32 = 4.0;
/// An area can be shrunk to its header, so its minimum height is the header's.
pub const HEADER_HEIGHT: f32 = 36.0;
const CORNER_SIZE: f32 = 12.0;

/// Lays out an area's toolbar across the header row's free width. The slot is a flex box
/// so the toolbar fills it, and the toolbar is a cached view: one with live readouts then
/// re-renders on its own rather than re-rendering the whole screen. A cached view is laid
/// out as a root, so the toolbar fills the cached box itself.
pub fn toolbar_slot(toolbar: AnyView) -> impl IntoElement {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .items_center()
        .child(toolbar.cached(StyleRefinement::default().size_full()))
}

#[derive(Clone, Copy)]
struct EdgeDrag;

#[derive(Clone, Copy)]
enum MenuKind {
    Type,
    Area,
}

struct OpenMenu {
    area: AreaId,
    kind: MenuKind,
    at: Point<Pixels>,
    popup: Option<popup::Open>,
}

/// A drag from an area's corner.
struct CornerDrag {
    source: AreaId,
    start: gestures::Point,
    swap: bool,
    outcome: Option<Outcome>,
}

/// A split following the pointer until a click puts it down.
struct Splitting {
    axis: Axis,
    area: Option<AreaId>,
    at: f64,
}

/// The options of the edge between two areas, as Blender's right-click on an edge offers.
struct EdgeMenu<E> {
    at: Point<Pixels>,
    axis: Axis,
    before: AreaId,
    after: AreaId,
    /// The screen once the area before the edge grew over the one after it, if it can.
    forward: Option<Screen<E>>,
    backward: Option<Screen<E>>,
    popup: Option<popup::Open>,
}

type RenderArea<E> = Box<dyn Fn(AreaId, &E, &mut Window, &mut App) -> AreaContent>;

/// Renders a `Screen`: its areas with a header each, and draggable dividers
/// between them.
pub struct ScreenView<E: Editor> {
    screen: Screen<E>,
    render_area: RenderArea<E>,
    bounds: Bounds<Pixels>,
    maximized: Option<AreaId>,
    /// The edge being dragged, and the screen as it was when the drag began.
    drag: Option<(EdgeSelection, Screen<E>)>,
    menu: Option<OpenMenu>,
    edge_menu: Option<EdgeMenu<E>>,
    corner: Option<CornerDrag>,
    splitting: Option<Splitting>,
    hovered: Option<AreaId>,
    /// Where the pointer last was, in pixels from the screen's corner.
    pointer: gestures::Point,
    /// Held by the gesture overlay, so Escape reaches it.
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
}

impl<E: Editor> EventEmitter<ScreenEvent> for ScreenView<E> {}

impl<E: Editor> ScreenView<E> {
    pub fn new(
        screen: Screen<E>,
        render_area: impl Fn(AreaId, &E, &mut Window, &mut App) -> AreaContent + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            screen,
            render_area: Box::new(render_area),
            bounds: Bounds::default(),
            maximized: None,
            drag: None,
            menu: None,
            edge_menu: None,
            corner: None,
            splitting: None,
            hovered: None,
            pointer: (0.0, 0.0),
            focus: cx.focus_handle(),
            previous_focus: None,
        }
    }

    pub fn screen(&self) -> &Screen<E> {
        &self.screen
    }

    pub fn set_screen(&mut self, screen: Screen<E>, cx: &mut Context<Self>) {
        if self.maximized.is_some_and(|id| screen.area(id).is_none()) {
            self.maximized = None;
        }
        self.screen = screen;
        cx.emit(ScreenEvent::Changed);
        cx.notify();
    }

    pub fn maximized(&self) -> Option<AreaId> {
        self.maximized
    }

    pub fn toggle_maximized(&mut self, area: AreaId, cx: &mut Context<Self>) {
        self.maximized = if self.maximized == Some(area) {
            None
        } else {
            Some(area)
        };
        cx.notify();
    }

    fn min_size(&self) -> MinSize {
        let (width, height) = (
            f64::from(self.bounds.size.width),
            f64::from(self.bounds.size.height),
        );
        MinSize {
            width: f64::from(MIN_AREA_WIDTH) / width.max(1.0),
            height: f64::from(HEADER_HEIGHT) / height.max(1.0),
        }
    }

    fn grab_size(&self) -> MinSize {
        let (width, height) = (
            f64::from(self.bounds.size.width),
            f64::from(self.bounds.size.height),
        );
        MinSize {
            width: f64::from(EDGE_GRAB) / width.max(1.0),
            height: f64::from(EDGE_GRAB) / height.max(1.0),
        }
    }

    /// A window position as fractions of the screen.
    fn fractions(&self, position: Point<Pixels>) -> (f64, f64) {
        let local = position - self.bounds.origin;
        (
            f64::from(local.x) / f64::from(self.bounds.size.width).max(1.0),
            f64::from(local.y) / f64::from(self.bounds.size.height).max(1.0),
        )
    }

    fn split(&mut self, id: AreaId, axis: Axis, cx: &mut Context<Self>) {
        let Some(area) = self.screen.area(id) else {
            return;
        };
        let at = match axis {
            Axis::X => (area.rect.left + area.rect.right) / 2.0,
            Axis::Y => (area.rect.top + area.rect.bottom) / 2.0,
        };
        if let Some((screen, _)) = self
            .screen
            .split(id, axis, at, Part::After, self.min_size())
        {
            self.set_screen(screen, cx);
        }
    }

    fn close(&mut self, id: AreaId, cx: &mut Context<Self>) {
        if let Some(screen) = self.screen.close(id, self.min_size()) {
            self.set_screen(screen, cx);
        }
    }

    fn pixels(&self) -> ScreenPixels {
        ScreenPixels {
            width: f64::from(self.bounds.size.width).max(1.0),
            height: f64::from(self.bounds.size.height).max(1.0),
        }
    }

    /// A window position in pixels from the screen's top-left corner.
    fn local(&self, position: Point<Pixels>) -> gestures::Point {
        let local = position - self.bounds.origin;
        (f64::from(local.x), f64::from(local.y))
    }

    fn gesture_active(&self) -> bool {
        self.corner.is_some() || self.splitting.is_some()
    }

    /// Takes the keyboard for the gesture, so Escape cancels it.
    fn begin_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previous_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn end_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.corner = None;
        self.splitting = None;
        if let Some(previous) = self.previous_focus.take() {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn start_corner_drag(&mut self, area: AreaId, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.maximized.is_some() || self.gesture_active() {
            return;
        }
        self.corner = Some(CornerDrag {
            source: area,
            start: self.local(event.position),
            swap: event.modifiers.control,
            outcome: None,
        });
        self.begin_gesture(window, cx);
    }

    fn update_split(&mut self) {
        let Some(axis) = self.splitting.as_ref().map(|s| s.axis) else { return };
        let (area, at) =
            match gestures::split_at(&self.screen, axis, self.pointer, self.pixels(), self.min_size()) {
                Some((area, at)) => (Some(area), at),
                None => (None, 0.0),
            };
        self.splitting = Some(Splitting { axis, area, at });
    }

    fn gesture_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.pointer = self.local(event.position);
        if self.corner.is_some() && event.pressed_button != Some(MouseButton::Left) {
            // The button was let go where we couldn't see it.
            self.finish_corner(window, cx);
            return;
        }
        if let Some(corner) = &self.corner {
            let outcome = self.screen.area(corner.source).and_then(|source| {
                gestures::corner_outcome(
                    &self.screen,
                    source,
                    corner.start,
                    self.pointer,
                    corner.swap,
                    self.pixels(),
                    self.min_size(),
                )
            });
            if let Some(corner) = &mut self.corner {
                corner.outcome = outcome;
            }
        } else {
            self.update_split();
        }
        cx.notify();
    }

    fn finish_corner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drag = self.corner.take();
        self.end_gesture(window, cx);
        let Some(CornerDrag { source, outcome: Some(outcome), .. }) = drag else { return };
        if let Some(screen) = gestures::apply(&self.screen, source, &outcome, self.min_size()) {
            self.set_screen(screen, cx);
        }
    }

    fn finish_split(&mut self, confirm: bool, window: &mut Window, cx: &mut Context<Self>) {
        let split = self.splitting.take();
        self.end_gesture(window, cx);
        let (true, Some(Splitting { axis, area: Some(area), at })) = (confirm, split) else { return };
        if let Some((screen, _)) = self.screen.split(area, axis, at, Part::After, self.min_size()) {
            self.set_screen(screen, cx);
        }
    }

    fn cancel_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gesture_active() {
            self.end_gesture(window, cx);
        }
    }

    fn open_edge_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.maximized.is_some() || self.gesture_active() {
            return;
        }
        let (x, y) = self.fractions(position);
        let grab = self.grab_size();
        let Some(edge) = self.screen.edge_at(x, y, grab, false) else { return };
        let offset = 2.0 * if edge.axis == Axis::X { grab.width } else { grab.height };
        let beside = |delta: f64| match edge.axis {
            Axis::X => self.screen.area_at(edge.position + delta, y),
            Axis::Y => self.screen.area_at(x, edge.position + delta),
        };
        let (Some(before), Some(after)) = (beside(-offset), beside(offset)) else { return };
        if before.id == after.id {
            return;
        }
        let min = self.min_size();
        self.pointer = self.local(position);
        self.edge_menu = Some(EdgeMenu {
            at: position,
            axis: edge.axis,
            before: before.id,
            after: after.id,
            forward: self.screen.join(before.id, after.id, min),
            backward: self.screen.join(after.id, before.id, min),
            popup: None,
        });
        cx.notify();
    }

    fn start_edge_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let (x, y) = self.fractions(event.position);
        self.drag = self
            .screen
            .edge_at(x, y, self.grab_size(), event.modifiers.shift)
            .map(|edge| (edge, self.screen.clone()));
        cx.notify();
    }

    fn drag_edge(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((edge, start)) = &self.drag else {
            return;
        };
        let (x, y) = self.fractions(position);
        let to = if edge.axis == Axis::X { x } else { y };
        let screen = start.move_edge(edge, to, self.min_size());
        if screen != self.screen {
            self.set_screen(screen, cx);
        }
    }

    fn render_header(
        &self,
        area: &Area<E>,
        toolbar: Option<AnyView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = area.id;
        let open_type = move |this: &mut Self, event: &ClickEvent, cx: &mut Context<Self>| {
            this.menu = Some(OpenMenu {
                area: id,
                kind: MenuKind::Type,
                at: event.position() + point(px(0.0), px(4.0)),
                popup: None,
            });
            cx.notify();
        };
        let type_open = self
            .menu
            .as_ref()
            .is_some_and(|m| m.area == id && matches!(m.kind, MenuKind::Type));
        div()
            .id(("area-header", id.0 as usize))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(HEADER_HEIGHT))
            .min_w_0()
            .px(px(6.0))
            .overflow_hidden()
            .bg(theme.colors.background)
            .border_b_1()
            .border_color(theme.colors.border)
            .on_mouse_down(
                MouseButton::Right,
                cx.safe_listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.menu = Some(OpenMenu {
                        area: id,
                        kind: MenuKind::Area,
                        at: event.position,
                        popup: None,
                    });
                    cx.notify();
                }),
            )
            .child(
                Button::new(("area-type", id.0 as usize))
                    .ghost()
                    .xsmall()
                    .selected(type_open)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0p5()
                            .child(theme::Icon::new(area.editor.icon()).size(px(16.0)))
                            .child(theme::Icon::new(IconName::CaretDown).size(px(12.0))),
                    )
                    .tooltip("Editor type")
                    .on_click(cx.safe_listener(move |this, event, _, cx| open_type(this, event, cx))),
            )
            .children(toolbar.map(toolbar_slot))
            .into_any_element()
    }

    fn render_edge_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.edge_menu.as_ref()?;
        if open.popup.is_none() {
            let axis = open.axis;
            let (forward, backward) = (open.forward.is_some(), open.backward.is_some());
            let view = cx.entity();
            let split = |split_axis: Axis| {
                let view = view.clone();
                move |window: &mut Window, cx: &mut App| {
                    guard::guard("split from an edge", || {
                        view.update(cx, |this, cx| {
                            this.splitting = Some(Splitting { axis: split_axis, area: None, at: 0.0 });
                            this.update_split();
                            this.begin_gesture(window, cx);
                        });
                    });
                }
            };
            let join = |forward: bool| {
                let view = view.clone();
                move |_: &mut Window, cx: &mut App| {
                    guard::guard("join from an edge", || {
                        view.update(cx, |this, cx| {
                            let next = this.edge_menu.as_ref().and_then(|m| {
                                if forward { m.forward.clone() } else { m.backward.clone() }
                            });
                            if let Some(screen) = next {
                                this.set_screen(screen, cx);
                            }
                        });
                    });
                }
            };
            let swap = {
                let view = view.clone();
                move |_: &mut Window, cx: &mut App| {
                    guard::guard("swap from an edge", || {
                        view.update(cx, |this, cx| {
                            let next = this
                                .edge_menu
                                .as_ref()
                                .and_then(|m| this.screen.swap(m.before, m.after));
                            if let Some(screen) = next {
                                this.set_screen(screen, cx);
                            }
                        });
                    });
                }
            };
            let (forward_icon, backward_icon, swap_icon, forward_label, backward_label) = match axis {
                Axis::X => (IconName::ArrowLineRight, IconName::ArrowLineLeft, IconName::ArrowsLeftRight, "Join right", "Join left"),
                Axis::Y => (IconName::ArrowLineDown, IconName::ArrowLineUp, IconName::ArrowsDownUp, "Join down", "Join up"),
            };
            let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                menu.min_w(px(208.0))
                    .label("Area edge options")
                    .separator()
                    .item(PopupMenuItem::new("Vertical split").icon(IconName::SquareSplitHorizontal).on_click(popup::on_select(split(Axis::X))))
                    .item(PopupMenuItem::new("Horizontal split").icon(IconName::SquareSplitVertical).on_click(popup::on_select(split(Axis::Y))))
                    .separator()
                    .item(PopupMenuItem::new(forward_label).icon(forward_icon).disabled(!forward).on_click(popup::on_select(join(true))))
                    .item(PopupMenuItem::new(backward_label).icon(backward_icon).disabled(!backward).on_click(popup::on_select(join(false))))
                    .separator()
                    .item(PopupMenuItem::new("Swap areas").icon(swap_icon).on_click(popup::on_select(swap)))
            });
            let opened = popup::open(window, cx, menu, |this: &mut Self, cx| {
                this.edge_menu = None;
                cx.notify();
            });
            if let Some(open) = self.edge_menu.as_mut() {
                open.popup = Some(opened);
            }
        }
        let open = self.edge_menu.as_ref()?;
        Some(open.popup.as_ref()?.draw(window, open.at).into_any_element())
    }

    fn render_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.as_ref()?;
        if open.popup.is_none() {
            let (id, kind) = (open.area, open.kind);
            let current = self.screen.area(id)?.editor.clone();
            let maximized = self.maximized == Some(id);
            let can_close = self.screen.areas().len() >= 2;
            let view = cx.entity();
            let act = |f: fn(&mut Self, AreaId, &mut Context<Self>)| {
                let view = view.clone();
                move |_: &mut Window, cx: &mut App| view.update(cx, |this, cx| f(this, id, cx))
            };
            let menu = match kind {
                MenuKind::Type => {
                    let items: Vec<PopupMenuItem> = E::all()
                        .iter()
                        .map(|editor| {
                            let view = view.clone();
                            let editor_value = editor.clone();
                            PopupMenuItem::new(editor.label())
                                .icon(editor.icon())
                                .checked(*editor == current)
                                .on_click(popup::on_select(move |_, cx| {
                                    let editor = editor_value.clone();
                                    view.update(cx, |this, cx| {
                                        let mut screen = this.screen.clone();
                                        screen.set_editor(id, editor);
                                        this.set_screen(screen, cx);
                                    });
                                }))
                        })
                        .collect();
                    PopupMenu::build(window, cx, move |menu, _, _| {
                        items.into_iter().fold(menu.min_w(px(176.0)), |menu, item| menu.item(item))
                    })
                }
                MenuKind::Area => PopupMenu::build(window, cx, move |menu, _, _| {
                    menu.min_w(px(208.0))
                        .item(PopupMenuItem::new("Vertical split").on_click(popup::on_select(act(|this, id, cx| this.split(id, Axis::X, cx)))))
                        .item(PopupMenuItem::new("Horizontal split").on_click(popup::on_select(act(|this, id, cx| this.split(id, Axis::Y, cx)))))
                        .separator()
                        .item(
                            PopupMenuItem::new(if maximized { "Restore area" } else { "Maximize area" })
                                .on_click(popup::on_select(act(Self::toggle_maximized))),
                        )
                        .separator()
                        .item(PopupMenuItem::new("Close area").disabled(!can_close).on_click(popup::on_select(act(Self::close))))
                }),
            };
            let opened = popup::open(window, cx, menu, |this: &mut Self, cx| {
                this.menu = None;
                cx.notify();
            });
            if let Some(open) = self.menu.as_mut() {
                open.popup = Some(opened);
            }
        }
        let open = self.menu.as_ref()?;
        Some(open.popup.as_ref()?.draw(window, open.at).into_any_element())
    }

    /// Handles in an area's corners: drag into the area to split it, out to join or dock, with Ctrl to swap.
    fn corner_handles(&self, area: AreaId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.maximized.is_some() {
            return Vec::new();
        }
        (0..4)
            .map(|corner| {
                let handle = div()
                    .id(("corner", area.0 as usize * 4 + corner))
                    .absolute()
                    .size(px(CORNER_SIZE))
                    .cursor(CursorStyle::Crosshair)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.safe_listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.start_corner_drag(area, event, window, cx);
                        }),
                    );
                match corner {
                    0 => handle.top_0().left_0(),
                    1 => handle.top_0().right_0(),
                    2 => handle.bottom_0().left_0(),
                    _ => handle.bottom_0().right_0(),
                }
                .into_any_element()
            })
            .collect()
    }

    /// The part of the window the gesture under way would affect, and what it would do.
    fn highlight(&self) -> Option<(crate::Rect, &'static str)> {
        if let Some(split) = &self.splitting {
            let area = self.screen.area(split.area?)?;
            let r = area.rect;
            let rect = match split.axis {
                Axis::X => crate::Rect { left: split.at, ..r },
                Axis::Y => crate::Rect { top: split.at, ..r },
            };
            return Some((rect, "Split"));
        }
        let corner = self.corner.as_ref()?;
        let outcome = corner.outcome?;
        Some((gestures::highlight(&self.screen, corner.source, &outcome)?, outcome.label()))
    }

    fn gesture_cursor(&self) -> CursorStyle {
        let along = |axis: Axis| match axis {
            Axis::X => CursorStyle::ResizeLeftRight,
            Axis::Y => CursorStyle::ResizeUpDown,
        };
        if let Some(split) = &self.splitting {
            return along(split.axis);
        }
        match self.corner.as_ref().and_then(|c| c.outcome) {
            None => CursorStyle::Crosshair,
            Some(Outcome::Split { axis, .. }) => along(axis),
            Some(_) => CursorStyle::ClosedHand,
        }
    }

    /// Covers the window during a gesture, so what's under the pointer doesn't react to it.
    fn gesture_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.gesture_active() {
            return None;
        }
        Some(
            div()
                .id("screen-gesture")
                .track_focus(&self.focus)
                .absolute()
                .size_full()
                .occlude()
                .cursor(self.gesture_cursor())
                .on_mouse_move(cx.safe_listener(|this, event: &MouseMoveEvent, window, cx| {
                    this.gesture_move(event, window, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.safe_listener(|this, _: &MouseUpEvent, window, cx| {
                        if this.corner.is_some() {
                            this.finish_corner(window, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, _: &MouseDownEvent, window, cx| {
                        if this.splitting.is_some() {
                            this.finish_split(true, window, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.safe_listener(|this, _: &MouseDownEvent, window, cx| {
                        this.cancel_gesture(window, cx);
                    }),
                )
                .on_key_down(cx.safe_listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel_gesture(window, cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// A draggable strip over each interior edge between areas.
    fn dividers(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut lines: Vec<(Axis, f64, f64, f64)> = Vec::new();
        for area in self.screen.areas() {
            let r = area.rect;
            for (axis, at, start, end) in [
                (Axis::X, r.right, r.top, r.bottom),
                (Axis::Y, r.bottom, r.left, r.right),
            ] {
                if at < 1.0 - 1e-9 {
                    lines.push((axis, at, start, end));
                }
            }
        }
        let grab = px(EDGE_GRAB * 2.0);
        lines
            .into_iter()
            .enumerate()
            .map(|(i, (axis, at, start, end))| {
                let strip = div()
                    .id(("divider", i))
                    .absolute()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.safe_listener(|this, event, _, cx| this.start_edge_drag(event, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.safe_listener(|this, event: &MouseDownEvent, _, cx| {
                            this.open_edge_menu(event.position, cx);
                        }),
                    )
                    .on_drag(EdgeDrag, |_, _, _, cx| cx.new(|_| Empty));
                match axis {
                    Axis::X => strip
                        .cursor(CursorStyle::ResizeLeftRight)
                        .left(relative(at as f32))
                        .ml(-grab / 2.0)
                        .top(relative(start as f32))
                        .h(relative((end - start) as f32))
                        .w(grab),
                    Axis::Y => strip
                        .cursor(CursorStyle::ResizeUpDown)
                        .top(relative(at as f32))
                        .mt(-grab / 2.0)
                        .left(relative(start as f32))
                        .w(relative((end - start) as f32))
                        .h(grab),
                }
                .into_any_element()
            })
            .collect()
    }
}

impl<E: Editor> Render for ScreenView<E> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("screen", || {
        let theme = cx.theme().clone();
        let (background, border) = (theme.colors.background, theme.colors.border);
        let view = cx.entity();
        let maximized = self.maximized;
        let areas: Vec<Area<E>> = self.screen.areas().to_vec();
        let rendered: Vec<AnyElement> = areas
            .iter()
            .filter(|area| maximized.is_none_or(|id| id == area.id))
            .map(|area| {
                let content = (self.render_area)(area.id, &area.editor, window, cx);
                let header = self.render_header(area, content.toolbar, cx);
                let r = area.rect;
                let rect = if maximized.is_some() {
                    crate::Rect::FULL
                } else {
                    r
                };
                let id = area.id;
                div()
                    .id(("area", id.0 as usize))
                    .absolute()
                    .left(relative(rect.left as f32))
                    .top(relative(rect.top as f32))
                    .w(relative(rect.width() as f32))
                    .h(relative(rect.height() as f32))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, _| {
                        if *hovered {
                            this.hovered = Some(id);
                        } else if this.hovered == Some(id) {
                            this.hovered = None;
                        }
                    }))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(background)
                    .when(rect.right < 1.0 - 1e-9, |d| {
                        d.border_r_1().border_color(border)
                    })
                    .when(rect.bottom < 1.0 - 1e-9, |d| {
                        d.border_b_1().border_color(border)
                    })
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .overflow_hidden()
                            .children(
                                content
                                    .body
                                    .map(|body| body.cached(StyleRefinement::default().size_full())),
                            ),
                    )
                    .children(self.corner_handles(id, cx))
                    .into_any_element()
            })
            .collect();
        let dividers = if maximized.is_some() {
            Vec::new()
        } else {
            self.dividers(cx)
        };
        let highlight = self.highlight().map(|(rect, label)| {
            div()
                .absolute()
                .left(relative(rect.left as f32))
                .top(relative(rect.top as f32))
                .w(relative(rect.width() as f32))
                .h(relative(rect.height() as f32))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radius.md)
                .border_2()
                .border_color(theme.colors.primary)
                .bg(theme.colors.primary.opacity(0.15))
                .child(
                    div()
                        .px_2()
                        .py(px(2.0))
                        .rounded(theme.radius.sm)
                        .bg(theme.colors.background.opacity(0.8))
                        .text_size(theme.text.xs)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label),
                )
        });
        let overlay = self.gesture_overlay(cx);
        div()
            .relative()
            .size_full()
            .bg(background)
            .on_key_down(cx.safe_listener(|this, event: &KeyDownEvent, _, cx| {
                let m = event.keystroke.modifiers;
                if event.keystroke.key == "space"
                    && m.control
                    && !m.platform
                    && !m.alt
                    && let Some(hovered) = this.hovered
                {
                    this.toggle_maximized(hovered, cx);
                    cx.stop_propagation();
                }
            }))
            .font_family(theme.font.clone())
            .text_size(theme.text.sm)
            .text_color(theme.colors.foreground)
            .child(
                canvas(
                    move |bounds, _, cx| { guard::guard("view measure", || {
                        view.update(cx, |this, cx| {
                            if this.bounds != bounds {
                                this.bounds = bounds;
                                cx.notify();
                            }
                        });
                    }); },
                    |_, _, _, _| { guard::guard("view canvas", || {}); },
                )
                .absolute()
                .size_full(),
            )
            .children(rendered)
            .children(dividers)
            .children(highlight)
            .children(overlay)
            .on_drag_move::<EdgeDrag>(cx.safe_listener(
                |this, event: &DragMoveEvent<EdgeDrag>, _, cx| {
                    this.drag_edge(event.event.position, cx);
                },
            ))
            .children(self.render_menu(window, cx))
            .children(self.render_edge_menu(window, cx))
    })
    }
}

#[cfg(test)]
mod ui_tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext, size};

    use super::*;
    use crate::Rect;

    #[derive(Clone, Debug, PartialEq)]
    enum Panel {
        A,
        B,
    }

    impl Editor for Panel {
        fn all() -> &'static [Self] {
            &[Self::A, Self::B]
        }

        fn label(&self) -> &'static str {
            "panel"
        }

        fn icon(&self) -> IconName {
            IconName::Code
        }
    }

    fn open(cx: &mut TestAppContext) -> (gpui::Entity<ScreenView<Panel>>, &mut VisualTestContext) {
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let screen = Screen::new([
                (Rect::new(0.0, 0.0, 0.5, 1.0), Panel::A),
                (Rect::new(0.5, 0.0, 1.0, 1.0), Panel::B),
            ]);
            ScreenView::new(
                screen,
                |_, _, _, _| AreaContent { toolbar: None, body: None },
                cx,
            )
        });
        cx.simulate_resize(size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        (view, cx)
    }

    fn drag_corner(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>, modifiers: Modifiers) {
        cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(to, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn dragging_a_corner_into_a_neighbour_joins_it(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        drag_corner(cx, point(px(495.0), px(795.0)), point(px(560.0), px(300.0)), Modifiers::default());
        assert_eq!(view.read_with(cx, |v, _| v.screen().areas().len()), 1);
    }

    #[gpui::test]
    fn dragging_a_corner_with_control_swaps_the_editors(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let control = Modifiers { control: true, ..Modifiers::default() };
        drag_corner(cx, point(px(495.0), px(795.0)), point(px(800.0), px(300.0)), control);
        let editors: Vec<Panel> =
            view.read_with(cx, |v, _| v.screen().areas().iter().map(|a| a.editor.clone()).collect());
        assert_eq!(editors, [Panel::B, Panel::A]);
    }

    #[gpui::test]
    fn dragging_a_corner_into_its_own_area_splits_it(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        drag_corner(cx, point(px(5.0), px(5.0)), point(px(200.0), px(60.0)), Modifiers::default());
        assert_eq!(view.read_with(cx, |v, _| v.screen().areas().len()), 3);
    }

    #[gpui::test]
    fn a_short_corner_drag_changes_nothing(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        drag_corner(cx, point(px(5.0), px(5.0)), point(px(10.0), px(10.0)), Modifiers::default());
        assert_eq!(view.read_with(cx, |v, _| v.screen().areas().len()), 2);
        assert!(view.read_with(cx, |v, _| v.corner.is_none()));
    }

    #[gpui::test]
    fn the_edge_menu_opens_on_a_right_click_over_an_edge_and_split_follows_the_pointer(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_mouse_down(point(px(500.0), px(300.0)), MouseButton::Right, Modifiers::default());
        cx.run_until_parked();
        let (before, after, axis) = view.read_with(cx, |v, _| {
            let m = v.edge_menu.as_ref().expect("the edge menu opened");
            (m.before, m.after, m.axis)
        });
        assert_eq!((before, after, axis), (AreaId(0), AreaId(1), Axis::X));
        // Splitting is started by the menu's item: follow, then click to put it down.
        view.update(cx, |v, cx| {
            v.edge_menu = None;
            v.splitting = Some(Splitting { axis: Axis::X, area: None, at: 0.0 });
            v.update_split();
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(750.0), px(300.0)), None, Modifiers::default());
        cx.simulate_mouse_down(point(px(750.0), px(300.0)), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.screen().areas().len()), 3);
    }

    #[gpui::test]
    fn right_click_cancels_a_split_in_progress(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, cx| {
            v.splitting = Some(Splitting { axis: Axis::Y, area: None, at: 0.0 });
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_mouse_down(point(px(750.0), px(300.0)), MouseButton::Right, Modifiers::default());
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.splitting.is_none()));
        assert_eq!(view.read_with(cx, |v, _| v.screen().areas().len()), 2);
    }
}
