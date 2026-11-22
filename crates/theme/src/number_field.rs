use gpui::{
    AppContext as _, ClickEvent, Context, CursorStyle, DragMoveEvent, Empty, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, TextAlign, Window,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Escape, Input, InputEvent, InputState, MaskPattern};
use guard::SafeListener as _;

use crate::ActiveTheme as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberEvent {
    Change(f64),
}

/// Marks a drag as scrubbing a number field.
#[derive(Clone, Copy)]
struct Scrub;

/// Pixels dragged per step of the value.
const PIXELS_PER_STEP: f64 = 6.0;
const DRAG_THRESHOLD: f32 = 3.0;

/// A number shown as a label and a value: drag it sideways to scrub, click to
/// type one in.
pub struct NumberField {
    label: Option<SharedString>,
    value: f64,
    step: f64,
    integer: bool,
    read_only: bool,
    min: Option<f64>,
    max: Option<f64>,
    soft_min: Option<f64>,
    soft_max: Option<f64>,
    /// Made on the first edit, which is when a window is at hand to build it.
    input: Option<Entity<InputState>>,
    _input_subscription: Option<Subscription>,
    editing: bool,
    scrub: Option<(f32, f64)>,
    moved: bool,
    /// Scales every size of the field, its text included.
    zoom: f32,
}

impl EventEmitter<NumberEvent> for NumberField {}

impl NumberField {
    pub fn new(value: f64, _cx: &mut Context<Self>) -> Self {
        Self {
            label: None,
            value,
            step: 0.1,
            integer: false,
            read_only: false,
            min: None,
            max: None,
            soft_min: None,
            soft_max: None,
            input: None,
            _input_subscription: None,
            editing: false,
            scrub: None,
            moved: false,
            zoom: 1.0,
        }
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if self.zoom != zoom {
            self.zoom = zoom;
            cx.notify();
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Whole numbers only, one per step.
    pub fn integer(mut self) -> Self {
        self.integer = true;
        self.step = 1.0;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// The range scrubbing stays within, widened to include the value it started
    /// from. Typing isn't held by it.
    pub fn soft_range(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        self.soft_min = min;
        self.soft_max = max;
        self
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Shown dimmed and neither scrubbed nor typed into.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            self.editing = false;
            cx.notify();
        }
    }

    /// Changes the label and range of a field already built.
    pub fn configure(
        &mut self,
        label: impl Into<SharedString>,
        min: Option<f64>,
        max: Option<f64>,
        cx: &mut Context<Self>,
    ) {
        self.label = Some(label.into());
        self.min = min;
        self.max = max;
        self.value = self.constrain(self.value);
        cx.notify();
    }

    /// Shows `value` without emitting a change.
    pub fn set_value(&mut self, value: f64, cx: &mut Context<Self>) {
        let value = self.constrain(value);
        if value != self.value {
            self.value = value;
            cx.notify();
        }
    }

    fn constrain(&self, value: f64) -> f64 {
        let value = if self.integer { value.round() } else { value };
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }

    fn format(&self) -> String {
        if self.integer {
            format!("{}", self.value.round() as i64)
        } else {
            format!("{:.3}", self.value)
        }
    }

    fn change(&mut self, value: f64, cx: &mut Context<Self>) {
        let value = self.constrain(value);
        if value != self.value {
            self.value = value;
            cx.emit(NumberEvent::Change(value));
            cx.notify();
        }
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        let text = self.format();
        let input = match self.input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx).mask_pattern(MaskPattern::Number {
                        separator: None,
                        fraction: None,
                    })
                });
                self._input_subscription =
                    Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
                                        if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            this.commit(cx);
                        }
                    }));
                self.input = Some(input.clone());
                input
            }
        };
        input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn stop_editing(&mut self, cx: &mut Context<Self>) {
        if self.editing {
            self.editing = false;
            cx.notify();
        }
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        self.editing = false;
        let typed = self
            .input
            .as_ref()
            .and_then(|input| input.read(cx).value().trim().parse::<f64>().ok());
        if let Some(parsed) = typed {
            self.change(parsed, cx);
        }
        cx.notify();
    }

    fn scrub_to(&mut self, x: f32, modifiers_shift: bool, cx: &mut Context<Self>) {
        let Some((start_x, start_value)) = self.scrub else {
            return;
        };
        let dx = f64::from(x - start_x);
        if !self.moved && dx.abs() < f64::from(DRAG_THRESHOLD) {
            return;
        }
        self.moved = true;
        let step = if modifiers_shift && !self.integer {
            self.step / 10.0
        } else {
            self.step
        };
        let target = start_value + dx / PIXELS_PER_STEP * step;
        self.change(
            soft_clamp(target, start_value, self.soft_min, self.soft_max),
            cx,
        );
    }
}

/// `value` kept within the soft range, widened to include `from`.
fn soft_clamp(value: f64, from: f64, soft_min: Option<f64>, soft_max: Option<f64>) -> f64 {
    let low = soft_min.map_or(value, |min| min.min(from));
    let high = soft_max.map_or(value, |max| max.max(from));
    value.max(low).min(high)
}

impl Render for NumberField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        guard::render("NumberField", || {
            let theme = cx.theme();
            let c = &theme.colors;
            let z = self.zoom;
            let text = theme.text.sm * z;
            let base = div()
                .flex()
                .items_center()
                .h(px(32.0 * z))
                .w_full()
                .min_w_0()
                .font_family(theme.font.clone())
                .text_size(text);
            if self.editing
                && let Some(input) = self.input.clone()
            {
                // The input's own text size and padding are set from its size, so they are
                // overridden here to follow the zoom.
                let entry = div()
                    .w_full()
                    .h_full()
                    .min_w_0()
                    .rounded(theme.radius.lg * z)
                    .border(px(z))
                    .border_color(c.ring)
                    .on_action(cx.safe_listener(|this, _: &Escape, _, cx| {
                        this.stop_editing(cx);
                    }))
                    .child(
                        Input::new(&input)
                            .appearance(false)
                            .px(px(10.0 * z))
                            .py(px(0.0))
                            .text_size(text)
                            .line_height(text * 1.25)
                            .text_align(TextAlign::Center),
                    );
                #[cfg(test)]
                let entry = entry.debug_selector(|| "number-field-entry".into());
                return base.child(entry).into_any_element();
            }
            if self.read_only {
                return base
                    .px(px(10.0 * z))
                    .gap(px(8.0 * z))
                    .rounded(theme.radius.lg * z)
                    .border(px(z))
                    .border_color(c.input)
                    .opacity(0.5)
                    .children(
                        self.label
                            .clone()
                            .map(|label| div().text_color(c.muted_foreground).child(label)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_color(c.foreground)
                            .child(self.format()),
                    )
                    .into_any_element();
            }
            let field = base
                .id("number-field")
                .px(px(10.0 * z))
                .gap(px(8.0 * z))
                .rounded(theme.radius.lg * z)
                .border(px(z))
                .border_color(c.input)
                .when(theme.is_dark(), |d| d.bg(c.input.opacity(0.3)))
                .cursor(CursorStyle::ResizeLeftRight)
                .hover(|s| s.border_color(c.ring))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, event: &MouseDownEvent, _, _| {
                        this.scrub = Some((event.position.x.into(), this.value));
                        this.moved = false;
                    }),
                )
                .on_drag(Scrub, |_, _, _, cx| cx.new(|_| Empty))
                .on_drag_move::<Scrub>(cx.safe_listener(
                    |this, event: &DragMoveEvent<Scrub>, _, cx| {
                        this.scrub_to(
                            event.event.position.x.into(),
                            event.event.modifiers.shift,
                            cx,
                        );
                    },
                ))
                .on_click(cx.safe_listener(|this, _: &ClickEvent, window, cx| {
                    let scrubbed = std::mem::take(&mut this.moved);
                    this.scrub = None;
                    if !scrubbed {
                        this.start_editing(window, cx);
                    }
                }))
                .children(
                    self.label
                        .clone()
                        .map(|label| div().text_color(c.muted_foreground).child(label)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_right()
                        .text_color(c.foreground)
                        .child(self.format()),
                );
            #[cfg(test)]
            let field = field.debug_selector(|| "number-field".into());
            field.into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Render, TestAppContext, VisualTestContext};

    use super::*;

    #[test]
    fn scrubbing_stays_within_the_soft_range() {
        assert_eq!(soft_clamp(1.7, 0.5, Some(0.0), Some(1.0)), 1.0);
        assert_eq!(soft_clamp(-0.4, 0.5, Some(0.0), Some(1.0)), 0.0);
        assert_eq!(soft_clamp(0.3, 0.5, Some(0.0), Some(1.0)), 0.3);
    }

    #[test]
    fn a_value_past_the_soft_range_can_still_be_scrubbed_back_and_forth() {
        // Typed as 3: scrubbing may go anywhere between the range's ends and 3.
        assert_eq!(soft_clamp(2.0, 3.0, Some(0.0), Some(1.0)), 2.0);
        assert_eq!(soft_clamp(5.0, 3.0, Some(0.0), Some(1.0)), 3.0);
        assert_eq!(soft_clamp(-1.0, 3.0, Some(0.0), Some(1.0)), 0.0);
    }

    #[test]
    fn without_a_soft_range_nothing_is_held() {
        assert_eq!(soft_clamp(9.0, 0.0, None, None), 9.0);
    }

    struct Host(Entity<NumberField>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            guard::render("number field host", || div().w(px(200.0)).child(self.0.clone()))
        }
    }

    type Changes = Rc<RefCell<Vec<f64>>>;

    fn setup(
        cx: &mut TestAppContext,
        make: impl FnOnce(&mut Context<NumberField>) -> NumberField,
    ) -> (Entity<NumberField>, Changes, &mut VisualTestContext) {
        cx.update(crate::init);
        let field = cx.update(|cx| cx.new(make));
        let changes = Rc::new(RefCell::new(Vec::new()));
        let log = changes.clone();
        let (_, cx) = cx.add_window_view(|_, _| Host(field.clone()));
        cx.update(|_, cx| {
            cx.subscribe(&field, move |_, event: &NumberEvent, _| {
                let NumberEvent::Change(value) = *event;
                log.borrow_mut().push(value);
            })
            .detach();
        });
        cx.run_until_parked();
        (field, changes, cx)
    }

    fn edit(field: &Entity<NumberField>, cx: &mut VisualTestContext, text: &str, key: &str) {
        cx.update(|window, cx| field.update(cx, |f, cx| f.start_editing(window, cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input(text);
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn typing_a_number_and_pressing_enter_commits_it(cx: &mut TestAppContext) {
        let (field, changes, cx) = setup(cx, |cx| NumberField::new(1.0, cx));
        edit(&field, cx, "7.5", "enter");
        assert_eq!(*changes.borrow(), vec![7.5]);
    }

    #[gpui::test]
    fn typed_values_are_held_to_the_range(cx: &mut TestAppContext) {
        let (field, changes, cx) = setup(cx, |cx| NumberField::new(1.0, cx).min(0.0).max(10.0));
        edit(&field, cx, "99", "enter");
        assert_eq!(*changes.borrow(), vec![10.0]);
    }

    #[gpui::test]
    fn whole_numbers_round_what_is_typed(cx: &mut TestAppContext) {
        let (field, changes, cx) = setup(cx, |cx| NumberField::new(1.0, cx).integer());
        edit(&field, cx, "2.6", "enter");
        assert_eq!(*changes.borrow(), vec![3.0]);
    }

    #[gpui::test]
    fn the_field_and_its_typed_entry_scale_with_the_zoom(cx: &mut TestAppContext) {
        let (field, _, cx) = setup(cx, |cx| NumberField::new(1.0, cx));
        let height_at = |cx: &mut VisualTestContext, zoom: f32| {
            cx.update(|_, cx| field.update(cx, |f, cx| f.set_zoom(zoom, cx)));
            cx.run_until_parked();
            cx.debug_bounds("number-field").expect("the field is drawn").size.height
        };
        let (small, large) = (height_at(cx, 0.5), height_at(cx, 2.0));
        assert_eq!((small, large), (px(16.0), px(64.0)));
        cx.update(|window, cx| field.update(cx, |f, cx| f.start_editing(window, cx)));
        cx.run_until_parked();
        let entry = cx.debug_bounds("number-field-entry").expect("the entry is drawn");
        assert_eq!(entry.size.height, px(64.0));
        cx.update(|_, cx| field.update(cx, |f, cx| f.set_zoom(0.5, cx)));
        cx.run_until_parked();
        let entry = cx.debug_bounds("number-field-entry").expect("the entry is drawn");
        assert_eq!(entry.size.height, px(16.0));
    }

    #[gpui::test]
    fn escape_cancels_the_edit(cx: &mut TestAppContext) {
        let (field, changes, cx) = setup(cx, |cx| NumberField::new(1.0, cx));
        edit(&field, cx, "7", "escape");
        assert!(changes.borrow().is_empty());
        assert!(!cx.read(|cx| field.read(cx).editing));
    }
}
