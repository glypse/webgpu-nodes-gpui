use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, CursorStyle, DispatchPhase, ElementId, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, RenderOnce, SharedString, Styled as _, Subscription,
    TextAlign, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Escape, Input, InputEvent, InputState, MaskPattern};
use theme::{ActiveTheme as _, Icon, IconName};

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

/// The typed entry of a number, kept beside its scrub across renders.
struct EntryState {
    input: Entity<InputState>,
    editing: bool,
    /// Where focus was before typing began, to give it back once done.
    previous_focus: Option<FocusHandle>,
    value: f64,
    integer: bool,
    min: Option<f64>,
    max: Option<f64>,
    on_change: Option<OnNumber>,
    _subscription: Subscription,
}

impl EntryState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).mask_pattern(MaskPattern::Number {
                separator: None,
                fraction: None,
            })
        });
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                guard::guard("number entry", || match event {
                    InputEvent::PressEnter { .. } => {
                        this.commit(window, cx);
                        this.restore_focus(window, cx);
                    }
                    InputEvent::Blur => this.commit(window, cx),
                    _ => {}
                });
            });
        Self {
            input,
            editing: false,
            previous_focus: None,
            value: 0.0,
            integer: false,
            min: None,
            max: None,
            on_change: None,
            _subscription: subscription,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previous_focus = window.focused(cx);
        self.editing = true;
        let text = format_number(self.value, self.integer);
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if self.editing {
            self.editing = false;
            cx.notify();
        }
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.previous_focus.take() {
            window.focus(&handle, cx);
        }
    }

    /// Input that isn't a number leaves the value as it was.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        let typed = parse_number(&self.input.read(cx).value());
        self.finish(cx);
        let Some(typed) = typed else {
            return;
        };
        let next = constrain(typed, self.integer, self.min, self.max);
        if next != self.value
            && let Some(on_change) = self.on_change.clone()
        {
            on_change(next, Edit::Commit, window, cx);
        }
    }
}

fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn constrain(value: f64, integer: bool, min: Option<f64>, max: Option<f64>) -> f64 {
    let value = if integer { value.round() } else { value };
    value.max(min.unwrap_or(f64::NEG_INFINITY)).min(max.unwrap_or(f64::INFINITY))
}

#[derive(Default)]
struct ScrubState {
    /// Where the press started and the value then, until it's released.
    pressed: Option<(f32, f64)>,
    scrubbing: bool,
    last: f64,
}

/// Where a field sits in a stack of fields, such as a vector's components:
/// only the outer corners of the stack are rounded, and each field below the
/// first is set off by a separating line.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Stack {
    #[default]
    Single,
    First,
    Middle,
    Last,
}

impl Stack {
    /// The stack position of item `index` of `count`.
    pub fn of(index: usize, count: usize) -> Self {
        match (index, count) {
            (_, 0 | 1) => Self::Single,
            (0, _) => Self::First,
            (i, n) if i + 1 == n => Self::Last,
            _ => Self::Middle,
        }
    }

    fn rounds_top(self) -> bool {
        matches!(self, Self::Single | Self::First)
    }

    fn rounds_bottom(self) -> bool {
        matches!(self, Self::Single | Self::Last)
    }

    fn separated(self) -> bool {
        matches!(self, Self::Middle | Self::Last)
    }
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
    stack: Stack,
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
            stack: Stack::Single,
            on_change: None,
        }
    }

    pub fn stack(mut self, stack: Stack) -> Self {
        self.stack = stack;
        self
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement { guard::render("number field", || {
        let colors = cx.theme().colors.clone();
        let z = self.zoom;
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ScrubState::default());
        let scrubbing = state.read(cx).scrubbing;
        let pressed = state.read(cx).pressed.is_some();
        let (integer, value, min, max) = (self.integer, self.value, self.min, self.max);
        let clamp = move |v: f64| constrain(v, integer, min, max);
        let entry = window.use_keyed_state((self.id.clone(), "entry"), cx, |window, cx| {
            EntryState::new(window, cx)
        });
        let typing = entry.read(cx).editing;
        let input = entry.read(cx).input.clone();
        entry.update(cx, |e, _| {
            e.value = value;
            e.integer = integer;
            e.min = min;
            e.max = max;
            e.on_change = self.on_change.clone();
        });
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
                .child(Icon::new(if right { IconName::CaretRight } else { IconName::CaretLeft }).size(px(10.0 * z)))
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
            .when(self.stack.rounds_top() && self.stack.rounds_bottom(), |d| d.rounded(px(4.0 * z)))
            .when(self.stack.rounds_top() && !self.stack.rounds_bottom(), |d| d.rounded_t(px(4.0 * z)))
            .when(!self.stack.rounds_top() && self.stack.rounds_bottom(), |d| d.rounded_b(px(4.0 * z)))
            .when(self.stack.separated(), |d| d.border_t(px(z)).border_color(colors.border))
            .bg(fg.opacity(if scrubbing || pressed { 0.1 } else { 0.05 }))
            .text_size(px(12.0 * z))
            .text_color(fg)
            .when(self.readonly, |d| d.opacity(0.5))
            .when(typing, |d| {
                let entry = entry.clone();
                let field = div()
                    .w_full()
                    .h(px(22.0 * z))
                    .min_w_0()
                    .on_action(move |_: &Escape, window, cx| {
                        cx.stop_propagation();
                        guard::guard("number entry", || {
                            entry.update(cx, |e, cx| {
                                e.finish(cx);
                                e.restore_focus(window, cx);
                            });
                        });
                    })
                    .child(
                        // The input's own size sets its text and padding, so they are
                        // overridden here to follow the zoom. Its height fills the wrapper.
                        Input::new(&input)
                            .appearance(false)
                            .px(px(4.0 * z))
                            .py(px(0.0))
                            .text_size(px(12.0 * z))
                            .line_height(px(15.0 * z))
                            .text_align(TextAlign::Center),
                    );
                #[cfg(test)]
                let field = field.debug_selector(|| "scrub-entry".into());
                d.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(field)
            })
            .when(!self.readonly && !typing, |d| {
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
            .when(!typing, |d| d.child(format_number(value, integer)))
            .when(pressed && !typing, |d| {
                let state = state.clone();
                let entry = entry.clone();
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| { guard::guard("fields canvas", || {
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
                                if !was_scrubbing {
                                    entry.update(cx, |e, cx| e.start(window, cx));
                                } else if let Some(f) = &changed {
                                    f(last, Edit::Commit, window, cx);
                                }
                            });
                        }); },
                    )
                    .absolute()
                    .size_0(),
                )
            })
    })
    }
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
        .border(px(zoom))
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
        .child(Icon::new(IconName::CaretDown).size(px(10.0 * zoom)).color(colors.muted_foreground))
}

/// A checkbox drawn at `zoom`, so it scales with the card around it.
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    zoom: f32,
    disabled: bool,
    cx: &App,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    let (fill, line) = if checked { (colors.primary, colors.primary) } else { (colors.background, colors.input) };
    div()
        .id(id.into())
        .flex_none()
        .size(px(16.0 * zoom))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0 * zoom))
        .border(px(zoom))
        .border_color(line)
        .bg(fill)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.cursor_pointer())
        .when(checked, |d| d.child(Icon::new(IconName::Check).size(px(12.0 * zoom)).color(colors.primary_foreground)))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            if !disabled {
                on_change(!checked, window, cx);
            }
        })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext, point};

    type Changes = Rc<RefCell<Vec<(f64, Edit)>>>;

    struct Host {
        value: f64,
        zoom: f32,
        changes: Changes,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let host = cx.entity();
            let changes = self.changes.clone();
            div().w(px(200.0)).h(px(40.0)).child(
                Scrub::new("n", self.value, self.zoom).range(Some(0.0), Some(10.0)).on_change(
                    move |v, edit, _, cx| {
                        changes.borrow_mut().push((v, edit));
                        host.update(cx, |h, cx| {
                            h.value = v;
                            cx.notify();
                        });
                    },
                ),
            )
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Changes, &mut gpui::VisualTestContext) {
        setup_at(cx, 1.0)
    }

    fn setup_at(cx: &mut TestAppContext, zoom: f32) -> (Changes, &mut gpui::VisualTestContext) {
        cx.update(theme::init);
        let changes = Rc::new(RefCell::new(Vec::new()));
        let shared = changes.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host { value: 1.0, zoom, changes: shared });
        cx.run_until_parked();
        (changes, cx)
    }

    fn type_into_field(cx: &mut gpui::VisualTestContext, text: &str, key: &str) {
        let at = point(px(100.0), px(11.0));
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input(text);
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn typing_a_number_and_pressing_enter_commits_it(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx);
        type_into_field(cx, "7.5", "enter");
        assert_eq!(*changes.borrow(), vec![(7.5, Edit::Commit)]);
    }

    #[gpui::test]
    fn typed_values_are_held_to_the_range(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx);
        type_into_field(cx, "99", "enter");
        assert_eq!(*changes.borrow(), vec![(10.0, Edit::Commit)]);
    }

    #[gpui::test]
    fn the_typed_entry_is_as_tall_as_the_zoomed_field(cx: &mut TestAppContext) {
        let (_, cx) = setup_at(cx, 2.0);
        cx.simulate_click(point(px(100.0), px(22.0)), Modifiers::default());
        cx.run_until_parked();
        let entry = cx.debug_bounds("scrub-entry").expect("typing shows the entry");
        assert_eq!(entry.size.height, px(44.0));
    }

    #[gpui::test]
    fn escape_and_invalid_text_leave_the_value_alone(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx);
        type_into_field(cx, "7", "escape");
        type_into_field(cx, "abc", "enter");
        assert!(changes.borrow().is_empty());
    }

    struct Panicking;

    impl Render for Panicking {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            guard::render("panicking view", || -> gpui::Div { panic!("render failed") })
        }
    }

    #[gpui::test]
    fn a_view_whose_render_panics_leaves_the_window_alive(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Panicking);
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[test]
    fn parsing_rejects_what_is_not_a_finite_number() {
        assert_eq!(parse_number(" 2.5 "), Some(2.5));
        assert_eq!(parse_number("-3"), Some(-3.0));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("1e999"), None);
        assert_eq!(parse_number("NaN"), None);
    }
}
