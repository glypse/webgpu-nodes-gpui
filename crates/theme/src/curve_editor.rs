use gpui::{
    AppContext as _, Bounds, Context, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div,
    fill, point, prelude::FluentBuilder as _, px, size,
};
use graph_math::curve::{
    CurvePoint, CurvePointKind, HandleSide, Vec2, add_curve_point, curve_presets, curve_segments,
    flip_curve_x, flip_curve_y, insert_curve_point, move_curve_handle, move_curve_point,
    point_handles, remove_curve_point, segment_point, set_curve_point_kind,
};
use guard::SafeListener as _;

use crate::{
    ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName, NumberEvent, NumberField,
};

/// Room around the square, so points on its edges are drawn whole.
const INSET: f64 = 6.0;
/// How close the pointer must be (in pixels) to grab a point or a handle.
const GRAB: f64 = 7.0;
/// How close the pointer must be (in pixels) to the curve to add a point on it.
const CURVE_GRAB: f64 = 5.0;
/// Ctrl-dragged points snap to 1 / SNAP steps, as in Blender.
const SNAP: f64 = 8.0;
/// How much slower things move while Shift is held.
const PRECISION: f64 = 0.1;
const GRID: [f64; 3] = [0.25, 0.5, 0.75];
/// Samples per segment finding the pointer's distance to the curve.
const CURVE_SAMPLES: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub enum CurveEditorEvent {
    Change(Vec<CurvePoint>),
}

/// What one of the point's three coordinate pairs is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Point,
    Left,
    Right,
}

const PARTS: [(Part, &str); 3] = [
    (Part::Point, "Point"),
    (Part::Left, "Left"),
    (Part::Right, "Right"),
];

/// One step of a path through a curve, in the units `at` maps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathStep {
    Move(Vec2),
    Line(Vec2),
    Cubic {
        first: Vec2,
        second: Vec2,
        end: Vec2,
    },
}

/// A path of a curve across the whole square, `at` placing its points.
pub fn curve_path(points: &[CurvePoint], at: impl Fn(Vec2) -> Vec2) -> Vec<PathStep> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    let mut steps = vec![
        PathStep::Move(at(Vec2::new(0.0, first.y))),
        PathStep::Line(at(Vec2::new(first.x, first.y))),
    ];
    for [_, c0, c1, end] in curve_segments(points) {
        steps.push(PathStep::Cubic {
            first: at(c0),
            second: at(c1),
            end: at(end),
        });
    }
    steps.push(PathStep::Line(at(Vec2::new(1.0, last.y))));
    steps
}

/// The handles drawn around a point: an end point's outer one shapes nothing.
fn sides(points: &[CurvePoint], index: usize) -> Vec<HandleSide> {
    let Some(point) = points.get(index) else {
        return Vec::new();
    };
    if point.kind == CurvePointKind::Vector {
        return Vec::new();
    }
    let mut sides = Vec::new();
    if index > 0 {
        sides.push(HandleSide::Left);
    }
    if index + 1 < points.len() {
        sides.push(HandleSide::Right);
    }
    sides
}

fn part_side(part: Part) -> Option<HandleSide> {
    match part {
        Part::Point => None,
        Part::Left => Some(HandleSide::Left),
        Part::Right => Some(HandleSide::Right),
    }
}

/// Where a part of a point is in the square.
fn part_at(point: &CurvePoint, part: Part) -> Vec2 {
    let (left, right) = point_handles(point);
    match part {
        Part::Point => Vec2::new(point.x, point.y),
        Part::Left => Vec2::new(point.x + left.x, point.y + left.y),
        Part::Right => Vec2::new(point.x + right.x, point.y + right.y),
    }
}

/// The curve's pixels at a size: the square sits `INSET` in from the sides.
#[derive(Clone, Copy)]
struct Plane {
    span: f64,
}

impl Plane {
    fn new(side: f64) -> Self {
        Self {
            span: (side - 2.0 * INSET).max(1.0),
        }
    }

    fn project(self, p: Vec2) -> Vec2 {
        Vec2::new(INSET + p.x * self.span, INSET + (1.0 - p.y) * self.span)
    }

    fn unproject(self, p: Vec2) -> Vec2 {
        Vec2::new((p.x - INSET) / self.span, 1.0 - (p.y - INSET) / self.span)
    }
}

fn distance(a: Vec2, b: Vec2) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn segment_distance(m: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq == 0.0 {
        0.0
    } else {
        (((m.x - a.x) * dx + (m.y - a.y) * dy) / length_sq).clamp(0.0, 1.0)
    };
    distance(m, Vec2::new(a.x + t * dx, a.y + t * dy))
}

/// Whether a pixel is within `CURVE_GRAB` of the curve.
fn on_curve(points: &[CurvePoint], plane: Plane, m: Vec2) -> bool {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return false;
    };
    let mut line = vec![Vec2::new(0.0, first.y)];
    for segment in curve_segments(points) {
        line.extend(
            (0..=CURVE_SAMPLES).map(|i| segment_point(&segment, i as f64 / CURVE_SAMPLES as f64)),
        );
    }
    line.push(Vec2::new(1.0, last.y));
    let pixels: Vec<Vec2> = line.into_iter().map(|p| plane.project(p)).collect();
    pixels
        .windows(2)
        .any(|pair| segment_distance(m, pair[0], pair[1]) < CURVE_GRAB)
}

/// The point nearest the pointer, within `GRAB`.
fn grabbed_point(points: &[CurvePoint], plane: Plane, m: Vec2) -> Option<usize> {
    let mut nearest = None;
    let mut best = GRAB;
    for (i, p) in points.iter().enumerate() {
        let d = distance(plane.project(Vec2::new(p.x, p.y)), m);
        if d < best {
            best = d;
            nearest = Some(i);
        }
    }
    nearest
}

struct Drag {
    part: Part,
    index: usize,
    /// Where the point, or the handle's offset, was when the drag started.
    origin: Vec2,
    /// How far the pointer has moved, in pixels, slowed while Shift is held.
    moved: Vec2,
    last: Vec2,
    /// The curve as last sent, which the owner may not have given back yet.
    points: Vec<CurvePoint>,
}

struct DragMarker;

/// A graph of a curve through control points, with a point's kind, flips,
/// presets and coordinates.
pub struct CurveEditor {
    points: Vec<CurvePoint>,
    disabled: bool,
    side: Pixels,
    selected: Option<usize>,
    drag: Option<Drag>,
    /// The graph's top left, as last drawn.
    origin: Point<Pixels>,
    focus: FocusHandle,
    fields: [[Entity<NumberField>; 2]; 3],
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CurveEditorEvent> for CurveEditor {}

impl CurveEditor {
    pub fn new(points: Vec<CurvePoint>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let fields = [0, 1, 2].map(|part| {
            [0, 1].map(|axis| {
                let field = cx.new(|cx| NumberField::new(0.0, cx).step(0.01));
                subscriptions.push(cx.subscribe(&field, move |this, _, event, cx| {
                    let NumberEvent::Change(number) = event;
                    this.set_coordinate(PARTS[part].0, axis, *number, cx);
                }));
                field
            })
        });
        let mut editor = Self {
            points,
            disabled: false,
            side: px(150.0),
            selected: None,
            drag: None,
            origin: Point::default(),
            focus: cx.focus_handle(),
            fields,
            _subscriptions: subscriptions,
        };
        editor.refresh_fields(cx);
        editor
    }

    /// The side of the graph.
    pub fn side(mut self, side: Pixels) -> Self {
        self.side = side;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn set_side(&mut self, side: Pixels, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            cx.notify();
        }
    }

    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Shows a curve, without emitting a change.
    pub fn set_points(&mut self, points: Vec<CurvePoint>, cx: &mut Context<Self>) {
        if self.points == points {
            return;
        }
        self.points = points;
        if self.selected.is_some_and(|i| i >= self.points.len()) {
            self.selected = None;
        }
        self.refresh_fields(cx);
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.refresh_fields(cx);
            cx.notify();
        }
    }

    fn plane(&self) -> Plane {
        Plane::new(f64::from(self.side))
    }

    fn active(&self) -> Option<&CurvePoint> {
        self.selected.and_then(|i| self.points.get(i))
    }

    fn editable(&self, part: Part) -> bool {
        let Some(index) = self.selected.filter(|_| !self.disabled) else {
            return false;
        };
        part_side(part).is_none_or(|side| sides(&self.points, index).contains(&side))
    }

    fn refresh_fields(&mut self, cx: &mut Context<Self>) {
        let active = self.active().copied();
        for (row, (part, _)) in self.fields.iter().zip(PARTS) {
            let at = active.map_or(Vec2::default(), |p| part_at(&p, part));
            let editable = self.editable(part);
            for (axis, field) in row.iter().enumerate() {
                let value = if axis == 0 { at.x } else { at.y };
                let (min, max) = if part == Part::Point {
                    (Some(0.0), Some(1.0))
                } else {
                    (None, None)
                };
                let label = if axis == 0 { "X" } else { "Y" };
                field.update(cx, |field, cx| {
                    field.configure(label, min, max, cx);
                    field.set_read_only(!editable, cx);
                    field.set_value(value, cx);
                });
            }
        }
    }

    fn send(&mut self, next: Vec<CurvePoint>, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.drag {
            drag.points = next.clone();
        }
        self.points = next.clone();
        self.refresh_fields(cx);
        cx.emit(CurveEditorEvent::Change(next));
        cx.notify();
    }

    fn local(&self, position: Point<Pixels>) -> Vec2 {
        Vec2::new(
            f64::from(position.x - self.origin.x),
            f64::from(position.y - self.origin.y),
        )
    }

    fn grabbed_handle(&self, m: Vec2) -> Option<HandleSide> {
        let (index, point) = (self.selected?, self.active()?);
        let plane = self.plane();
        sides(&self.points, index).into_iter().find(|side| {
            let part = if *side == HandleSide::Left {
                Part::Left
            } else {
                Part::Right
            };
            distance(plane.project(part_at(point, part)), m) < GRAB
        })
    }

    fn press(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus);
        let m = self.local(position);
        let plane = self.plane();
        let mut next = None;
        let mut part = Part::Point;
        let index = if let (Some(side), Some(selected)) = (self.grabbed_handle(m), self.selected) {
            part = if side == HandleSide::Left {
                Part::Left
            } else {
                Part::Right
            };
            Some(selected)
        } else {
            let grabbed = grabbed_point(&self.points, plane, m);
            if grabbed.is_none() && (modifiers.control || modifiers.platform) {
                let at = plane.unproject(m);
                let (points, added) = add_curve_point(&self.points, at.x, at.y);
                next = Some(points);
                Some(added)
            } else if grabbed.is_none() && on_curve(&self.points, plane, m) {
                let (points, added) = insert_curve_point(&self.points, plane.unproject(m).x);
                next = Some(points);
                Some(added)
            } else {
                grabbed
            }
        };
        self.selected = index;
        let current = next.clone().unwrap_or_else(|| self.points.clone());
        let Some((index, point)) = index.and_then(|i| Some((i, *current.get(i)?))) else {
            self.drag = None;
            self.refresh_fields(cx);
            cx.notify();
            return;
        };
        if let Some(points) = next {
            self.points = points.clone();
            cx.emit(CurveEditorEvent::Change(points));
        }
        let origin = match part {
            Part::Point => Vec2::new(point.x, point.y),
            Part::Left => point.left,
            Part::Right => point.right,
        };
        self.drag = Some(Drag {
            part,
            index,
            origin,
            moved: Vec2::default(),
            last: m,
            points: current,
        });
        self.refresh_fields(cx);
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
        let m = self.local(position);
        let span = self.plane().span;
        let Some(drag) = &mut self.drag else { return };
        let scale = if modifiers.shift { PRECISION } else { 1.0 };
        drag.moved = Vec2::new(
            drag.moved.x + (m.x - drag.last.x) * scale,
            drag.moved.y + (m.y - drag.last.y) * scale,
        );
        drag.last = m;
        let target = Vec2::new(
            drag.origin.x + drag.moved.x / span,
            drag.origin.y - drag.moved.y / span,
        );
        let (part, index, base) = (drag.part, drag.index, drag.points.clone());
        let next = match part_side(part) {
            None => {
                let snap = |n: f64| {
                    if modifiers.control || modifiers.platform {
                        (n * SNAP).round() / SNAP
                    } else {
                        n
                    }
                };
                move_curve_point(&base, index, snap(target.x), snap(target.y))
            }
            Some(side) => move_curve_handle(&base, index, side, target),
        };
        self.send(next, cx);
    }

    fn release(&mut self) {
        self.drag = None;
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self
            .selected
            .filter(|_| !self.disabled && self.points.len() > 2)
        else {
            return;
        };
        let next = remove_curve_point(&self.points, index);
        self.selected = None;
        self.send(next, cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        match event.keystroke.key.as_str() {
            "delete" | "backspace" => self.remove_selected(cx),
            "escape" => {
                self.selected = None;
                self.refresh_fields(cx);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn set_kind(&mut self, kind: CurvePointKind, cx: &mut Context<Self>) {
        if let Some(index) = self.selected.filter(|_| !self.disabled) {
            let next = set_curve_point_kind(&self.points, index, kind);
            self.send(next, cx);
        }
    }

    fn flip(&mut self, horizontally: bool, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let next = if horizontally {
            self.selected = self
                .selected
                .map(|i| self.points.len().saturating_sub(1).saturating_sub(i));
            flip_curve_x(&self.points)
        } else {
            flip_curve_y(&self.points)
        };
        self.send(next, cx);
    }

    fn apply_preset(&mut self, points: Vec<CurvePoint>, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.selected = None;
        self.send(points, cx);
    }

    fn set_coordinate(&mut self, part: Part, axis: usize, number: f64, cx: &mut Context<Self>) {
        let (Some(index), Some(active)) = (self.selected, self.active().copied()) else {
            return;
        };
        if !self.editable(part) {
            return;
        }
        let mut at = part_at(&active, part);
        if axis == 0 {
            at.x = number;
        } else {
            at.y = number;
        }
        let next = match part_side(part) {
            None => move_curve_point(&self.points, index, at.x, at.y),
            Some(side) => move_curve_handle(
                &self.points,
                index,
                side,
                Vec2::new(at.x - active.x, at.y - active.y),
            ),
        };
        self.send(next, cx);
    }
}

impl Focusable for CurveEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn pt(origin: Point<Pixels>, p: Vec2) -> Point<Pixels> {
    point(origin.x + px(p.x as f32), origin.y + px(p.y as f32))
}

fn paint_dot(window: &mut Window, centre: Point<Pixels>, radius: f32, color: Hsla, ring: Hsla) {
    let bounds = Bounds::new(
        point(centre.x - px(radius), centre.y - px(radius)),
        size(px(2.0 * radius), px(2.0 * radius)),
    );
    let mut quad = fill(bounds, color);
    quad.corner_radii = px(radius).into();
    quad.border_widths = px(1.5).into();
    quad.border_color = ring;
    window.paint_quad(quad);
}

/// Strokes a path of steps, each point offset by `origin`.
fn stroke_path(
    window: &mut Window,
    origin: Point<Pixels>,
    steps: &[PathStep],
    width: f32,
    color: Hsla,
) {
    let mut builder = PathBuilder::stroke(px(width));
    for step in steps {
        match *step {
            PathStep::Move(p) => builder.move_to(pt(origin, p)),
            PathStep::Line(p) => builder.line_to(pt(origin, p)),
            PathStep::Cubic { first, second, end } => {
                builder.cubic_bezier_to(pt(origin, end), pt(origin, first), pt(origin, second));
            }
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A glyph's lines, its point and its handles, in a 16 by 16 box.
type Glyph = (&'static [(f32, f32)], (f32, f32), &'static [(f32, f32)]);

fn kind_glyph(kind: CurvePointKind) -> Glyph {
    match kind {
        CurvePointKind::Vector => (&[(2.0, 13.0), (8.0, 4.0), (14.0, 13.0)], (8.0, 4.0), &[]),
        CurvePointKind::Free => (
            &[(3.0, 4.0), (8.0, 10.0), (14.0, 7.0)],
            (8.0, 10.0),
            &[(3.0, 4.0), (14.0, 7.0)],
        ),
        CurvePointKind::Aligned => (
            &[(2.5, 11.0), (13.0, 5.0)],
            (6.5, 9.0),
            &[(2.5, 11.0), (13.0, 5.0)],
        ),
        CurvePointKind::Mirrored => (
            &[(2.0, 8.0), (14.0, 8.0)],
            (8.0, 8.0),
            &[(2.0, 8.0), (14.0, 8.0)],
        ),
    }
}

impl Render for CurveEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("CurveEditor", || {
            let theme = cx.theme();
            let c = theme.colors.clone();
            let side = self.side;
            let plane = self.plane();
            let disabled = self.disabled;
            let selected = self.selected;
            let points = self.points.clone();
            let active = self.active().copied();
            let this = cx.entity();

            let graph = {
                let (points, c, this) = (points.clone(), c.clone(), this.clone());
                canvas(
                    {
                        let this = this.clone();
                        move |bounds, _, cx| {
                            guard::guard("curve editor layout", || {
                                this.update(cx, |editor, _| editor.origin = bounds.origin)
                            });
                        }
                    },
                    move |bounds, _, window, _| {
                        guard::guard("curve editor paint", || {
                            let origin = bounds.origin;
                            let mut quad = fill(bounds, c.foreground.opacity(0.05));
                            quad.corner_radii = px(4.0).into();
                            window.paint_quad(quad);
                            let span = plane.span as f32;
                            let inset = INSET as f32;
                            for g in GRID {
                                let g = g as f32;
                                let line = c.foreground.opacity(0.1);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            origin.x + px(inset + g * span),
                                            origin.y + px(inset),
                                        ),
                                        size(px(1.0), px(span)),
                                    ),
                                    line,
                                ));
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            origin.x + px(inset),
                                            origin.y + px(inset + g * span),
                                        ),
                                        size(px(span), px(1.0)),
                                    ),
                                    line,
                                ));
                            }
                            let mut frame = fill(
                                Bounds::new(
                                    point(origin.x + px(inset), origin.y + px(inset)),
                                    size(px(span), px(span)),
                                ),
                                gpui::transparent_black(),
                            );
                            frame.border_widths = px(1.0).into();
                            frame.border_color = c.foreground.opacity(0.15);
                            window.paint_quad(frame);

                            let steps = curve_path(&points, |p| plane.project(p));
                            // The area under the curve.
                            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                                let mut area = PathBuilder::fill();
                                for step in &steps {
                                    match *step {
                                        PathStep::Move(p) => area.move_to(pt(origin, p)),
                                        PathStep::Line(p) => area.line_to(pt(origin, p)),
                                        PathStep::Cubic { first, second, end } => {
                                            area.cubic_bezier_to(
                                                pt(origin, end),
                                                pt(origin, first),
                                                pt(origin, second),
                                            );
                                        }
                                    }
                                }
                                let _ = (first, last);
                                area.line_to(pt(origin, plane.project(Vec2::new(1.0, 0.0))));
                                area.line_to(pt(origin, plane.project(Vec2::new(0.0, 0.0))));
                                area.close();
                                if let Ok(path) = area.build() {
                                    window.paint_path(path, c.foreground.opacity(0.05));
                                }
                            }
                            stroke_path(window, origin, &steps, 1.5, c.foreground);

                            if let (Some(point), Some(index)) =
                                (points.get(selected.unwrap_or(usize::MAX)), selected)
                            {
                                let at = plane.project(Vec2::new(point.x, point.y));
                                for handle_side in sides(&points, index) {
                                    let part = if handle_side == HandleSide::Left {
                                        Part::Left
                                    } else {
                                        Part::Right
                                    };
                                    let handle = plane.project(part_at(point, part));
                                    stroke_path(
                                        window,
                                        origin,
                                        &[PathStep::Move(at), PathStep::Line(handle)],
                                        1.0,
                                        c.primary.opacity(0.7),
                                    );
                                    paint_dot(window, pt(origin, handle), 3.0, c.card, c.primary);
                                }
                            }
                            for (i, p) in points.iter().enumerate() {
                                let at = pt(origin, plane.project(Vec2::new(p.x, p.y)));
                                if Some(i) == selected {
                                    paint_dot(window, at, 4.5, c.primary, c.card);
                                } else {
                                    paint_dot(window, at, 3.5, c.foreground, c.card);
                                }
                            }
                        });
                    },
                )
                .absolute()
                .size_full()
            };
            let graph = div()
                .id("curve-graph")
                .relative()
                .flex_none()
                .size(side)
                .when(!disabled, |d| d.cursor_crosshair())
                .track_focus(&self.focus)
                .on_key_down(cx.safe_listener(Self::key_down))
                .child(graph)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, event: &MouseDownEvent, window, cx| {
                        this.press(event.position, event.modifiers, window, cx);
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
            let graph = graph.debug_selector(|| "curve-graph".into());

            let muted = c.muted_foreground;
            let presets = div().flex().flex_col().justify_between().children(
                curve_presets().into_iter().map(|preset| {
                    let label = SharedString::from(format!("{} preset", preset.label));
                    let thumb = preset.points.clone();
                    let applied = preset.points;
                    let id = preset.id;
                    let button = Button::new(SharedString::from(format!("curve-preset-{id}")))
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconXs)
                        .tooltip(label)
                        .disabled(disabled)
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, (), window, _| {
                                    guard::guard("curve editor paint", || {
                                        let steps = curve_path(&thumb, |p| {
                                            Vec2::new(3.0 + p.x * 14.0, 3.0 + (1.0 - p.y) * 14.0)
                                        });
                                        stroke_path(window, bounds.origin, &steps, 1.5, muted);
                                    });
                                },
                            )
                            .size(px(20.0)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_preset(applied.clone(), cx)
                        }));
                    #[cfg(test)]
                    let button = div()
                        .debug_selector(move || format!("curve-preset-{id}"))
                        .child(button);
                    button
                }),
            );

            let kinds = [
                (
                    CurvePointKind::Vector,
                    "Vector: no handles, straight to its neighbours",
                ),
                (CurvePointKind::Free, "Free: each handle goes its own way"),
                (
                    CurvePointKind::Aligned,
                    "Aligned: opposite handles, each with its length",
                ),
                (
                    CurvePointKind::Mirrored,
                    "Mirrored: opposite handles of the same length",
                ),
            ];
            let kind_row = div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .children(kinds.into_iter().map(|(kind, label)| {
                    let pressed = active.is_some_and(|p| p.kind == kind);
                    let (lines, dot, handles) = kind_glyph(kind);
                    let ink = c.foreground;
                    let button = Button::new(SharedString::from(format!("curve-kind-{kind:?}")))
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconSm)
                        .tooltip(label)
                        .selected(pressed)
                        .disabled(disabled || active.is_none())
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, (), window, _| {
                                    guard::guard("curve editor paint", || {
                                        let o = bounds.origin;
                                        let steps: Vec<PathStep> = lines
                                            .iter()
                                            .enumerate()
                                            .map(|(i, (x, y))| {
                                                let p = Vec2::new(f64::from(*x), f64::from(*y));
                                                if i == 0 {
                                                    PathStep::Move(p)
                                                } else {
                                                    PathStep::Line(p)
                                                }
                                            })
                                            .collect();
                                        stroke_path(window, o, &steps, 1.25, ink);
                                        for (x, y) in handles {
                                            paint_dot(
                                                window,
                                                point(o.x + px(*x), o.y + px(*y)),
                                                1.5,
                                                gpui::transparent_black(),
                                                ink,
                                            );
                                        }
                                        paint_dot(
                                            window,
                                            point(o.x + px(dot.0), o.y + px(dot.1)),
                                            2.0,
                                            ink,
                                            gpui::transparent_black(),
                                        );
                                    });
                                },
                            )
                            .size(px(16.0)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.set_kind(kind, cx)));
                    #[cfg(test)]
                    let button = div()
                        .debug_selector(move || format!("curve-kind-{kind:?}"))
                        .child(button);
                    button
                }))
                .child(div().flex_1())
                .child({
                    let b = Button::new("curve-flip-x")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::FlipHorizontal)
                        .tooltip("Flip left to right")
                        .disabled(disabled)
                        .on_click(cx.listener(|this, _, _, cx| this.flip(true, cx)));
                    #[cfg(test)]
                    let b = div().debug_selector(|| "curve-flip-x".into()).child(b);
                    b
                })
                .child({
                    let b = Button::new("curve-flip-y")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::FlipVertical)
                        .tooltip("Flip upside down")
                        .disabled(disabled)
                        .on_click(cx.listener(|this, _, _, cx| this.flip(false, cx)));
                    #[cfg(test)]
                    let b = div().debug_selector(|| "curve-flip-y".into()).child(b);
                    b
                })
                .child({
                    let b = Button::new("curve-delete")
                        .variant(ButtonVariant::Destructive)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::X)
                        .tooltip("Delete point")
                        .disabled(disabled || active.is_none() || points.len() <= 2)
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx)));
                    #[cfg(test)]
                    let b = div().debug_selector(|| "curve-delete".into()).child(b);
                    b
                });

            let coordinates =
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(self.fields.iter().zip(PARTS).map(|(row, (_, label))| {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(theme.text.xs)
                            .child(div().w(px(36.0)).flex_none().text_color(muted).child(label))
                            .children(
                                row.iter()
                                    .map(|field| div().flex_1().min_w_0().child(field.clone())),
                            )
                    }));

            div()
                .flex()
                .flex_col()
                .gap_1()
                .font_family(theme.font.clone())
                .when(disabled, |d| d.opacity(0.6))
                .child(div().flex().gap_1().child(graph).child(presets))
                .child(kind_row)
                .child(coordinates)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};
    use graph_math::curve::{evaluate_curve, linear_curve};

    use super::*;

    type Changes = Rc<RefCell<Vec<Vec<CurvePoint>>>>;

    fn setup(
        cx: &mut TestAppContext,
        points: Vec<CurvePoint>,
        disabled: bool,
    ) -> (Entity<CurveEditor>, Changes, &mut VisualTestContext) {
        cx.update(crate::init);
        let (editor, cx) =
            cx.add_window_view(move |_, cx| CurveEditor::new(points, cx).disabled(disabled));
        let changes: Changes = Rc::default();
        let log = changes.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &CurveEditorEvent, _| {
                let CurveEditorEvent::Change(points) = event;
                log.borrow_mut().push(points.clone());
            })
            .detach();
        });
        cx.run_until_parked();
        (editor, changes, cx)
    }

    /// A point of the square, in window coordinates.
    fn at(cx: &mut VisualTestContext, p: Vec2) -> Point<Pixels> {
        let graph = cx.debug_bounds("curve-graph").expect("the graph is drawn");
        let plane = Plane::new(f64::from(graph.size.width));
        let px_pos = plane.project(p);
        point(
            graph.left() + px(px_pos.x as f32),
            graph.top() + px(px_pos.y as f32),
        )
    }

    fn click(cx: &mut VisualTestContext, position: Point<Pixels>, modifiers: Modifiers) {
        cx.simulate_mouse_down(position, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(position, MouseButton::Left, modifiers);
        cx.run_until_parked();
    }

    fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("the control is drawn");
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn a_path_runs_across_the_whole_square() {
        let steps = curve_path(&linear_curve(), |p| p);
        assert_eq!(steps.first(), Some(&PathStep::Move(Vec2::new(0.0, 0.0))));
        assert_eq!(steps.last(), Some(&PathStep::Line(Vec2::new(1.0, 1.0))));
        assert_eq!(steps.len(), 4);
        assert!(curve_path(&[], |p| p).is_empty());
    }

    #[test]
    fn pixels_and_the_square_map_both_ways() {
        let plane = Plane::new(150.0);
        for p in [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.25, 0.9),
            Vec2::new(1.0, 1.0),
        ] {
            let back = plane.unproject(plane.project(p));
            near(back.x, p.x);
            near(back.y, p.y);
        }
        near(plane.project(Vec2::new(0.0, 1.0)).y, INSET);
    }

    #[test]
    fn only_a_shaped_inner_side_has_handles() {
        let mut points = linear_curve();
        assert_eq!(sides(&points, 0), [HandleSide::Right]);
        assert_eq!(sides(&points, 1), [HandleSide::Left]);
        points[0].kind = CurvePointKind::Vector;
        assert!(sides(&points, 0).is_empty());
        assert!(sides(&points, 7).is_empty());
    }

    #[gpui::test]
    fn clicking_a_point_selects_it_without_changing_the_curve(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let on_end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, on_end, Modifiers::default());
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), Some(1));
        assert!(changes.borrow().is_empty());
    }

    #[gpui::test]
    fn clicking_the_curve_adds_a_point_on_it_keeping_its_shape(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let on_line = at(cx, Vec2::new(0.5, 0.5));
        click(cx, on_line, Modifiers::default());
        let sent = changes.borrow().last().cloned().expect("a change");
        assert_eq!(sent.len(), 3);
        near(sent[1].x, 0.5);
        for i in 0..=10 {
            let x = f64::from(i) / 10.0;
            near(evaluate_curve(&sent, x), x);
        }
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), Some(1));
    }

    #[gpui::test]
    fn ctrl_clicking_adds_a_point_anywhere(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        let away = at(cx, Vec2::new(0.5, 0.9));
        click(
            cx,
            away,
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        let sent = changes.borrow().last().cloned().expect("a change");
        assert_eq!(sent.len(), 3);
        near(sent[1].x, 0.5);
        near(sent[1].y, 0.9);
    }

    #[gpui::test]
    fn clicking_away_from_everything_deselects(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let on_end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, on_end, Modifiers::default());
        let away = at(cx, Vec2::new(0.8, 0.1));
        click(cx, away, Modifiers::default());
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
        assert!(changes.borrow().is_empty());
    }

    #[gpui::test]
    fn dragging_a_point_moves_it_within_the_square(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        let from = at(cx, Vec2::new(1.0, 1.0));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        let to = at(cx, Vec2::new(1.0, 0.5));
        cx.simulate_mouse_move(
            point(from.x, (from.y + to.y) / 2.0),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let sent = changes.borrow().last().cloned().expect("a change");
        near(sent[1].y, 0.5);
        near(sent[1].x, 1.0);
        // Past the square, the point stays at its edge.
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        let from = at(cx, Vec2::new(1.0, 1.0));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        for dy in [-20.0, -60.0] {
            cx.simulate_mouse_move(
                point(from.x, from.y + px(dy)),
                Some(MouseButton::Left),
                Modifiers::default(),
            );
        }
        cx.simulate_mouse_up(from, MouseButton::Left, Modifiers::default());
        near(changes.borrow().last().expect("a change")[1].y, 1.0);
    }

    #[gpui::test]
    fn dragging_a_handle_of_the_selected_point_reshapes_the_curve(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        let start = at(cx, Vec2::new(0.0, 0.0));
        click(cx, start, Modifiers::default());
        let handle = at(cx, Vec2::new(1.0 / 3.0, 1.0 / 3.0));
        cx.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
        let part_way = at(cx, Vec2::new(0.4, 0.2));
        let to = at(cx, Vec2::new(0.5, 0.1));
        cx.simulate_mouse_move(part_way, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let sent = changes.borrow().last().cloned().expect("a change");
        near(sent[0].right.x, 0.5);
        near(sent[0].right.y, 0.1);
        assert_eq!((sent[0].x, sent[0].y), (0.0, 0.0), "the point stays");
    }

    #[gpui::test]
    fn delete_removes_the_selected_point_but_keeps_two(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let middle = at(cx, Vec2::new(0.5, 0.5));
        click(cx, middle, Modifiers::default());
        assert_eq!(changes.borrow().last().map(Vec::len), Some(3));
        cx.simulate_keystrokes("delete");
        assert_eq!(changes.borrow().last().map(Vec::len), Some(2));
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        cx.simulate_keystrokes("delete");
        assert_eq!(changes.borrow().len(), 2, "two points stay");
    }

    #[gpui::test]
    fn escape_deselects(cx: &mut TestAppContext) {
        let (editor, _, cx) = setup(cx, linear_curve(), false);
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        cx.simulate_keystrokes("escape");
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
    }

    #[gpui::test]
    fn a_preset_replaces_the_curve_and_the_selection(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        click_selector(cx, "curve-preset-bell");
        let sent = changes.borrow().last().cloned().expect("a change");
        assert_eq!(sent.len(), 3);
        near(sent[1].y, 1.0);
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
    }

    #[gpui::test]
    fn a_kind_button_changes_the_selected_points_kind(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        click_selector(cx, "curve-kind-Vector");
        assert_eq!(
            changes.borrow().last().expect("a change")[1].kind,
            CurvePointKind::Vector
        );
    }

    #[gpui::test]
    fn kind_buttons_need_a_selected_point(cx: &mut TestAppContext) {
        let (_, changes, cx) = setup(cx, linear_curve(), false);
        click_selector(cx, "curve-kind-Vector");
        assert!(changes.borrow().is_empty());
    }

    #[gpui::test]
    fn flipping_mirrors_the_curve_and_follows_the_selection(cx: &mut TestAppContext) {
        let mut points = linear_curve();
        points[1].y = 0.5;
        let (editor, changes, cx) = setup(cx, points, false);
        let end = at(cx, Vec2::new(1.0, 0.5));
        click(cx, end, Modifiers::default());
        click_selector(cx, "curve-flip-x");
        let sent = changes.borrow().last().cloned().expect("a change");
        near(sent[0].x, 0.0);
        near(sent[0].y, 0.5);
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), Some(0));
        click_selector(cx, "curve-flip-y");
        near(changes.borrow().last().expect("a change")[0].y, 0.5);
        near(changes.borrow().last().expect("a change")[1].y, 1.0);
    }

    #[gpui::test]
    fn a_coordinate_typed_into_a_field_moves_the_selected_point(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), false);
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        let y = cx.read(|cx| editor.read(cx).fields[0][1].clone());
        cx.update(|_, cx| {
            y.update(cx, |_, cx| cx.emit(NumberEvent::Change(0.25)));
        });
        cx.run_until_parked();
        near(changes.borrow().last().expect("a change")[1].y, 0.25);
    }

    #[gpui::test]
    fn a_disabled_editor_ignores_the_pointer(cx: &mut TestAppContext) {
        let (editor, changes, cx) = setup(cx, linear_curve(), true);
        let on_line = at(cx, Vec2::new(0.5, 0.5));
        click(cx, on_line, Modifiers::default());
        let end = at(cx, Vec2::new(1.0, 1.0));
        click(cx, end, Modifiers::default());
        assert!(changes.borrow().is_empty());
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
    }

    #[gpui::test]
    fn showing_other_points_emits_nothing_and_drops_a_selection_out_of_range(
        cx: &mut TestAppContext,
    ) {
        let (editor, changes, cx) = setup(cx, curve_presets()[5].points.clone(), false);
        let last = at(cx, Vec2::new(1.0, 0.0));
        click(cx, last, Modifiers::default());
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), Some(2));
        cx.update(|_, cx| editor.update(cx, |e, cx| e.set_points(linear_curve(), cx)));
        assert_eq!(cx.read(|cx| editor.read(cx).selected()), None);
        assert!(changes.borrow().is_empty());
    }
}
