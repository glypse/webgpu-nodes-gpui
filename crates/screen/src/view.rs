use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent,
    Empty, EventEmitter, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, StatefulInteractiveElement as _, Styled as _,
    Window, canvas, div, point, prelude::FluentBuilder as _, px, relative,
};
use theme::{ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName, Menu, MenuItem};

use crate::{Area, AreaId, Axis, EdgeSelection, MinSize, Part, Screen};

/// The kinds of panel an area can show.
pub trait Editor: Clone + PartialEq + 'static {
    fn all() -> &'static [Self];
    fn label(&self) -> &'static str;
    fn icon(&self) -> IconName;
}

/// What an area shows: its panel's controls, drawn in the header after the
/// type picker, and its body.
pub struct AreaContent {
    pub toolbar: Option<AnyElement>,
    pub body: AnyElement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenEvent {
    /// The areas, their sizes or their editors changed.
    Changed,
}

/// Smallest area, in pixels: a header and a bit of body.
const MIN_AREA: (f32, f32) = (96.0, 56.0);
/// How far from an edge the pointer grabs it.
const EDGE_GRAB: f32 = 4.0;
const HEADER_HEIGHT: f32 = 36.0;

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
}

impl<E: Editor> EventEmitter<ScreenEvent> for ScreenView<E> {}

impl<E: Editor> ScreenView<E> {
    pub fn new(
        screen: Screen<E>,
        render_area: impl Fn(AreaId, &E, &mut Window, &mut App) -> AreaContent + 'static,
    ) -> Self {
        Self {
            screen,
            render_area: Box::new(render_area),
            bounds: Bounds::default(),
            maximized: None,
            drag: None,
            menu: None,
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
            width: f64::from(MIN_AREA.0) / width.max(1.0),
            height: f64::from(MIN_AREA.1) / height.max(1.0),
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
        toolbar: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = area.id;
        let open_type = move |this: &mut Self, event: &ClickEvent, cx: &mut Context<Self>| {
            this.menu = Some(OpenMenu {
                area: id,
                kind: MenuKind::Type,
                at: event.position() + point(px(0.0), px(4.0)),
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
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.menu = Some(OpenMenu {
                        area: id,
                        kind: MenuKind::Area,
                        at: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(
                Button::new(("area-type", id.0 as usize))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .icon(area.editor.icon())
                    .icon_end(IconName::CaretDown)
                    .selected(type_open)
                    .tooltip("Editor type")
                    .on_click(cx.listener(move |this, event, _, cx| open_type(this, event, cx))),
            )
            .children(toolbar)
            .into_any_element()
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.as_ref()?;
        let area = self.screen.area(open.area)?;
        let id = open.area;
        let view = cx.entity();
        let dismiss = {
            let view = view.clone();
            move |_: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.menu = None;
                    cx.notify();
                });
            }
        };
        let act = |f: fn(&mut Self, AreaId, &mut Context<Self>)| {
            let view = view.clone();
            move |_: &mut Window, cx: &mut App| view.update(cx, |this, cx| f(this, id, cx))
        };
        let menu = Menu::new("area-menu").at(open.at).on_dismiss(dismiss);
        Some(
            match open.kind {
                MenuKind::Type => E::all().iter().fold(menu, |menu, editor| {
                    let view = view.clone();
                    let editor = editor.clone();
                    menu.item(
                        MenuItem::new(editor.label())
                            .icon(editor.icon())
                            .checked(editor == area.editor)
                            .on_select(move |_, cx| {
                                let editor = editor.clone();
                                view.update(cx, |this, cx| {
                                    let mut screen = this.screen.clone();
                                    screen.set_editor(id, editor);
                                    this.set_screen(screen, cx);
                                });
                            }),
                    )
                }),
                MenuKind::Area => {
                    let maximized = self.maximized == Some(id);
                    menu.width(px(208.0))
                        .item(
                            MenuItem::new("Vertical split")
                                .on_select(act(|this, id, cx| this.split(id, Axis::X, cx))),
                        )
                        .item(
                            MenuItem::new("Horizontal split")
                                .on_select(act(|this, id, cx| this.split(id, Axis::Y, cx))),
                        )
                        .separator()
                        .item(
                            MenuItem::new(if maximized {
                                "Restore area"
                            } else {
                                "Maximize area"
                            })
                            .on_select(act(Self::toggle_maximized)),
                        )
                        .separator()
                        .item(
                            MenuItem::new("Close area")
                                .disabled(self.screen.areas().len() < 2)
                                .on_select(act(Self::close)),
                        )
                }
            }
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
                        cx.listener(|this, event, _, cx| this.start_edge_drag(event, cx)),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                div()
                    .absolute()
                    .left(relative(rect.left as f32))
                    .top(relative(rect.top as f32))
                    .w(relative(rect.width() as f32))
                    .h(relative(rect.height() as f32))
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
                            .child(content.body),
                    )
                    .into_any_element()
            })
            .collect();
        let dividers = if maximized.is_some() {
            Vec::new()
        } else {
            self.dividers(cx)
        };
        div()
            .relative()
            .size_full()
            .bg(background)
            .font_family(theme.font.clone())
            .text_size(theme.text.sm)
            .text_color(theme.colors.foreground)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        view.update(cx, |this, cx| {
                            if this.bounds != bounds {
                                this.bounds = bounds;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rendered)
            .children(dividers)
            .on_drag_move::<EdgeDrag>(cx.listener(
                |this, event: &DragMoveEvent<EdgeDrag>, _, cx| {
                    this.drag_edge(event.event.position, cx);
                },
            ))
            .children(self.render_menu(cx))
    }
}
