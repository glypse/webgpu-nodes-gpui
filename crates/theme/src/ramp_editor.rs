use std::collections::{BTreeMap, BTreeSet};

use gpui::{
    AppContext as _, Bounds, Context, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div, fill, point,
    prelude::FluentBuilder as _, px, size,
};
use graph_math::color::{ColorSpace, Rgba as LinearRgba};
use graph_math::ramp::{
    RampColorStop, RampModes, RampStop, add_ramp_stop, distribute_ramp, evaluate_ramp, flip_ramp,
    move_ramp_stop, new_stop_position, remove_ramp_stop,
};
use guard::SafeListener as _;

use crate::{
    ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName, NumberEvent, NumberField,
    linear_to_hsla,
};

/// How close the pointer must be (in pixels) to grab a stop on a double click.
const GRAB: f32 = 6.0;
/// Ctrl-dragged stops snap to 1 / SNAP steps.
const SNAP: f64 = 20.0;
/// How much slower stops move while Shift is held.
const PRECISION: f64 = 0.1;
/// Arrow keys move the selected stop by this much (ten times less with Shift).
const NUDGE: f64 = 0.01;
const BAR_HEIGHT: Pixels = px(24.0);
const MARKER_HEIGHT: Pixels = px(12.0);
const CHECKER: Pixels = px(8.0);

#[derive(Clone, Debug, PartialEq)]
pub enum RampEditorEvent {
    /// New stops, with the colours of those added.
    Change {
        stops: Vec<RampStop>,
        added: BTreeMap<String, LinearRgba>,
    },
}

struct Drag {
    id: String,
    origin: f64,
    /// How far the pointer has moved, in pixels, slowed while Shift is held.
    moved: f64,
    last: f64,
    /// The ramp as last sent, which the owner may not have given back yet.
    stops: Vec<RampStop>,
}

struct DragMarker;

/// A gradient with a marker for each stop: drag them, add them with Ctrl-click
/// or a double click, and edit the selected one's position.
pub struct RampEditor {
    /// Sorted by position.
    stops: Vec<RampStop>,
    /// Each stop's colour by id, as far as it's known.
    colors: BTreeMap<String, LinearRgba>,
    /// Stops whose colour is linked, so only known in the shader.
    linked: BTreeSet<String>,
    modes: RampModes,
    alpha: bool,
    gamut: ColorSpace,
    disabled: bool,
    picked: Option<String>,
    drag: Option<Drag>,
    /// The bar's bounds, as last drawn.
    lane: Bounds<Pixels>,
    focus: FocusHandle,
    position: Entity<NumberField>,
    _subscription: Subscription,
}

impl EventEmitter<RampEditorEvent> for RampEditor {}

impl RampEditor {
    pub fn new(
        stops: Vec<RampStop>,
        colors: BTreeMap<String, LinearRgba>,
        modes: RampModes,
        cx: &mut Context<Self>,
    ) -> Self {
        let position = cx.new(|cx| {
            NumberField::new(0.0, cx)
                .label("Position")
                .min(0.0)
                .max(1.0)
                .step(0.01)
        });
        let subscription = cx.subscribe(&position, |this, _, event, cx| {
            let NumberEvent::Change(number) = event;
            this.set_position(*number, cx);
        });
        let mut editor = Self {
            stops,
            colors,
            linked: BTreeSet::new(),
            modes,
            alpha: false,
            gamut: ColorSpace::Srgb,
            disabled: false,
            picked: None,
            drag: None,
            lane: Bounds::default(),
            focus: cx.focus_handle(),
            position,
            _subscription: subscription,
        };
        editor.refresh_position(cx);
        editor
    }

    /// Whether the colours' alpha is drawn (over a checkerboard).
    pub fn alpha(mut self, alpha: bool) -> Self {
        self.alpha = alpha;
        self
    }

    pub fn gamut(mut self, gamut: ColorSpace) -> Self {
        self.gamut = gamut;
        self
    }

    pub fn linked(mut self, linked: BTreeSet<String>) -> Self {
        self.linked = linked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn stops(&self) -> &[RampStop] {
        &self.stops
    }

    /// The id of the stop the position field edits: the picked one, or the first.
    pub fn selected(&self) -> Option<&str> {
        self.selected_stop().map(|s| s.id.as_str())
    }

    /// Shows a ramp, without emitting a change.
    pub fn set_ramp(
        &mut self,
        stops: Vec<RampStop>,
        colors: BTreeMap<String, LinearRgba>,
        linked: BTreeSet<String>,
        modes: RampModes,
        cx: &mut Context<Self>,
    ) {
        self.stops = stops;
        self.colors = colors;
        self.linked = linked;
        self.modes = modes;
        self.refresh_position(cx);
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.refresh_position(cx);
            cx.notify();
        }
    }

    fn selected_stop(&self) -> Option<&RampStop> {
        self.picked
            .as_ref()
            .and_then(|id| self.stops.iter().find(|s| s.id == *id))
            .or_else(|| self.stops.first())
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected_stop()?.id.clone();
        self.stops.iter().position(|s| s.id == id)
    }

    fn refresh_position(&mut self, cx: &mut Context<Self>) {
        let position = self.selected_stop().map_or(0.0, |s| s.position);
        let read_only = self.disabled || self.selected_stop().is_none();
        self.position.update(cx, |field, cx| {
            field.set_read_only(read_only, cx);
            field.set_value(position, cx);
        });
    }

    fn color_of(&self, id: &str) -> LinearRgba {
        self.colors
            .get(id)
            .copied()
            .unwrap_or(LinearRgba::new(0.0, 0.0, 0.0, 1.0))
    }

    fn colored(&self, stops: &[RampStop]) -> Vec<RampColorStop> {
        stops
            .iter()
            .map(|s| {
                let c = self.color_of(&s.id);
                RampColorStop {
                    position: s.position,
                    color: [c.r, c.g, c.b, c.a],
                }
            })
            .collect()
    }

    fn width(&self) -> f64 {
        f64::from(self.lane.size.width).max(1.0)
    }

    fn local_x(&self, position: Point<Pixels>) -> f64 {
        f64::from(position.x - self.lane.left())
    }

    fn send(
        &mut self,
        stops: Vec<RampStop>,
        added: BTreeMap<String, LinearRgba>,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = &mut self.drag {
            drag.stops = stops.clone();
        }
        for (id, color) in &added {
            self.colors.insert(id.clone(), *color);
        }
        self.colors
            .retain(|id, _| stops.iter().any(|s| s.id == *id));
        self.stops = stops.clone();
        self.refresh_position(cx);
        cx.emit(RampEditorEvent::Change { stops, added });
        cx.notify();
    }

    /// The nearest stop to `x`; of stops on top of each other, the last drawn.
    fn nearest(&self, x: f64) -> Option<&RampStop> {
        let width = self.width();
        self.stops.iter().rev().min_by(|a, b| {
            (a.position * width - x)
                .abs()
                .total_cmp(&(b.position * width - x).abs())
        })
    }

    fn add(&mut self, position: f64, cx: &mut Context<Self>) -> Option<(Vec<RampStop>, RampStop)> {
        if self.disabled {
            return None;
        }
        let (stops, stop) = add_ramp_stop(&self.stops, position);
        let [r, g, b, a] = evaluate_ramp(&self.colored(&self.stops), stop.position, self.modes);
        let added = BTreeMap::from([(stop.id.clone(), LinearRgba::new(r, g, b, a))]);
        self.picked = Some(stop.id.clone());
        self.send(stops.clone(), added, cx);
        Some((stops, stop))
    }

    fn add_after_selected(&mut self, cx: &mut Context<Self>) {
        let position = new_stop_position(&self.stops, self.selected());
        self.add(position, cx);
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        if self.disabled || self.stops.len() <= 1 {
            return;
        }
        let (Some(index), Some(id)) = (self.selected_index(), self.selected().map(str::to_owned))
        else {
            return;
        };
        let next = remove_ramp_stop(&self.stops, &id);
        self.picked = next
            .get(index.min(next.len().saturating_sub(1)))
            .map(|s| s.id.clone());
        self.send(next, BTreeMap::new(), cx);
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus);
        let x = self.local_x(event.position);
        let width = self.width();
        let near = self.nearest(x).cloned();
        if event.click_count >= 2 {
            let far = near
                .as_ref()
                .is_none_or(|s| (s.position * width - x).abs() > f64::from(GRAB));
            if far {
                self.add(x / width, cx);
            }
            return;
        }
        // Ctrl-click adds a stop, as in Blender; a click grabs the nearest one.
        let (stop, stops) = match near {
            Some(near) if !(event.modifiers.control || event.modifiers.platform) => {
                (near, self.stops.clone())
            }
            _ => match self.add(x / width, cx) {
                Some((stops, stop)) => (stop, stops),
                None => return,
            },
        };
        self.picked = Some(stop.id.clone());
        self.drag = Some(Drag {
            id: stop.id.clone(),
            origin: stop.position,
            moved: 0.0,
            last: x,
            stops,
        });
        self.refresh_position(cx);
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
        let x = self.local_x(position);
        let width = self.width();
        let Some(drag) = &mut self.drag else { return };
        drag.moved += (x - drag.last) * if modifiers.shift { PRECISION } else { 1.0 };
        drag.last = x;
        let target = drag.origin + drag.moved / width;
        let snapped = if modifiers.control || modifiers.platform {
            (target * SNAP).round() / SNAP
        } else {
            target
        };
        let next = move_ramp_stop(&drag.stops, &drag.id, snapped);
        self.send(next, BTreeMap::new(), cx);
    }

    fn release(&mut self) {
        self.drag = None;
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.selected_stop().cloned().filter(|_| !self.disabled) else {
            return;
        };
        match event.keystroke.key.as_str() {
            "delete" | "backspace" => self.remove_selected(cx),
            key @ ("left" | "right") => {
                let step = if event.keystroke.modifiers.shift {
                    NUDGE / 10.0
                } else {
                    NUDGE
                };
                let step = if key == "left" { -step } else { step };
                let next = move_ramp_stop(&self.stops, &selected.id, selected.position + step);
                self.send(next, BTreeMap::new(), cx);
            }
            "escape" => window.blur(),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn set_position(&mut self, number: f64, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(selected) = self.selected_stop().cloned() {
            let next = move_ramp_stop(&self.stops, &selected.id, number);
            self.send(next, BTreeMap::new(), cx);
        }
    }

    fn flip(&mut self, cx: &mut Context<Self>) {
        if !self.disabled {
            let next = flip_ramp(&self.stops);
            self.send(next, BTreeMap::new(), cx);
        }
    }

    fn distribute(&mut self, cx: &mut Context<Self>) {
        if !self.disabled && self.stops.len() >= 2 {
            let next = distribute_ramp(&self.stops);
            self.send(next, BTreeMap::new(), cx);
        }
    }

    fn display_color(&self, color: LinearRgba, with_alpha: bool) -> Hsla {
        let shown = linear_to_hsla(color.rgb(), self.gamut);
        if with_alpha {
            shown.opacity(color.a as f32)
        } else {
            shown
        }
    }
}

impl Focusable for RampEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RampEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = theme.colors.clone();
        let disabled = self.disabled;
        let this = cx.entity();
        let selected_id = self.selected().map(str::to_owned);
        let alpha = self.alpha;

        // The gradient, one column per pixel, sampled at its middle.
        let colored = self.colored(&self.stops);
        let modes = self.modes;
        let gamut = self.gamut;
        let bar = {
            let (this, colored) = (this.clone(), colored.clone());
            canvas(
                {
                    let this = this.clone();
                    move |bounds, _, cx| this.update(cx, |editor, _| editor.lane = bounds)
                },
                move |bounds, _, window, _| {
                    let width = f64::from(bounds.size.width);
                    let columns = width.ceil().max(1.0) as usize;
                    if alpha {
                        let rows =
                            (f64::from(bounds.size.height) / f64::from(CHECKER)).ceil() as usize;
                        let cols = (width / f64::from(CHECKER)).ceil() as usize;
                        for row in 0..rows {
                            for col in 0..cols {
                                let shade = if (row + col) % 2 == 0 { 0.6 } else { 0.4 };
                                let cell = Bounds::new(
                                    point(
                                        bounds.left() + CHECKER * col as f32,
                                        bounds.top() + CHECKER * row as f32,
                                    ),
                                    size(CHECKER, CHECKER),
                                );
                                window.paint_quad(fill(
                                    cell.intersect(&bounds),
                                    gpui::hsla(0.0, 0.0, shade, 1.0),
                                ));
                            }
                        }
                    }
                    for i in 0..columns {
                        let x = (i as f64 + 0.5) / columns as f64;
                        let [r, g, b, a] = evaluate_ramp(&colored, x, modes);
                        let color = linear_to_hsla(graph_math::color::Rgb::new(r, g, b), gamut);
                        let color = if alpha {
                            color.opacity(a as f32)
                        } else {
                            color
                        };
                        let left = bounds.left() + bounds.size.width * (i as f32 / columns as f32);
                        let next =
                            bounds.left() + bounds.size.width * ((i + 1) as f32 / columns as f32);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(left, bounds.top()),
                                size(next - left, bounds.size.height),
                            ),
                            color,
                        ));
                    }
                },
            )
            .absolute()
            .size_full()
        };

        let lane_width = self.lane.size.width;
        let markers = self.stops.iter().map(|stop| {
            let active = Some(&stop.id) == selected_id.as_ref();
            let fill_color = if self.linked.contains(&stop.id) {
                c.muted_foreground
            } else {
                self.display_color(
                    LinearRgba {
                        a: 1.0,
                        ..self.color_of(&stop.id)
                    },
                    false,
                )
            };
            let left = lane_width * stop.position as f32;
            div()
                .absolute()
                .top_0()
                .left(left - px(5.0))
                .w(px(10.0))
                .flex()
                .flex_col()
                .items_center()
                .when(active, |d| d.relative())
                .child(
                    div()
                        .h(BAR_HEIGHT)
                        .w(px(3.0))
                        .border_x_1()
                        .border_color(gpui::black().opacity(0.6))
                        .bg(gpui::white().opacity(if active { 1.0 } else { 0.7 })),
                )
                .child(
                    div()
                        .h(MARKER_HEIGHT)
                        .w(px(10.0))
                        .rounded_b_sm()
                        .border(if active { px(2.0) } else { px(1.0) })
                        .border_color(if active {
                            c.primary
                        } else {
                            gpui::black().opacity(0.5)
                        })
                        .bg(fill_color),
                )
        });

        let lane = div()
            .id("ramp-lane")
            .relative()
            .mx_1()
            .h(BAR_HEIGHT + MARKER_HEIGHT)
            .when(!disabled, |d| d.cursor(gpui::CursorStyle::ResizeLeftRight))
            .track_focus(&self.focus)
            .on_key_down(cx.safe_listener(Self::key_down))
            .child(
                div()
                    .relative()
                    .h(BAR_HEIGHT)
                    .rounded_sm()
                    .overflow_hidden()
                    .border_1()
                    .border_color(gpui::black().opacity(0.2))
                    .child(bar),
            )
            .children(markers)
            .on_mouse_down(
                MouseButton::Left,
                cx.safe_listener(|this, event: &MouseDownEvent, window, cx| {
                    this.press(event, window, cx)
                }),
            )
            .on_drag(DragMarker, |_, _, _, cx| cx.new(|_| Empty))
            .on_drag_move::<DragMarker>(cx.safe_listener(
                |this, event: &DragMoveEvent<DragMarker>, _, cx| {
                    this.drag_to(event.event.position, event.event.modifiers, cx);
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.safe_listener(|this, _: &MouseUpEvent, _, _| this.release()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.safe_listener(|this, _: &MouseUpEvent, _, _| this.release()),
            );
        #[cfg(test)]
        let lane = lane.debug_selector(|| "ramp-lane".into());

        let tool = |id: &'static str, icon: IconName, label: &'static str, off: bool| {
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .icon(icon)
                .tooltip(label)
                .disabled(disabled || off)
        };
        #[cfg(test)]
        let wrap = |id: &'static str, button: Button| {
            div().debug_selector(move || id.to_owned()).child(button)
        };
        #[cfg(not(test))]
        let wrap = |_: &'static str, button: Button| button;
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(wrap(
                "ramp-add",
                tool("ramp-add", IconName::Plus, "Add a stop", false)
                    .on_click(cx.listener(|this, _, _, cx| this.add_after_selected(cx))),
            ))
            .child(wrap(
                "ramp-remove",
                tool(
                    "ramp-remove",
                    IconName::Minus,
                    "Remove the selected stop",
                    self.stops.len() <= 1,
                )
                .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
            ))
            .child(div().flex_1())
            .child(wrap(
                "ramp-flip",
                tool(
                    "ramp-flip",
                    IconName::FlipHorizontal,
                    "Flip left to right",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.flip(cx))),
            ))
            .child(wrap(
                "ramp-distribute",
                tool(
                    "ramp-distribute",
                    IconName::ArrowsOutLineHorizontal,
                    "Space the stops evenly",
                    self.stops.len() < 2,
                )
                .on_click(cx.listener(|this, _, _, cx| this.distribute(cx))),
            ));

        let selected_index = self.selected_index();
        let linked = self.selected().is_some_and(|id| self.linked.contains(id));
        let caption = SharedString::from(format!(
            "Stop {}{}",
            selected_index.map_or(0, |i| i + 1),
            if linked { " (linked)" } else { "" }
        ));
        let footer = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .text_size(theme.text.xs)
            .child(
                div()
                    .flex_none()
                    .text_color(c.muted_foreground)
                    .child(caption),
            )
            .child(div().flex_1().min_w_0().child(self.position.clone()));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .font_family(theme.font.clone())
            .when(disabled, |d| d.opacity(0.6))
            .child(toolbar)
            .child(lane)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};
    use graph_math::ramp::default_ramp;

    use super::*;

    type Changes = Rc<RefCell<Vec<(Vec<RampStop>, BTreeMap<String, LinearRgba>)>>>;

    fn colors() -> BTreeMap<String, LinearRgba> {
        BTreeMap::from([
            ("stop-0".to_owned(), LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
            ("stop-1".to_owned(), LinearRgba::new(1.0, 1.0, 1.0, 1.0)),
        ])
    }

    fn setup(
        cx: &mut TestAppContext,
        stops: Vec<RampStop>,
        disabled: bool,
    ) -> (Entity<RampEditor>, Changes, &mut VisualTestContext) {
        cx.update(crate::init);
        let (editor, cx) = cx.add_window_view(move |_, cx| {
            RampEditor::new(stops, colors(), RampModes::default(), cx).disabled(disabled)
        });
        let changes: Changes = Rc::default();
        let log = changes.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &RampEditorEvent, _| {
                let RampEditorEvent::Change { stops, added } = event;
                log.borrow_mut().push((stops.clone(), added.clone()));
            })
            .detach();
        });
        cx.run_until_parked();
        (editor, changes, cx)
    }

    /// A position of the ramp, in window coordinates, at the middle of the bar.
    fn at(cx: &mut VisualTestContext, position: f64) -> Point<Pixels> {
        let lane = cx.debug_bounds("ramp-lane").expect("the lane is drawn");
        point(
            lane.left() + lane.size.width * position as f32,
            lane.top() + px(10.0),
        )
    }

    fn ids(stops: &[RampStop]) -> Vec<&str> {
        stops.iter().map(|s| s.id.as_str()).collect()
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 2e-2, "{a} vs {b}");
    }

    fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("the control is drawn");
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_click_selects_the_nearest_stop_without_changing_the_ramp(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, default_ramp(), false);
        let near_end = at(cx, 0.9);
        cx.simulate_mouse_down(near_end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(near_end, MouseButton::Left, Modifiers::default());
        assert_eq!(
            cx.read(|cx| editor.read(cx).selected().map(str::to_owned)),
            Some("stop-1".into())
        );
        assert!(changes.borrow().is_empty());
    }

    #[gpui::test]
    fn dragging_a_stop_moves_it_and_it_may_pass_others(cx: &mut TestAppContext) {
        let mut stops = default_ramp();
        stops.push(RampStop {
            id: "stop-2".into(),
            position: 0.5,
        });
        let stops = graph_math::ramp::sorted_ramp_stops(Some(&stops));
        let (_, changes, cx) = setup(cx, stops, false);
        let from = at(cx, 0.0);
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        for to in [0.4, 0.8] {
            let target = at(cx, to);
            cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
        }
        let end = at(cx, 0.8);
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let (sent, _) = changes.borrow().last().cloned().expect("a change");
        assert_eq!(ids(&sent), ["stop-2", "stop-0", "stop-1"]);
        near(sent[1].position, 0.8);
    }

    #[gpui::test]
    fn ctrl_click_adds_a_stop_coloured_by_the_ramp_there(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, default_ramp(), false);
        let middle = at(cx, 0.5);
        cx.simulate_mouse_down(
            middle,
            MouseButton::Left,
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        cx.simulate_mouse_up(middle, MouseButton::Left, Modifiers::default());
        let (sent, added) = changes.borrow().first().cloned().expect("a change");
        assert_eq!(sent.len(), 3);
        let colour = added.get("stop-2").expect("the new stop has a colour");
        near(colour.r, 0.5);
        assert_eq!(
            cx.read(|cx| editor.read(cx).selected().map(str::to_owned)),
            Some("stop-2".into())
        );
    }

    #[gpui::test]
    fn a_double_click_adds_a_stop_only_away_from_the_others(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, default_ramp(), false);
        let down = |position, count| MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: count,
            first_mouse: false,
        };
        let on_stop = at(cx, 1.0);
        cx.simulate_event(down(on_stop, 2));
        assert!(changes.borrow().is_empty());
        let away = at(cx, 0.5);
        cx.simulate_event(down(away, 2));
        assert_eq!(changes.borrow().len(), 1);
        assert_eq!(changes.borrow()[0].0.len(), 3);
    }

    #[gpui::test]
    fn the_add_button_adds_halfway_to_the_next_stop(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, default_ramp(), false);
        click_selector(cx, "ramp-add");
        let (sent, _) = changes.borrow().last().cloned().expect("a change");
        assert_eq!(sent.len(), 3);
        near(sent[1].position, 0.5);
    }

    #[gpui::test]
    fn the_remove_button_removes_the_selected_stop_but_keeps_one(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, default_ramp(), false);
        click_selector(cx, "ramp-remove");
        assert_eq!(ids(&changes.borrow()[0].0), ["stop-1"]);
        assert_eq!(
            cx.read(|cx| editor.read(cx).selected().map(str::to_owned)),
            Some("stop-1".into())
        );
        click_selector(cx, "ramp-remove");
        assert_eq!(changes.borrow().len(), 1, "the last stop stays");
    }

    #[gpui::test]
    fn flip_and_distribute_rearrange_the_stops(cx: &mut TestAppContext) {
        let stops = vec![
            RampStop {
                id: "stop-0".into(),
                position: 0.0,
            },
            RampStop {
                id: "stop-1".into(),
                position: 0.2,
            },
            RampStop {
                id: "stop-2".into(),
                position: 1.0,
            },
        ];
        let (_, changes, cx) = setup(cx, stops, false);
        click_selector(cx, "ramp-flip");
        let (flipped, _) = changes.borrow().last().cloned().expect("a change");
        assert_eq!(ids(&flipped), ["stop-2", "stop-1", "stop-0"]);
        near(flipped[1].position, 0.8);
        click_selector(cx, "ramp-distribute");
        let (spread, _) = changes.borrow().last().cloned().expect("a change");
        assert_eq!(
            spread.iter().map(|s| s.position).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0]
        );
    }

    #[gpui::test]
    fn arrow_keys_nudge_the_selected_stop_and_delete_removes_it(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, default_ramp(), false);
        let on_end = at(cx, 1.0);
        cx.simulate_mouse_down(on_end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(on_end, MouseButton::Left, Modifiers::default());
        cx.simulate_keystrokes("left");
        near(
            changes.borrow().last().expect("a change").0[1].position,
            0.99,
        );
        cx.simulate_keystrokes("shift-right");
        near(
            changes.borrow().last().expect("a change").0[1].position,
            0.991,
        );
        cx.simulate_keystrokes("delete");
        assert_eq!(changes.borrow().last().expect("a change").0.len(), 1);
    }

    #[gpui::test]
    fn the_position_field_moves_the_selected_stop(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, default_ramp(), false);
        let field = cx.read(|cx| editor.read(cx).position.clone());
        cx.update(|_, cx| field.update(cx, |_, cx| cx.emit(NumberEvent::Change(0.25))));
        cx.run_until_parked();
        let (sent, _) = changes.borrow().last().cloned().expect("a change");
        near(sent[0].position, 0.25);
    }

    #[gpui::test]
    fn a_disabled_editor_ignores_everything(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, default_ramp(), true);
        let middle = at(cx, 0.5);
        cx.simulate_mouse_down(
            middle,
            MouseButton::Left,
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        cx.simulate_mouse_up(middle, MouseButton::Left, Modifiers::default());
        click_selector(cx, "ramp-flip");
        assert!(changes.borrow().is_empty());
    }

    #[gpui::test]
    fn showing_a_ramp_emits_nothing(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, default_ramp(), false);
        cx.update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.set_ramp(
                    vec![RampStop {
                        id: "stop-3".into(),
                        position: 0.3,
                    }],
                    BTreeMap::new(),
                    BTreeSet::new(),
                    RampModes::default(),
                    cx,
                );
            });
        });
        assert!(changes.borrow().is_empty());
        assert_eq!(
            cx.read(|cx| editor.read(cx).selected().map(str::to_owned)),
            Some("stop-3".into())
        );
    }
}
