use std::rc::Rc;

use gpui::{
    App, CursorStyle, DispatchPhase, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, RenderOnce, SharedString,
    Styled as _, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use theme::ActiveTheme as _;

/// Whether a change is a step of a gesture still going on, or its end.
/// History keeps only the ends, so a scrub is undone in one go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Preview,
    Commit,
}

pub type OnNumber = Rc<dyn Fn(f64, Edit, &mut Window, &mut App)>;

/// Moves under this many pixels are a click, not a scrub.
const DRAG_THRESHOLD: f32 = 3.0;

#[derive(Default)]
struct ScrubState {
    /// Where the press started and the value then, until it's released.
    pressed: Option<(f32, f64)>,
    scrubbing: bool,
    last: f64,
}

/// A number scrubbed by dragging sideways, with step arrows at its sides
/// while hovered; 0.1 per 2 pixels, 0.01 with Shift. Sizes follow `zoom`.
#[derive(IntoElement)]
pub struct Scrub {
    id: ElementId,
    value: f64,
    zoom: f32,
    integer: bool,
    min: Option<f64>,
    max: Option<f64>,
    readonly: bool,
    on_change: Option<OnNumber>,
}

impl Scrub {
    pub fn new(id: impl Into<ElementId>, value: f64, zoom: f32) -> Self {
        Self {
            id: id.into(),
            value,
            zoom,
            integer: false,
            min: None,
            max: None,
            readonly: false,
            on_change: None,
        }
    }

    pub fn integer(mut self, integer: bool) -> Self {
        self.integer = integer;
        self
    }

    pub fn range(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn on_change(mut self, f: impl Fn(f64, Edit, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

fn step(integer: bool, shift: bool) -> f64 {
    match (integer, shift) {
        (true, _) => 1.0,
        (false, true) => 0.01,
        (false, false) => 0.1,
    }
}

pub fn format_number(value: f64, integer: bool) -> SharedString {
    if integer {
        format!("{}", value.round() as i64).into()
    } else {
        let text = format!("{value:.3}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        if text.is_empty() || text == "-" {
            "0".into()
        } else {
            text.to_owned().into()
        }
    }
}

impl RenderOnce for Scrub {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let z = self.zoom;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ScrubState::default());
        let scrubbing = state.read(cx).scrubbing;
        let pressed = state.read(cx).pressed.is_some();
        let (integer, value, min, max) = (self.integer, self.value, self.min, self.max);
        let clamp = move |v: f64| {
            let v = if integer { v.round() } else { v };
            v.max(min.unwrap_or(f64::NEG_INFINITY))
                .min(max.unwrap_or(f64::INFINITY))
        };
        let group: SharedString = format!("scrub-{:?}", self.id).into();

        let side = |right: bool| {
            let on_change = self.on_change.clone();
            let fg = colors.foreground;
            div()
                .id(if right { "inc" } else { "dec" })
                .absolute()
                .top_0()
                .bottom_0()
                .when(right, |d| d.right_0())
                .when(!right, |d| d.left_0())
                .w(px(16.0 * z))
                .flex()
                .items_center()
                .justify_center()
                .text_color(fg)
                .opacity(0.0)
                .group_hover(group.clone(), |s| s.opacity(0.5))
                .hover(move |s| s.opacity(1.0).bg(fg.opacity(0.1)))
                .child(if right { "›" } else { "‹" })
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    cx.stop_propagation();
                    let by = step(integer, e.modifiers.shift);
                    if let Some(f) = &on_change {
                        f(
                            clamp(value + if right { by } else { -by }),
                            Edit::Commit,
                            window,
                            cx,
                        );
                    }
                })
        };

        let on_change = self.on_change.clone();
        let press_state = state.clone();
        let fg = colors.foreground;
        div()
            .id(self.id.clone())
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .h(px(22.0 * z))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .rounded(px(4.0 * z))
            .bg(fg.opacity(if scrubbing || pressed { 0.1 } else { 0.05 }))
            .text_size(px(12.0 * z))
            .text_color(fg)
            .when(self.readonly, |d| d.opacity(0.5))
            .when(!self.readonly, |d| {
                d.cursor(CursorStyle::ResizeLeftRight)
                    .hover(move |s| s.bg(fg.opacity(0.08)))
                    .on_mouse_down(MouseButton::Left, move |e, _, cx| {
                        cx.stop_propagation();
                        press_state.update(cx, |s, cx| {
                            s.pressed = Some((f32::from(e.position.x), value));
                            s.scrubbing = false;
                            s.last = value;
                            cx.notify();
                        });
                    })
                    .child(side(false))
                    .child(side(true))
            })
            .child(format_number(value, integer))
            .when(pressed, |d| {
                let state = state.clone();
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let on_move = state.clone();
                            let changed = on_change.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                                if phase != DispatchPhase::Bubble {
                                    return;
                                }
                                let Some((start_x, start)) = on_move.read(cx).pressed else {
                                    return;
                                };
                                let dx = f32::from(e.position.x) - start_x;
                                let scrubbing = on_move.read(cx).scrubbing;
                                if !scrubbing && dx.abs() <= DRAG_THRESHOLD {
                                    return;
                                }
                                let by = step(integer, e.modifiers.shift);
                                let next = clamp(
                                    start
                                        + f64::from(dx.signum())
                                            * by
                                            * f64::from((dx.abs() / 2.0).ceil()),
                                );
                                on_move.update(cx, |s, cx| {
                                    s.scrubbing = true;
                                    s.last = next;
                                    cx.notify();
                                });
                                if let Some(f) = &changed {
                                    f(next, Edit::Preview, window, cx);
                                }
                            });
                            let on_up = state.clone();
                            let changed = on_change.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                                if phase != DispatchPhase::Bubble {
                                    return;
                                }
                                let (was_scrubbing, last) = {
                                    let s = on_up.read(cx);
                                    (s.scrubbing, s.last)
                                };
                                on_up.update(cx, |s, cx| {
                                    *s = ScrubState::default();
                                    cx.notify();
                                });
                                if was_scrubbing && let Some(f) = &changed {
                                    f(last, Edit::Commit, window, cx);
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            })
    }
}

/// An on/off box.
pub fn check(
    id: impl Into<ElementId>,
    on: bool,
    zoom: f32,
    readonly: bool,
    cx: &App,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    let (fg, primary) = (colors.foreground, colors.primary);
    div()
        .id(id.into())
        .flex_1()
        .h(px(22.0 * zoom))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0 * zoom))
        .text_size(px(12.0 * zoom))
        .bg(if on { primary } else { fg.opacity(0.05) })
        .text_color(if on { colors.primary_foreground } else { fg })
        .when(!readonly, |d| {
            d.cursor_pointer().hover(move |s| s.opacity(0.85))
        })
        .when(readonly, |d| d.opacity(0.5))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            if !readonly {
                on_toggle(!on, window, cx);
            }
        })
        .child(if on { "true" } else { "false" })
}

/// A flat button that opens a list: a type picker, an enum.
pub fn picker(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    zoom: f32,
    disabled: bool,
    cx: &App,
    on_click: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    let fg = colors.foreground;
    div()
        .id(id.into())
        .flex_1()
        .min_w_0()
        .h(px(24.0 * zoom))
        .px(px(8.0 * zoom))
        .flex()
        .items_center()
        .justify_between()
        .rounded(px(4.0 * zoom))
        .border(px(1.0))
        .border_color(colors.input)
        .text_size(px(12.0 * zoom))
        .text_color(fg)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer().hover(move |s| s.bg(fg.opacity(0.08)))
        })
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            cx.stop_propagation();
            if !disabled {
                on_click(e.position, window, cx);
            }
        })
        .child(div().truncate().child(label.into()))
        .child(div().text_color(colors.muted_foreground).child("⌄"))
}
