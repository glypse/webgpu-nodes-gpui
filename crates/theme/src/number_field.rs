use guard::SafeListener as _;
use gpui::{
    AppContext as _, ClickEvent, Context, CursorStyle, DragMoveEvent, Empty, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{ActiveTheme as _, InputEvent, TextInput};

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
    min: Option<f64>,
    max: Option<f64>,
    input: Entity<TextInput>,
    editing: bool,
    scrub: Option<(f32, f64)>,
    moved: bool,
    _subscription: Subscription,
}

impl EventEmitter<NumberEvent> for NumberField {}

impl NumberField {
    pub fn new(value: f64, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Submit | InputEvent::Blur => this.commit(cx),
            InputEvent::Cancel => this.stop_editing(cx),
            InputEvent::Change => {}
        });
        Self {
            label: None,
            value,
            step: 0.1,
            integer: false,
            min: None,
            max: None,
            input,
            editing: false,
            scrub: None,
            moved: false,
            _subscription: subscription,
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

    pub fn value(&self) -> f64 {
        self.value
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
        self.input.update(cx, |input, cx| {
            input.set_value(text, cx);
            input.select_all_text(cx);
            input.focus(window);
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
        if let Ok(parsed) = self.input.read(cx).value().trim().parse::<f64>() {
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
        self.change(start_value + dx / PIXELS_PER_STEP * step, cx);
    }
}

impl Render for NumberField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let base = div()
            .flex()
            .items_center()
            .h(px(32.0))
            .w_full()
            .min_w_0()
            .font_family(theme.font.clone())
            .text_size(theme.text.sm);
        if self.editing {
            return base.child(self.input.clone()).into_any_element();
        }
        base.id("number-field")
            .px(px(10.0))
            .gap_2()
            .rounded(theme.radius.lg)
            .border_1()
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
            .on_drag_move::<Scrub>(cx.safe_listener(|this, event: &DragMoveEvent<Scrub>, _, cx| {
                this.scrub_to(
                    event.event.position.x.into(),
                    event.event.modifiers.shift,
                    cx,
                );
            }))
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
            )
            .into_any_element()
    }
}
