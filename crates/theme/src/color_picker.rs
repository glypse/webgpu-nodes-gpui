use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use graph_math::color::{
    ColorMode, ColorSpace, Hsv, Lch, Rgb, Rgba as LinearRgba, clip_to_gamut, decode_rgb, encode_in,
    hex_to_linear, hsv_keeping_hue, hsv_to_rgb, lch_keeping_hue, linear_to_hex, oklch_to_linear,
};

use crate::{
    ActiveTheme as _, ColorWheels, ColorWheelsEvent, InputEvent, NumberEvent, NumberField,
    TextInput,
};

/// The mode last shown, kept for the next picker opened.
static LAST_MODE: AtomicU8 = AtomicU8::new(1);

fn mode_from_index(index: u8) -> ColorMode {
    ColorMode::ALL
        .get(usize::from(index))
        .copied()
        .unwrap_or(ColorMode::Hsv)
}

fn index_of(mode: ColorMode) -> u8 {
    ColorMode::ALL
        .iter()
        .position(|m| *m == mode)
        .map_or(1, |i| i as u8)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorPickerEvent {
    /// A new linear RGB colour and alpha.
    Change(LinearRgba),
}

const EPSILON: f64 = 1e-6;
const MODE_LABELS: [(ColorMode, &str); 3] = [
    (ColorMode::Rgb, "RGB"),
    (ColorMode::Hsv, "HSV"),
    (ColorMode::Oklch, "OKLch"),
];

fn same_color(a: LinearRgba, b: LinearRgba) -> bool {
    (a.r - b.r).abs() < EPSILON
        && (a.g - b.g).abs() < EPSILON
        && (a.b - b.b).abs() < EPSILON
        && (a.a - b.a).abs() < EPSILON
}

/// A colour on screen: encoded in `space`, past what the screen can show clipped.
pub fn linear_to_hsla(color: Rgb, space: ColorSpace) -> Hsla {
    let Rgb { r, g, b } = encode_in(clip_to_gamut(color, space), space);
    Rgba {
        r: r as f32,
        g: g as f32,
        b: b as f32,
        a: 1.0,
    }
    .into()
}

/// A colour picker: a wheel and bar, fields for the components in RGB, HSV or
/// OKLch, and a hex field. Colours are linear RGB.
pub struct ColorPicker {
    value: LinearRgba,
    alpha: bool,
    gamut: ColorSpace,
    mode: ColorMode,
    /// HSV and OKLch are kept apart from `value` so a hue survives going
    /// through grey (no hue) or black, as it does in Blender.
    hsv: Hsv,
    lch: Lch,
    /// The last value sent, to tell our own changes coming back from new ones.
    sent: Option<LinearRgba>,
    wheels: Entity<ColorWheels>,
    fields: [Entity<NumberField>; 4],
    hex: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ColorPickerEvent> for ColorPicker {}

impl ColorPicker {
    pub fn new(value: LinearRgba, cx: &mut Context<Self>) -> Self {
        let mode = mode_from_index(LAST_MODE.load(Ordering::Relaxed));
        let hsv = hsv_keeping_hue(value.rgb(), Hsv::default());
        let lch = lch_keeping_hue(value.rgb(), Lch::default());
        let wheels = cx.new(|_| ColorWheels::new(mode, [0.0; 3]));
        let fields = [(); 4].map(|()| cx.new(|cx| NumberField::new(0.0, cx).step(0.01)));
        let hex = cx.new(TextInput::new);
        let mut subscriptions = vec![cx.subscribe(&wheels, |this, _, event, cx| {
            let ColorWheelsEvent::Change(components) = event;
            this.set_components(*components, cx);
        })];
        for (index, field) in fields.iter().enumerate() {
            subscriptions.push(cx.subscribe(field, move |this, _, event, cx| {
                let NumberEvent::Change(number) = event;
                this.set_field(index, *number, cx);
            }));
        }
        subscriptions.push(cx.subscribe(&hex, |this, _, event, cx| match event {
            InputEvent::Submit | InputEvent::Blur => this.commit_hex(cx),
            InputEvent::Cancel => this.refresh(cx),
            InputEvent::Change => {}
        }));
        let mut picker = Self {
            value,
            alpha: false,
            gamut: ColorSpace::Srgb,
            mode,
            hsv,
            lch,
            sent: None,
            wheels,
            fields,
            hex,
            _subscriptions: subscriptions,
        };
        picker.configure_fields(cx);
        picker.refresh(cx);
        picker
    }

    /// Edit the alpha too.
    pub fn alpha(mut self, alpha: bool) -> Self {
        self.alpha = alpha;
        self
    }

    pub fn gamut(mut self, gamut: ColorSpace) -> Self {
        self.gamut = gamut;
        self
    }

    pub fn value(&self) -> LinearRgba {
        self.value
    }

    pub fn mode(&self) -> ColorMode {
        self.mode
    }

    /// Shows a colour, without emitting a change.
    pub fn set_value(&mut self, value: LinearRgba, cx: &mut Context<Self>) {
        if self.sent.is_some_and(|sent| same_color(sent, value)) {
            self.value = value;
            return;
        }
        self.value = value;
        self.hsv = hsv_keeping_hue(value.rgb(), self.hsv);
        self.lch = lch_keeping_hue(value.rgb(), self.lch);
        self.refresh(cx);
    }

    pub fn set_alpha_shown(&mut self, alpha: bool, cx: &mut Context<Self>) {
        if self.alpha != alpha {
            self.alpha = alpha;
            cx.notify();
        }
    }

    pub fn set_gamut(&mut self, gamut: ColorSpace, cx: &mut Context<Self>) {
        if self.gamut != gamut {
            self.gamut = gamut;
            self.wheels
                .update(cx, |wheels, cx| wheels.set_gamut(gamut, cx));
            self.refresh(cx);
        }
    }

    pub fn set_mode(&mut self, mode: ColorMode, cx: &mut Context<Self>) {
        self.mode = mode;
        LAST_MODE.store(index_of(mode), Ordering::Relaxed);
        self.configure_fields(cx);
        self.refresh(cx);
    }

    /// The three components in the mode shown.
    fn components(&self) -> [f64; 3] {
        match self.mode {
            ColorMode::Rgb => [self.value.r, self.value.g, self.value.b],
            ColorMode::Hsv => [self.hsv.h, self.hsv.s, self.hsv.v],
            ColorMode::Oklch => [self.lch.l, self.lch.c, self.lch.h],
        }
    }

    /// The labels and ranges of the fields in the mode shown.
    fn configure_fields(&mut self, cx: &mut Context<Self>) {
        let specs: [(&str, Option<f64>, Option<f64>); 4] = match self.mode {
            ColorMode::Rgb => [
                ("R", Some(0.0), None),
                ("G", Some(0.0), None),
                ("B", Some(0.0), None),
                ("A", Some(0.0), Some(1.0)),
            ],
            ColorMode::Hsv => [
                ("H", Some(0.0), Some(1.0)),
                ("S", Some(0.0), Some(1.0)),
                ("V", Some(0.0), Some(1.0)),
                ("A", Some(0.0), Some(1.0)),
            ],
            ColorMode::Oklch => [
                ("L", Some(0.0), Some(1.0)),
                ("C", Some(0.0), Some(0.5)),
                ("H", Some(0.0), Some(360.0)),
                ("A", Some(0.0), Some(1.0)),
            ],
        };
        for (field, (label, min, max)) in self.fields.iter().zip(specs) {
            field.update(cx, |field, cx| field.configure(label, min, max, cx));
        }
    }

    /// Shows `value` and what is kept beside it in every part.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let components = self.components();
        for (field, number) in self
            .fields
            .iter()
            .zip(components.into_iter().chain([self.value.a]))
        {
            field.update(cx, |field, cx| field.set_value(number, cx));
        }
        let mode = self.mode;
        self.wheels
            .update(cx, |wheels, cx| wheels.set_components(mode, components, cx));
        let hex = linear_to_hex(self.value, self.alpha, self.gamut);
        self.hex.update(cx, |input, cx| input.set_value(hex, cx));
        cx.notify();
    }

    fn commit(
        &mut self,
        next: LinearRgba,
        keep: (Option<Hsv>, Option<Lch>),
        cx: &mut Context<Self>,
    ) {
        self.hsv = keep
            .0
            .unwrap_or_else(|| hsv_keeping_hue(next.rgb(), self.hsv));
        self.lch = keep
            .1
            .unwrap_or_else(|| lch_keeping_hue(next.rgb(), self.lch));
        self.sent = Some(next);
        self.value = next;
        self.refresh(cx);
        cx.emit(ColorPickerEvent::Change(next));
    }

    fn set_hsv(&mut self, hsv: Hsv, cx: &mut Context<Self>) {
        let Rgb { r, g, b } = decode_rgb(hsv_to_rgb(hsv));
        self.commit(
            LinearRgba::new(r, g, b, self.value.a),
            (Some(hsv), None),
            cx,
        );
    }

    fn set_lch(&mut self, lch: Lch, cx: &mut Context<Self>) {
        let Rgb { r, g, b } = oklch_to_linear(lch, self.gamut);
        self.commit(
            LinearRgba::new(r, g, b, self.value.a),
            (None, Some(lch)),
            cx,
        );
    }

    fn set_components(&mut self, [x, y, z]: [f64; 3], cx: &mut Context<Self>) {
        match self.mode {
            ColorMode::Rgb => self.commit(LinearRgba::new(x, y, z, self.value.a), (None, None), cx),
            ColorMode::Hsv => self.set_hsv(Hsv { h: x, s: y, v: z }, cx),
            ColorMode::Oklch => self.set_lch(Lch { l: x, c: y, h: z }, cx),
        }
    }

    fn set_field(&mut self, index: usize, number: f64, cx: &mut Context<Self>) {
        if index == 3 {
            let value = LinearRgba {
                a: number.clamp(0.0, 1.0),
                ..self.value
            };
            self.commit(value, (Some(self.hsv), Some(self.lch)), cx);
            return;
        }
        let mut components = self.components();
        components[index] = match self.mode {
            ColorMode::Rgb => number.max(0.0),
            ColorMode::Hsv => number.clamp(0.0, 1.0),
            ColorMode::Oklch => number.clamp(0.0, [1.0, 0.5, 360.0][index]),
        };
        self.set_components(components, cx);
    }

    fn commit_hex(&mut self, cx: &mut Context<Self>) {
        let text = self.hex.read(cx).value().to_string();
        if let Some(parsed) = hex_to_linear(&text, self.gamut) {
            let digits = text.trim().trim_start_matches('#').len();
            let with_alpha = self.alpha && (digits == 4 || digits == 8);
            let alpha = if with_alpha { parsed.a } else { self.value.a };
            let next = LinearRgba { a: alpha, ..parsed };
            if !same_color(next, self.value) {
                self.commit(next, (None, None), cx);
                return;
            }
        }
        self.refresh(cx);
    }
}

impl Render for ColorPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("ColorPicker", || {
            let theme = cx.theme();
            let c = &theme.colors;
            let mode = self.mode;
            let (primary, primary_fg, muted_fg, foreground) = (
                c.primary,
                c.primary_foreground,
                c.muted_foreground,
                c.foreground,
            );
            let tabs = div()
                .flex()
                .rounded_md()
                .p(px(2.0))
                .bg(c.muted)
                .children(MODE_LABELS.map(|(option, label)| {
                    let selected = option == mode;
                    let tab = div()
                        .id(SharedString::from(format!("picker-tab-{label}")))
                        .flex_1()
                        .flex()
                        .justify_center()
                        .rounded_sm()
                        .py(px(2.0))
                        .text_size(theme.text.xs)
                        .cursor_pointer()
                        .when(selected, |d| d.bg(primary).text_color(primary_fg))
                        .when(!selected, |d| {
                            d.text_color(muted_fg)
                                .hover(move |s| s.text_color(foreground))
                        })
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_mode(option, cx)));
                    #[cfg(test)]
                    let tab = gpui::InteractiveElement::debug_selector(tab, || {
                        format!("picker-tab-{label}")
                    });
                    tab
                }));
            let shown = if self.alpha { 4 } else { 3 };
            let fields = div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .children(self.fields.iter().take(shown).cloned());
            let swatch = div()
                .flex_none()
                .size(px(28.0))
                .rounded_sm()
                .border_1()
                .border_color(gpui::black().opacity(0.2))
                .bg(linear_to_hsla(self.value.rgb(), self.gamut));
            let hex = div().flex_1().min_w_0().child(self.hex.clone());
            #[cfg(test)]
            let hex = gpui::InteractiveElement::debug_selector(hex, || "picker-hex".into());
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w(px(176.0))
                .font_family(theme.font.clone())
                .child(div().flex().justify_center().child(self.wheels.clone()))
                .child(tabs)
                .child(fields)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(swatch)
                        .child(hex),
                )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use graph_math::color::{linear_to_oklch, srgb_decode};

    use super::*;

    type Changes = Rc<RefCell<Vec<LinearRgba>>>;

    fn setup(
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Context<ColorPicker>) -> ColorPicker + 'static,
    ) -> (Entity<ColorPicker>, Changes, &mut VisualTestContext) {
        cx.update(crate::init);
        let (picker, cx) = cx.add_window_view(|_, cx| build(cx));
        let changes: Changes = Rc::default();
        let log = changes.clone();
        cx.update(|_, cx| {
            cx.subscribe(&picker, move |_, event: &ColorPickerEvent, _| {
                let ColorPickerEvent::Change(color) = event;
                log.borrow_mut().push(*color);
            })
            .detach();
        });
        cx.run_until_parked();
        (picker, changes, cx)
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }

    fn color(r: f64, g: f64, b: f64, a: f64) -> LinearRgba {
        LinearRgba::new(r, g, b, a)
    }

    fn type_hex(cx: &mut VisualTestContext, text: &str) {
        let hex = cx
            .debug_bounds("picker-hex")
            .expect("the hex field is drawn");
        cx.simulate_click(hex.center(), Modifiers::default());
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input(text);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_typed_hex_sets_the_colour_encoded_in_the_gamut(cx: &mut TestAppContext) {
        let (picker, changes, cx) = setup(cx, |cx| ColorPicker::new(color(0.0, 0.0, 0.0, 1.0), cx));
        type_hex(cx, "#ff0080");
        let sent = *changes.borrow().last().expect("a change");
        near(sent.r, 1.0);
        near(sent.g, 0.0);
        near(sent.b, srgb_decode(128.0 / 255.0));
        assert_eq!(cx.read(|cx| picker.read(cx).value()), sent);
    }

    #[gpui::test]
    fn text_that_isnt_a_colour_changes_nothing_and_is_replaced_by_the_hex(cx: &mut TestAppContext) {
        let (picker, changes, cx) = setup(cx, |cx| ColorPicker::new(color(1.0, 1.0, 1.0, 1.0), cx));
        type_hex(cx, "nothex");
        assert!(changes.borrow().is_empty());
        let shown = cx.read(|cx| picker.read(cx).hex.read(cx).value().to_string());
        assert_eq!(shown, "#FFFFFF");
    }

    #[gpui::test]
    fn alpha_is_only_taken_from_hex_when_edited(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, |cx| ColorPicker::new(color(0.0, 0.0, 0.0, 0.25), cx));
        type_hex(cx, "#FF000080");
        // Without alpha editing the picker keeps its own.
        near(changes.borrow().last().expect("a change").a, 0.25);
        let (_, changes, cx) = setup(cx, |cx| {
            ColorPicker::new(color(0.0, 0.0, 0.0, 0.25), cx).alpha(true)
        });
        type_hex(cx, "#FF000080");
        near(changes.borrow().last().expect("a change").a, 128.0 / 255.0);
    }

    #[gpui::test]
    fn the_modes_show_the_same_colour(cx: &mut TestAppContext) {
        let value = color(0.2, 0.5, 0.1, 1.0);
        let (picker, _, cx) = setup(cx, move |cx| ColorPicker::new(value, cx));
        cx.update(|_, cx| picker.update(cx, |p, cx| p.set_mode(ColorMode::Oklch, cx)));
        let shown = cx.read(|cx| picker.read(cx).components());
        let lch = linear_to_oklch(value.rgb());
        near(shown[0], lch.l);
        near(shown[1], lch.c);
        near(shown[2], lch.h);
        cx.update(|_, cx| picker.update(cx, |p, cx| p.set_mode(ColorMode::Rgb, cx)));
        assert_eq!(cx.read(|cx| picker.read(cx).components()), [0.2, 0.5, 0.1]);
    }

    #[gpui::test]
    fn clicking_a_tab_changes_the_mode(cx: &mut TestAppContext) {
        let (picker, changes, cx) = setup(cx, |cx| ColorPicker::new(color(0.2, 0.5, 0.1, 1.0), cx));
        let tab = cx
            .debug_bounds("picker-tab-OKLch")
            .expect("the tab is drawn");
        cx.simulate_click(tab.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| picker.read(cx).mode()), ColorMode::Oklch);
        assert!(
            changes.borrow().is_empty(),
            "showing another mode isn't an edit"
        );
    }

    #[gpui::test]
    fn picking_on_the_wheel_changes_the_colour(cx: &mut TestAppContext) {
        let (picker, changes, cx) = setup(cx, |cx| {
            let mut picker = ColorPicker::new(color(0.5, 0.5, 0.5, 1.0), cx);
            picker.mode = ColorMode::Hsv;
            picker
        });
        cx.update(|_, cx| picker.update(cx, |p, cx| p.set_mode(ColorMode::Hsv, cx)));
        cx.run_until_parked();
        let wheel = cx.debug_bounds("color-wheel").expect("the wheel is drawn");
        // The rim to the left: hue 0.25 at full saturation.
        cx.simulate_mouse_down(
            gpui::point(wheel.left() + gpui::px(1.0), wheel.center().y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let sent = *changes.borrow().last().expect("a change");
        let expected = decode_rgb(hsv_to_rgb(Hsv {
            h: 0.25,
            s: 1.0 - 1.0 / 64.0,
            v: picker_value(cx, &picker),
        }));
        near(sent.r, expected.r);
        near(sent.g, expected.g);
        near(sent.b, expected.b);
    }

    fn picker_value(cx: &mut VisualTestContext, picker: &Entity<ColorPicker>) -> f64 {
        cx.read(|cx| picker.read(cx).hsv.v)
    }

    #[gpui::test]
    fn a_hue_survives_the_colour_going_grey_from_outside(cx: &mut TestAppContext) {
        let (picker, _, cx) = setup(cx, |cx| ColorPicker::new(color(1.0, 0.0, 0.0, 1.0), cx));
        let hue = cx.read(|cx| picker.read(cx).hsv.h);
        cx.update(|_, cx| picker.update(cx, |p, cx| p.set_value(color(0.2, 0.2, 0.2, 1.0), cx)));
        near(cx.read(|cx| picker.read(cx).hsv.h), hue);
        let lch_hue = cx.read(|cx| picker.read(cx).lch.h);
        assert!(lch_hue > 0.0, "the OKLch hue of red is kept too");
    }

    #[gpui::test]
    fn setting_what_it_just_sent_doesnt_redo_the_conversion(cx: &mut TestAppContext) {
        let (picker, changes, cx) = setup(cx, |cx| ColorPicker::new(color(0.0, 0.0, 0.0, 1.0), cx));
        type_hex(cx, "#336699");
        let sent = *changes.borrow().last().expect("a change");
        let kept = cx.read(|cx| picker.read(cx).hsv);
        cx.update(|_, cx| picker.update(cx, |p, cx| p.set_value(sent, cx)));
        assert_eq!(cx.read(|cx| picker.read(cx).hsv), kept);
    }
}
