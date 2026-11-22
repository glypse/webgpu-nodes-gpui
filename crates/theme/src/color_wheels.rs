use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

use gpui::{
    AppContext as _, Bounds, Context, Corners, DragMoveEvent, Empty, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, RenderImage, Rgba, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};
use graph_math::color::{
    ColorMode, ColorSpace, Hsv, Lch, Rgb, decode_rgb, encode_in, hsv_keeping_hue, hsv_to_rgb,
    oklch_to_linear,
};
use guard::SafeListener as _;
use image::Frame;

use crate::gamut_image::paint_gamut;

/// Red at the bottom, hues going clockwise (as Blender's wheel).
const HUE_OFFSET: f64 = PI;
/// A little over the most chroma each gamut reaches (about 0.32 and 0.36).
pub fn oklch_max_chroma(gamut: ColorSpace) -> f64 {
    match gamut {
        ColorSpace::Srgb => 0.33,
        ColorSpace::DisplayP3 => 0.37,
    }
}

/// Pixels per side of the image a wheel is painted from, a multiple of its
/// size on screen so it stays sharp on dense displays.
const WHEEL_PIXELS: u32 = 256;
/// The bar's width, the wheel's marker and the bar's knob at zoom 1.
const BAR_WIDTH: f32 = 12.0;
const MARKER: f32 = 14.0;
const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorWheelsEvent {
    /// All three components; locked ones as they were.
    Change([f64; 3]),
}

/// The hue (in turns) and the distance from the centre (clamped to 1) at a
/// point of a wheel, given as its offset from the centre in half-sizes.
pub fn wheel_polar(dx: f64, dy: f64) -> (f64, f64) {
    let turn = (dx.atan2(-dy) - HUE_OFFSET) / (2.0 * PI);
    (turn - turn.floor(), dx.hypot(dy).min(1.0))
}

/// Where a hue (in turns) at `radius` is on a wheel, as offsets from its centre in half-sizes.
pub fn wheel_cursor(turns: f64, radius: f64) -> (f64, f64) {
    let angle = turns * 2.0 * PI + HUE_OFFSET;
    (angle.sin() * radius, -angle.cos() * radius)
}

fn encoded(rgb: Rgb) -> Hsla {
    let c = |n: f64| n.clamp(0.0, 1.0) as f32;
    Rgba {
        r: c(rgb.r),
        g: c(rgb.g),
        b: c(rgb.b),
        a: 1.0,
    }
    .into()
}

/// The hue and saturation wheel at full value, whose pixels are sRGB-encoded.
fn hsv_wheel() -> Arc<RenderImage> {
    static WHEEL: OnceLock<Arc<RenderImage>> = OnceLock::new();
    WHEEL
        .get_or_init(|| {
            let n = WHEEL_PIXELS;
            let mut image = image::RgbaImage::new(n, n);
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                let dx = (f64::from(x) + 0.5) / f64::from(n) * 2.0 - 1.0;
                let dy = (f64::from(y) + 0.5) / f64::from(n) * 2.0 - 1.0;
                let (h, s) = wheel_polar(dx, dy);
                let Rgb { r, g, b } = hsv_to_rgb(Hsv { h, s, v: 1.0 });
                let byte = |n: f64| (n.clamp(0.0, 1.0) * 255.0).round() as u8;
                *pixel = image::Rgba([byte(b), byte(g), byte(r), 255]);
            }
            Arc::new(RenderImage::new(vec![Frame::new(image)]))
        })
        .clone()
}

#[derive(Clone, Copy, PartialEq)]
struct WheelKey {
    lightness: i32,
    gamut: ColorSpace,
}

#[derive(Clone, Copy, PartialEq)]
struct BarKey {
    chroma: i32,
    hue: i32,
    gamut: ColorSpace,
}

#[derive(Clone, Copy)]
enum Surface {
    Wheel,
    Bar,
}

struct Pick(Surface);

/// A hue / saturation (or hue / chroma) wheel and a value (or lightness) bar,
/// editing the three components of a colour in `mode`.
pub struct ColorWheels {
    mode: ColorMode,
    components: [f64; 3],
    locked: [bool; 3],
    disabled: bool,
    gamut: ColorSpace,
    size: Pixels,
    /// Scales the bar and the markers; the wheel's diameter is `size`.
    zoom: f32,
    /// In RGB the wheel and bar edit an HSV kept apart from the components,
    /// so a hue survives going through grey or black.
    hsv: Hsv,
    /// The RGB last sent, to tell our own edits coming back from new ones.
    sent: Option<[f64; 3]>,
    wheel_bounds: Bounds<Pixels>,
    bar_bounds: Bounds<Pixels>,
    wheel_image: Option<(WheelKey, Arc<RenderImage>)>,
    bar_image: Option<(BarKey, Arc<RenderImage>)>,
    retired: Vec<Arc<RenderImage>>,
}

impl EventEmitter<ColorWheelsEvent> for ColorWheels {}

impl ColorWheels {
    pub fn new(mode: ColorMode, components: [f64; 3]) -> Self {
        let mut wheels = Self {
            mode,
            components,
            locked: [false; 3],
            disabled: false,
            gamut: ColorSpace::Srgb,
            size: px(128.0),
            zoom: 1.0,
            hsv: Hsv::default(),
            sent: None,
            wheel_bounds: Bounds::default(),
            bar_bounds: Bounds::default(),
            wheel_image: None,
            bar_image: None,
            retired: Vec::new(),
        };
        wheels.follow_rgb();
        wheels
    }

    /// The diameter of the wheel.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn set_size(&mut self, size: Pixels, cx: &mut Context<Self>) {
        if self.size != size {
            self.size = size;
            cx.notify();
        }
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if self.zoom != zoom {
            self.zoom = zoom;
            cx.notify();
        }
    }

    pub fn gamut(mut self, gamut: ColorSpace) -> Self {
        self.gamut = gamut;
        self
    }

    /// Components not to edit, e.g. linked ones.
    pub fn locked(mut self, locked: [bool; 3]) -> Self {
        self.locked = locked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn components(&self) -> [f64; 3] {
        self.components
    }

    /// Shows a colour, without emitting a change. `mode` is what the three
    /// numbers are: linear RGB, HSV (hue in [0, 1]) or OKLch (hue in degrees).
    pub fn set_components(
        &mut self,
        mode: ColorMode,
        components: [f64; 3],
        cx: &mut Context<Self>,
    ) {
        self.mode = mode;
        self.components = components;
        self.follow_rgb();
        cx.notify();
    }

    pub fn set_locked(&mut self, locked: [bool; 3], disabled: bool, cx: &mut Context<Self>) {
        if self.locked != locked || self.disabled != disabled {
            self.locked = locked;
            self.disabled = disabled;
            cx.notify();
        }
    }

    pub fn set_gamut(&mut self, gamut: ColorSpace, cx: &mut Context<Self>) {
        if self.gamut != gamut {
            self.gamut = gamut;
            cx.notify();
        }
    }

    /// In RGB, the HSV of a colour that isn't one this sent itself.
    fn follow_rgb(&mut self) {
        if self.mode != ColorMode::Rgb {
            return;
        }
        let ours = self.sent.is_some_and(|sent| {
            sent.iter()
                .zip(self.components)
                .all(|(a, b)| (a - b).abs() < EPSILON)
        });
        if !ours {
            let [r, g, b] = self.components;
            self.hsv = hsv_keeping_hue(Rgb::new(r, g, b), self.hsv);
        }
    }

    fn any_locked(&self) -> bool {
        self.locked.iter().any(|l| *l)
    }

    /// Each control's own components: locked only when all of them are. In RGB
    /// both rewrite all three, which means nothing once one is locked.
    fn wheel_disabled(&self) -> bool {
        let [x, y, z] = self.locked;
        self.disabled
            || match self.mode {
                ColorMode::Rgb => self.any_locked(),
                ColorMode::Hsv => x && y,
                ColorMode::Oklch => y && z,
            }
    }

    fn bar_disabled(&self) -> bool {
        let [x, _, z] = self.locked;
        self.disabled
            || match self.mode {
                ColorMode::Rgb => self.any_locked(),
                ColorMode::Hsv => z,
                ColorMode::Oklch => x,
            }
    }

    /// The components with `patch` applied, locked ones left as they are.
    fn patched(&self, patch: [Option<f64>; 3]) -> [f64; 3] {
        std::array::from_fn(|i| {
            if self.locked[i] {
                self.components[i]
            } else {
                patch[i].unwrap_or(self.components[i])
            }
        })
    }

    fn set_rgb_hsv(&mut self, hsv: Hsv, cx: &mut Context<Self>) {
        self.hsv = hsv;
        let Rgb { r, g, b } = decode_rgb(hsv_to_rgb(hsv));
        self.components = [r, g, b];
        self.sent = Some(self.components);
        cx.emit(ColorWheelsEvent::Change(self.components));
        cx.notify();
    }

    fn send(&mut self, components: [f64; 3], cx: &mut Context<Self>) {
        self.components = components;
        cx.emit(ColorWheelsEvent::Change(components));
        cx.notify();
    }

    fn hsv_shown(&self) -> Hsv {
        match self.mode {
            ColorMode::Rgb => self.hsv,
            _ => Hsv {
                h: self.components[0],
                s: self.components[1],
                v: self.components[2],
            },
        }
    }

    fn lch_shown(&self) -> Lch {
        Lch {
            l: self.components[0],
            c: self.components[1],
            h: self.components[2],
        }
    }

    fn pick(&mut self, surface: Surface, position: Point<Pixels>, cx: &mut Context<Self>) {
        match surface {
            Surface::Wheel if !self.wheel_disabled() => self.pick_wheel(position, cx),
            Surface::Bar if !self.bar_disabled() => self.pick_bar(position, cx),
            _ => {}
        }
    }

    fn pick_wheel(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let b = self.wheel_bounds;
        if b.size.width <= px(0.0) {
            return;
        }
        let dx = f64::from((position.x - b.left()) / (b.size.width / 2.0)) - 1.0;
        let dy = f64::from((position.y - b.top()) / (b.size.height / 2.0)) - 1.0;
        let (h, radius) = wheel_polar(dx, dy);
        match self.mode {
            ColorMode::Oklch => {
                let chroma = radius * oklch_max_chroma(self.gamut);
                self.send(self.patched([None, Some(chroma), Some(h * 360.0)]), cx);
            }
            ColorMode::Hsv => self.send(self.patched([Some(h), Some(radius), None]), cx),
            ColorMode::Rgb => self.set_rgb_hsv(
                Hsv {
                    h,
                    s: radius,
                    ..self.hsv
                },
                cx,
            ),
        }
    }

    fn pick_bar(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let b = self.bar_bounds;
        if b.size.height <= px(0.0) {
            return;
        }
        let level = (1.0 - f64::from((position.y - b.top()) / b.size.height)).clamp(0.0, 1.0);
        match self.mode {
            ColorMode::Oklch => self.send(self.patched([Some(level), None, None]), cx),
            ColorMode::Hsv => self.send(self.patched([None, None, Some(level)]), cx),
            ColorMode::Rgb => self.set_rgb_hsv(
                Hsv {
                    v: level,
                    ..self.hsv
                },
                cx,
            ),
        }
    }

    fn retire(&mut self, image: Arc<RenderImage>) {
        self.retired.push(image);
    }

    fn oklch_wheel_image(&mut self) -> Arc<RenderImage> {
        let key = WheelKey {
            lightness: (self.lch_shown().l * 400.0).round() as i32,
            gamut: self.gamut,
        };
        if let Some((cached, image)) = &self.wheel_image
            && *cached == key
        {
            return image.clone();
        }
        let lightness = f64::from(key.lightness) / 400.0;
        let chroma = oklch_max_chroma(self.gamut);
        let frame = paint_gamut(WHEEL_PIXELS, WHEEL_PIXELS, self.gamut, |x, y| {
            let (dx, dy) = (x * 2.0 - 1.0, y * 2.0 - 1.0);
            let (h, radius) = wheel_polar(dx, dy);
            (dx.hypot(dy) <= 1.0).then_some(Lch {
                l: lightness,
                c: radius * chroma,
                h: h * 360.0,
            })
        });
        let image = Arc::new(RenderImage::new(vec![frame]));
        if let Some((_, old)) = self.wheel_image.replace((key, image.clone())) {
            self.retire(old);
        }
        image
    }

    fn lightness_bar_image(&mut self) -> Arc<RenderImage> {
        let lch = self.lch_shown();
        let key = BarKey {
            chroma: (lch.c * 1000.0).round() as i32,
            hue: (lch.h * 10.0).round() as i32,
            gamut: self.gamut,
        };
        if let Some((cached, image)) = &self.bar_image
            && *cached == key
        {
            return image.clone();
        }
        let (c, h) = (f64::from(key.chroma) / 1000.0, f64::from(key.hue) / 10.0);
        let frame = paint_gamut(4, 256, self.gamut, |_, y| Some(Lch { l: 1.0 - y, c, h }));
        let image = Arc::new(RenderImage::new(vec![frame]));
        if let Some((_, old)) = self.bar_image.replace((key, image.clone())) {
            self.retire(old);
        }
        image
    }

    fn marker(&self, left: Pixels, top: Pixels, fill: Hsla) -> gpui::Div {
        let z = self.zoom;
        let marker = px(MARKER * z);
        div()
            .absolute()
            .left(left - marker / 2.0)
            .top(top - marker / 2.0)
            .size(marker)
            .rounded_full()
            .border(px(2.0 * z))
            .border_color(gpui::white())
            .bg(fill)
    }
}

impl Render for ColorWheels {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("ColorWheels", || {
            let this = cx.entity();
            let size = self.size;
            let z = self.zoom;
            let bar_width = px(BAR_WIDTH * z);
            let (wheel_disabled, bar_disabled) = (self.wheel_disabled(), self.bar_disabled());
            let polar_mode = self.mode == ColorMode::Oklch;

            // The wheel: its image, then the cursor at the colour's hue and saturation.
            let (cursor, cursor_color, bar_level) = if polar_mode {
                let lch = self.lch_shown();
                let radius = (lch.c / oklch_max_chroma(self.gamut)).clamp(0.0, 1.0);
                let color = encode_in(oklch_to_linear(lch, self.gamut), self.gamut);
                (wheel_cursor(lch.h / 360.0, radius), encoded(color), lch.l)
            } else {
                let hsv = self.hsv_shown();
                (wheel_cursor(hsv.h, hsv.s), encoded(hsv_to_rgb(hsv)), hsv.v)
            };
            let image = if polar_mode {
                self.oklch_wheel_image()
            } else {
                hsv_wheel()
            };
            let bar_image = polar_mode.then(|| self.lightness_bar_image());
            let darkness = (!polar_mode).then(|| 1.0 - self.hsv_shown().v as f32);
            let half = size / 2.0;
            let cursor_at = (half + half * cursor.0 as f32, half + half * cursor.1 as f32);

            let wheel_canvas = {
                let this = this.clone();
                let image = image.clone();
                canvas(
                    {
                        let this = this.clone();
                        move |bounds, _, cx| {
                            guard::guard("color wheels layout", || {
                                this.update(cx, |wheels, _| wheels.wheel_bounds = bounds)
                            });
                        }
                    },
                    move |bounds, _, window, cx| {
                        guard::guard("color wheels paint", || {
                            window
                                .paint_image(
                                    bounds,
                                    bounds,
                                    Corners::all(bounds.size.width / 2.0),
                                    image,
                                    0,
                                    false,
                                )
                                .ok();
                            // Images the wheel moved on from are no longer drawn.
                            let retired =
                                this.update(cx, |wheels, _| std::mem::take(&mut wheels.retired));
                            for old in retired {
                                window.drop_image(old).ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full()
            };

            let wheel = div()
                .id("color-wheel")
                .relative()
                .flex_none()
                .size(size)
                .when(wheel_disabled, |d| d.opacity(0.5))
                .child(wheel_canvas)
                .when_some(darkness, |d, darkness| {
                    d.child(
                        div()
                            .absolute()
                            .size_full()
                            .rounded_full()
                            .bg(gpui::black().opacity(darkness)),
                    )
                })
                .child(self.marker(cursor_at.0, cursor_at.1, cursor_color))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, event: &MouseDownEvent, _, cx| {
                        this.pick(Surface::Wheel, event.position, cx);
                    }),
                )
                .on_drag(Pick(Surface::Wheel), |_, _, _, cx| cx.new(|_| Empty))
                .on_drag_move::<Pick>(cx.safe_listener(
                    |this, event: &DragMoveEvent<Pick>, _, cx| {
                        if matches!(event.drag(cx).0, Surface::Wheel) {
                            this.pick(Surface::Wheel, event.event.position, cx);
                        }
                    },
                ));

            let bar_canvas = {
                let this = this.clone();
                let image = bar_image.clone();
                canvas(
                    move |bounds, _, cx| {
                        guard::guard("color wheels layout", || {
                            this.update(cx, |wheels, _| wheels.bar_bounds = bounds)
                        });
                    },
                    move |bounds, _, window, _| {
                        guard::guard("color wheels paint", || {
                            if let Some(image) = image {
                                window
                                    .paint_image(
                                        bounds,
                                        bounds,
                                        Corners::all(px(2.0 * z)),
                                        image,
                                        0,
                                        false,
                                    )
                                    .ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full()
            };
            let top = px(0.0) + size * (1.0 - bar_level.clamp(0.0, 1.0) as f32);
            let bar = div()
                .id("color-bar")
                .relative()
                .flex_none()
                .w(bar_width)
                .h(size)
                .rounded(px(2.0 * z))
                .when(bar_disabled, |d| d.opacity(0.5))
                .when(!polar_mode, |d| {
                    let hsv = Hsv {
                        v: 1.0,
                        ..self.hsv_shown()
                    };
                    d.bg(linear_gradient(
                        180.0,
                        linear_color_stop(encoded(hsv_to_rgb(hsv)), 0.0),
                        linear_color_stop(gpui::black(), 1.0),
                    ))
                })
                .child(bar_canvas)
                .child(
                    div()
                        .absolute()
                        .left(px(-2.0 * z))
                        .w(bar_width + px(4.0 * z))
                        .top(top - px(3.0 * z))
                        .h(px(6.0 * z))
                        .rounded_full()
                        .border(px(2.0 * z))
                        .border_color(gpui::white()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, event: &MouseDownEvent, _, cx| {
                        this.pick(Surface::Bar, event.position, cx);
                    }),
                )
                .on_drag(Pick(Surface::Bar), |_, _, _, cx| cx.new(|_| Empty))
                .on_drag_move::<Pick>(cx.safe_listener(
                    |this, event: &DragMoveEvent<Pick>, _, cx| {
                        if matches!(event.drag(cx).0, Surface::Bar) {
                            this.pick(Surface::Bar, event.event.position, cx);
                        }
                    },
                ));
            #[cfg(test)]
            let (wheel, bar) = (
                wheel.debug_selector(|| "color-wheel".into()),
                bar.debug_selector(|| "color-bar".into()),
            );

            div().flex().gap(px(8.0 * z)).child(wheel).child(bar)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};

    use super::*;

    type Changes = Rc<RefCell<Vec<[f64; 3]>>>;

    fn setup(
        cx: &mut TestAppContext,
        wheels: ColorWheels,
    ) -> (Entity<ColorWheels>, Changes, &mut VisualTestContext) {
        cx.update(crate::init);
        let (view, cx) = cx.add_window_view(|_, _| wheels);
        let changes = Rc::new(RefCell::new(Vec::new()));
        let log = changes.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ColorWheelsEvent, _| {
                let ColorWheelsEvent::Change(c) = event;
                log.borrow_mut().push(*c);
            })
            .detach();
        });
        cx.run_until_parked();
        (view, changes, cx)
    }

    /// A point on the wheel, as offsets from its centre in half-sizes.
    fn on_wheel(cx: &mut VisualTestContext, dx: f32, dy: f32) -> Point<Pixels> {
        let b = cx.debug_bounds("color-wheel").expect("the wheel is drawn");
        let half = b.size.width / 2.0;
        point(b.center().x + half * dx, b.center().y + half * dy)
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn the_wheel_has_red_at_the_bottom_and_hues_going_clockwise() {
        let (h, s) = wheel_polar(0.0, 0.5);
        near(h, 0.0);
        near(s, 0.5);
        // Clockwise from red at the bottom is to the left, then up.
        near(wheel_polar(-0.5, 0.0).0, 0.25);
        near(wheel_polar(0.0, -0.5).0, 0.5);
        near(wheel_polar(0.5, 0.0).0, 0.75);
        // Outside the rim is the rim.
        near(wheel_polar(0.0, 3.0).1, 1.0);
    }

    #[test]
    fn the_cursor_is_where_the_wheel_picks() {
        for (turns, radius) in [(0.0, 0.5), (0.3, 1.0), (0.8, 0.25)] {
            let (x, y) = wheel_cursor(turns, radius);
            let (h, s) = wheel_polar(x, y);
            near(h, turns);
            near(s, radius);
        }
    }

    #[gpui::test]
    fn picking_on_the_hsv_wheel_sets_hue_and_saturation_keeping_value(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, ColorWheels::new(ColorMode::Hsv, [0.0, 0.0, 0.7]));
        // Halfway out to the left: hue 0.25, saturation 0.5.
        let at = on_wheel(cx, -0.5, 0.0);
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        let [h, s, v] = changes.borrow()[0];
        near(h, 0.25);
        near(s, 0.5);
        near(v, 0.7);
    }

    #[gpui::test]
    fn picking_on_the_value_bar_sets_value_only(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, ColorWheels::new(ColorMode::Hsv, [0.3, 0.6, 1.0]));
        let bar = cx.debug_bounds("color-bar").expect("the bar is drawn");
        cx.simulate_mouse_down(
            point(bar.center().x, bar.top() + bar.size.height * 0.75),
            MouseButton::Left,
            Modifiers::default(),
        );
        let [h, s, v] = changes.borrow()[0];
        near(h, 0.3);
        near(s, 0.6);
        near(v, 0.25);
    }

    #[gpui::test]
    fn the_oklch_wheel_sets_chroma_and_hue_degrees_keeping_lightness(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, ColorWheels::new(ColorMode::Oklch, [0.6, 0.0, 0.0]));
        let at = on_wheel(cx, 0.0, -1.0);
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        let [l, c, h] = changes.borrow()[0];
        near(l, 0.6);
        near(c, oklch_max_chroma(ColorSpace::Srgb));
        near(h, 180.0);
    }

    #[gpui::test]
    fn locked_components_stay_as_they_were(cx: &mut TestAppContext) {
        let wheels = ColorWheels::new(ColorMode::Hsv, [0.1, 0.2, 0.9]).locked([true, false, false]);
        let (_, changes, cx) = setup(cx, wheels);
        let at = on_wheel(cx, -0.5, 0.0);
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        let [h, s, _] = changes.borrow()[0];
        near(h, 0.1);
        near(s, 0.5);
    }

    #[gpui::test]
    fn a_control_whose_components_are_all_locked_ignores_the_pointer(cx: &mut TestAppContext) {
        let wheels = ColorWheels::new(ColorMode::Hsv, [0.1, 0.2, 0.9]).locked([true, true, false]);
        let (_, changes, cx) = setup(cx, wheels);
        let at = on_wheel(cx, -0.5, 0.0);
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        assert!(changes.borrow().is_empty());
        // The bar edits value, which isn't locked.
        let bar = cx.debug_bounds("color-bar").expect("the bar is drawn");
        cx.simulate_mouse_down(bar.center(), MouseButton::Left, Modifiers::default());
        assert_eq!(changes.borrow().len(), 1);
    }

    #[gpui::test]
    fn in_rgb_a_hue_survives_the_colour_going_grey(cx: &mut TestAppContext) {
        let (view, changes, cx) = setup(cx, ColorWheels::new(ColorMode::Rgb, [0.2, 0.2, 0.2]));
        let rim = on_wheel(cx, -0.9, 0.0);
        cx.simulate_mouse_down(rim, MouseButton::Left, Modifiers::default());
        let [r, g, b] = *changes.borrow().last().expect("sent");
        assert!(r != g || g != b, "the rim is coloured");
        let hue = cx.read(|cx| view.read(cx).hsv.h);
        near(hue, wheel_polar(-0.9, 0.0).0);
        // The colour is made grey from outside, which has no hue of its own.
        cx.update(|_, cx| {
            view.update(cx, |w, cx| {
                w.set_components(ColorMode::Rgb, [0.3, 0.3, 0.3], cx)
            });
        });
        near(cx.read(|cx| view.read(cx).hsv.h), hue);
        // Dragging the saturation out again finds that hue where it was left.
        let out = on_wheel(cx, -0.5, 0.0);
        cx.simulate_mouse_down(out, MouseButton::Left, Modifiers::default());
        near(cx.read(|cx| view.read(cx).hsv.h), wheel_polar(-0.5, 0.0).0);
    }

    #[gpui::test]
    fn the_value_bar_scales_with_the_zoom(cx: &mut TestAppContext) {
        let (view, _, cx) = setup(cx, ColorWheels::new(ColorMode::Hsv, [0.3, 0.6, 1.0]));
        for (zoom, width) in [(2.0, px(24.0)), (0.5, px(6.0))] {
            cx.update(|_, cx| view.update(cx, |w, cx| w.set_zoom(zoom, cx)));
            cx.run_until_parked();
            let bar = cx.debug_bounds("color-bar").expect("the bar is drawn");
            assert_eq!(bar.size.width, width);
        }
    }

    #[gpui::test]
    fn showing_a_new_colour_emits_nothing(cx: &mut TestAppContext) {
        let (view, changes, cx) = setup(cx, ColorWheels::new(ColorMode::Hsv, [0.0, 0.0, 0.0]));
        cx.update(|_, cx| {
            view.update(cx, |w, cx| {
                w.set_components(ColorMode::Oklch, [0.5, 0.1, 90.0], cx)
            });
        });
        assert!(changes.borrow().is_empty());
        assert_eq!(cx.read(|cx| view.read(cx).components()), [0.5, 0.1, 90.0]);
    }
}
