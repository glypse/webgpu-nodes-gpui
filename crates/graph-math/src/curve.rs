//! Curves mapping x to y within the unit square through control points
//! sorted by x. Neighbouring points make a cubic Bezier segment, shaped by
//! the first one's right handle and the second one's left handle; before the
//! first point and after the last one the curve stays flat.

use kurbo::common::{solve_cubic, solve_quadratic};
use kurbo::{CubicBez, ParamCurve, Point};
use serde::{Deserialize, Serialize};

/// Newton iterations the generated WGSL takes to find a segment's parameter
/// for an x. The shader has no closed-form solve, so this count is its only
/// accuracy knob.
pub const CURVE_SOLVE_STEPS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

fn to_point(v: Vec2) -> Point {
    Point::new(v.x, v.y)
}

fn to_vec2(p: Point) -> Vec2 {
    Vec2::new(p.x, p.y)
}

/// How a point's handles relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurvePointKind {
    /// No handles: a segment between two vector points is straight.
    Vector,
    /// Each handle goes its own way.
    Free,
    /// The handles point in opposite directions, each with its own length.
    Aligned,
    /// The handles point in opposite directions with the same length.
    Mirrored,
}

/// A control point. Its handles are offsets from it: the left one points
/// left (x <= 0), the right one right (x >= 0). A vector point keeps its
/// handles for when it's given another kind, but they don't shape the curve.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f64,
    pub y: f64,
    pub kind: CurvePointKind,
    pub left: Vec2,
    pub right: Vec2,
}

impl CurvePoint {
    fn handles(&self) -> (Vec2, Vec2) {
        match self.kind {
            CurvePointKind::Vector => (Vec2::default(), Vec2::default()),
            _ => (self.left, self.right),
        }
    }
}

/// A cubic Bezier segment: start, its two control points, end.
pub type CurveSegment = [Vec2; 4];

/// The identity: aligned handles along the diagonal, so added points are smooth.
pub fn linear_curve() -> Vec<CurvePoint> {
    let third = 1.0 / 3.0;
    let point = |x: f64| CurvePoint {
        x,
        y: x,
        kind: CurvePointKind::Aligned,
        left: Vec2::new(-third, -third),
        right: Vec2::new(third, third),
    };
    vec![point(0.0), point(1.0)]
}

/// The Bezier segments between neighbouring points. Handles reaching past
/// each other in x are shortened by the same factor, so x only grows along a
/// segment and y stays a function of x.
pub fn curve_segments(points: &[CurvePoint]) -> Vec<CurveSegment> {
    points
        .windows(2)
        .map(|pair| {
            let (a, b) = (&pair[0], &pair[1]);
            let out = a.handles().1;
            let into = b.handles().0;
            let reach_out = out.x.max(0.0);
            let reach_in = (-into.x).max(0.0);
            let span = (b.x - a.x).max(0.0);
            let reach = reach_out + reach_in;
            let fac = if reach > span { span / reach } else { 1.0 };
            [
                Vec2::new(a.x, a.y),
                Vec2::new(a.x + reach_out * fac, a.y + out.y * fac),
                Vec2::new(b.x - reach_in * fac, b.y + into.y * fac),
                Vec2::new(b.x, b.y),
            ]
        })
        .collect()
}

fn cubic(segment: &CurveSegment) -> CubicBez {
    let [p0, c0, c1, p1] = *segment;
    CubicBez {
        p0: to_point(p0),
        p1: to_point(c0),
        p2: to_point(c1),
        p3: to_point(p1),
    }
}

/// The parameter at which a segment reaches `x` (within its x range).
///
/// x is non-decreasing along a segment (see `curve_segments`), so the cubic
/// `x(t) - x` has one root in [0, 1] up to rounding. Its roots are solved
/// exactly, and the one nearest the linear guess is kept.
pub fn segment_parameter(segment: &CurveSegment, x: f64) -> f64 {
    let [p0, _, _, p1] = *segment;
    if p1.x <= p0.x {
        return 0.5;
    }
    let guess = ((x - p0.x) / (p1.x - p0.x)).clamp(0.0, 1.0);
    let [a, b, c, d] = [p0.x, segment[1].x, segment[2].x, p1.x];
    // Power-basis coefficients of x(t) - x.
    let (c0, c1, c2, c3) = (
        a - x,
        3.0 * (b - a),
        3.0 * (a - 2.0 * b + c),
        d - 3.0 * c + 3.0 * b - a,
    );
    let scale = c0.abs().max(c1.abs()).max(c2.abs()).max(c3.abs());
    // A near-zero cubic term makes the cubic formula lose precision.
    let roots: Vec<f64> = if c3.abs() > 1e-9 * scale {
        solve_cubic(c0, c1, c2, c3).iter().copied().collect()
    } else {
        solve_quadratic(c0, c1, c2).iter().copied().collect()
    };
    let mut best: Option<f64> = None;
    for t in roots {
        if !(-1e-9..=1.0 + 1e-9).contains(&t) {
            continue;
        }
        let t = t.clamp(0.0, 1.0);
        if best.is_none_or(|b| (t - guess).abs() < (b - guess).abs()) {
            best = Some(t);
        }
    }
    best.unwrap_or(guess)
}

/// The point of a segment at parameter `t`.
pub fn segment_point(segment: &CurveSegment, t: f64) -> Vec2 {
    to_vec2(cubic(segment).eval(t))
}

/// The curve's y at `x`.
pub fn evaluate_curve(points: &[CurvePoint], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if x <= first.x {
        return first.y;
    }
    for segment in curve_segments(points) {
        if x <= segment[3].x {
            return segment_point(&segment, segment_parameter(&segment, x)).y;
        }
    }
    last.y
}

/// Splits the segment starting at `index` at `x` without changing its shape:
/// the new point, and the neighbours with their handles shortened (except
/// mirrored ones, whose other handle would have to change too).
pub fn split_segment(points: &[CurvePoint], index: usize, x: f64) -> Option<[CurvePoint; 3]> {
    let segments = curve_segments(points);
    let segment = segments.get(index)?;
    let (a, b) = (points.get(index)?, points.get(index + 1)?);
    let [p0, _, _, p1] = *segment;
    let t = segment_parameter(segment, x);
    // De Casteljau's halves: the left one ends at the new point, and its
    // handle controls are the shortened handles of the neighbours.
    let curve = cubic(segment);
    let (left, right) = (curve.subsegment(0.0..t), curve.subsegment(t..1.0));
    let s = to_vec2(left.p3);
    // A handle as an offset from `to`, where `from` is a control point of a half.
    let offset = |from: Point, to: Vec2| Vec2::new(from.x - to.x, from.y - to.y);
    let straight = a.kind == CurvePointKind::Vector && b.kind == CurvePointKind::Vector;
    let added = CurvePoint {
        x: s.x.clamp(a.x, b.x.max(a.x)),
        y: s.y.clamp(0.0, 1.0),
        kind: if straight {
            CurvePointKind::Vector
        } else {
            CurvePointKind::Aligned
        },
        left: offset(left.p2, s),
        right: offset(right.p1, s),
    };
    let shaped = |kind| matches!(kind, CurvePointKind::Free | CurvePointKind::Aligned);
    let before = if shaped(a.kind) {
        CurvePoint {
            right: offset(left.p1, p0),
            ..*a
        }
    } else {
        *a
    };
    let after = if shaped(b.kind) {
        CurvePoint {
            left: offset(right.p2, p1),
            ..*b
        }
    } else {
        *b
    };
    Some([before, added, after])
}

impl Vec2 {
    fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
}

const ZERO: Vec2 = Vec2::new(0.0, 0.0);

fn clamp01(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.clamp(0.0, 1.0) }
}

/// A side of a point, whose handle goes that way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleSide {
    Left,
    Right,
}

/// The handles shaping the curve around `point`.
pub fn point_handles(point: &CurvePoint) -> (Vec2, Vec2) {
    point.handles()
}

fn replaced(points: &[CurvePoint], index: usize, point: CurvePoint) -> Vec<CurvePoint> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| if i == index { point } else { *p })
        .collect()
}

/// `point` with its handles pointing their own way in x and following its
/// kind, the handle on side `lead` deciding the direction.
fn constrained(point: CurvePoint, lead: HandleSide) -> CurvePoint {
    let left = Vec2::new(point.left.x.min(0.0), point.left.y);
    let right = Vec2::new(point.right.x.max(0.0), point.right.y);
    let (leader, follower) = match lead {
        HandleSide::Left => (left, right),
        HandleSide::Right => (right, left),
    };
    let leader_length = leader.length();
    let other = match point.kind {
        CurvePointKind::Aligned if leader_length > 0.0 => {
            let scale = -follower.length() / leader_length;
            Vec2::new(leader.x * scale, leader.y * scale)
        }
        CurvePointKind::Mirrored => Vec2::new(-leader.x, -leader.y),
        _ => follower,
    };
    match lead {
        HandleSide::Left => CurvePoint {
            left: leader,
            right: other,
            ..point
        },
        HandleSide::Right => CurvePoint {
            left: other,
            right: leader,
            ..point
        },
    }
}

/// Handles giving a smooth curve through `points[index]`: parallel to the line
/// between its neighbours, a third of the way to each.
pub fn smooth_handles(points: &[CurvePoint], index: usize) -> (Vec2, Vec2) {
    let Some(point) = points.get(index) else {
        return (ZERO, ZERO);
    };
    let before = index
        .checked_sub(1)
        .and_then(|i| points.get(i))
        .unwrap_or(point);
    let after = points.get(index + 1).unwrap_or(point);
    let dx = after.x - before.x;
    let slope = if dx > 0.0 {
        (after.y - before.y) / dx
    } else {
        0.0
    };
    let left_reach = (point.x - before.x) / 3.0;
    let right_reach = (after.x - point.x) / 3.0;
    // An end point has one neighbour: mirror the handle towards it.
    let l = if left_reach != 0.0 {
        left_reach
    } else {
        right_reach
    };
    let r = if right_reach != 0.0 {
        right_reach
    } else {
        left_reach
    };
    (Vec2::new(-l, -l * slope), Vec2::new(r, r * slope))
}

/// Add a point at (`x`, `y`), smooth. Returns the curve and the point's index.
pub fn add_curve_point(points: &[CurvePoint], x: f64, y: f64) -> (Vec<CurvePoint>, usize) {
    let (x, y) = (clamp01(x), clamp01(y));
    let index = points.iter().position(|p| p.x > x).unwrap_or(points.len());
    let added = CurvePoint {
        x,
        y,
        kind: CurvePointKind::Aligned,
        left: ZERO,
        right: ZERO,
    };
    let mut next = points.to_vec();
    next.insert(index, added);
    let (left, right) = smooth_handles(&next, index);
    let next = replaced(
        &next,
        index,
        CurvePoint {
            left,
            right,
            ..added
        },
    );
    (next, index)
}

/// Add a point on the curve at `x`, splitting its segment without changing
/// its shape: the neighbours' handles shorten, unless mirrored (their other
/// handle would have to change too). Returns the curve and the point's index.
pub fn insert_curve_point(points: &[CurvePoint], x: f64) -> (Vec<CurvePoint>, usize) {
    let segments = curve_segments(points);
    let found = segments.iter().position(|s| x >= s[0].x && x <= s[3].x);
    let Some(split) = found.and_then(|i| Some((i, split_segment(points, i, x)?))) else {
        return add_curve_point(points, x, evaluate_curve(points, x));
    };
    let (i, [before, added, after]) = split;
    let mut next = points[..i].to_vec();
    next.extend([before, added, after]);
    next.extend_from_slice(&points[i + 2..]);
    (next, i + 1)
}

/// Remove a point, keeping at least two.
pub fn remove_curve_point(points: &[CurvePoint], index: usize) -> Vec<CurvePoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, p)| *p)
        .collect()
}

/// Move a point, kept between its neighbours in x and within the square.
pub fn move_curve_point(points: &[CurvePoint], index: usize, x: f64, y: f64) -> Vec<CurvePoint> {
    let Some(point) = points.get(index) else {
        return points.to_vec();
    };
    let low = index
        .checked_sub(1)
        .and_then(|i| points.get(i))
        .map_or(0.0, |p| p.x);
    let high = points.get(index + 1).map_or(1.0, |p| p.x);
    let x = x.max(low).min(high);
    replaced(
        points,
        index,
        CurvePoint {
            x,
            y: clamp01(y),
            ..*point
        },
    )
}

/// Move one of a point's handles to `offset` from it; the other follows the point's kind.
pub fn move_curve_handle(
    points: &[CurvePoint],
    index: usize,
    side: HandleSide,
    offset: Vec2,
) -> Vec<CurvePoint> {
    let Some(point) = points
        .get(index)
        .filter(|p| p.kind != CurvePointKind::Vector)
    else {
        return points.to_vec();
    };
    let moved = match side {
        HandleSide::Left => CurvePoint {
            left: offset,
            ..*point
        },
        HandleSide::Right => CurvePoint {
            right: offset,
            ..*point
        },
    };
    replaced(points, index, constrained(moved, side))
}

/// Give a point another kind, its handles made to follow it. A point without
/// handles gets smooth ones.
pub fn set_curve_point_kind(
    points: &[CurvePoint],
    index: usize,
    kind: CurvePointKind,
) -> Vec<CurvePoint> {
    let Some(point) = points.get(index) else {
        return points.to_vec();
    };
    let bare = point.left.length() == 0.0 && point.right.length() == 0.0;
    let (left, right) = if kind != CurvePointKind::Vector && bare {
        smooth_handles(points, index)
    } else {
        (point.left, point.right)
    };
    let lead = if index + 1 == points.len() {
        HandleSide::Left
    } else {
        HandleSide::Right
    };
    replaced(
        points,
        index,
        constrained(
            CurvePoint {
                left,
                right,
                kind,
                ..*point
            },
            lead,
        ),
    )
}

/// Mirror the curve left to right: x becomes 1 - x, points keep their kind.
pub fn flip_curve_x(points: &[CurvePoint]) -> Vec<CurvePoint> {
    points
        .iter()
        .rev()
        .map(|p| CurvePoint {
            x: 1.0 - p.x,
            left: Vec2::new(-p.right.x, p.right.y),
            right: Vec2::new(-p.left.x, p.left.y),
            ..*p
        })
        .collect()
}

/// Mirror the curve top to bottom: y becomes 1 - y.
pub fn flip_curve_y(points: &[CurvePoint]) -> Vec<CurvePoint> {
    points
        .iter()
        .map(|p| CurvePoint {
            y: 1.0 - p.y,
            left: Vec2::new(p.left.x, -p.left.y),
            right: Vec2::new(p.right.x, -p.right.y),
            ..*p
        })
        .collect()
}

/// A named curve to start from.
#[derive(Clone, Debug, PartialEq)]
pub struct CurvePreset {
    pub id: &'static str,
    pub label: &'static str,
    pub points: Vec<CurvePoint>,
}

fn point(x: f64, y: f64, kind: CurvePointKind, left: Vec2, right: Vec2) -> CurvePoint {
    CurvePoint {
        x,
        y,
        kind,
        left,
        right,
    }
}

pub fn curve_presets() -> Vec<CurvePreset> {
    use CurvePointKind::{Aligned, Mirrored};
    let third = 1.0 / 3.0;
    // The distance of control points drawing a quarter circle with a cubic Bezier.
    let circle = 0.5523;
    // The handle reach of CSS's `ease-in` and `ease-out`.
    let ease = 0.42;
    let diagonal = |x: f64, y: f64| {
        point(
            x,
            y,
            Aligned,
            Vec2::new(-third, -third),
            Vec2::new(third, third),
        )
    };
    let level = |x: f64, y: f64, reach: f64| {
        point(x, y, Aligned, Vec2::new(-reach, 0.0), Vec2::new(reach, 0.0))
    };
    let falling = |x: f64, y: f64| {
        point(
            x,
            y,
            Aligned,
            Vec2::new(-third, third),
            Vec2::new(third, -third),
        )
    };
    let preset = |id, label, points| CurvePreset { id, label, points };
    vec![
        preset("linear", "Linear", linear_curve()),
        preset(
            "ease-in",
            "Ease in",
            vec![level(0.0, 0.0, ease), diagonal(1.0, 1.0)],
        ),
        preset(
            "ease-out",
            "Ease out",
            vec![diagonal(0.0, 0.0), level(1.0, 1.0, ease)],
        ),
        preset(
            "ease-in-out",
            "Ease in and out",
            vec![level(0.0, 0.0, ease), level(1.0, 1.0, ease)],
        ),
        preset(
            "round",
            "Round",
            vec![
                point(
                    0.0,
                    0.0,
                    Aligned,
                    Vec2::new(0.0, -circle),
                    Vec2::new(0.0, circle),
                ),
                point(
                    1.0,
                    1.0,
                    Aligned,
                    Vec2::new(-circle, 0.0),
                    Vec2::new(circle, 0.0),
                ),
            ],
        ),
        preset(
            "bell",
            "Bell",
            vec![
                level(0.0, 0.0, 0.25),
                point(
                    0.5,
                    1.0,
                    Mirrored,
                    Vec2::new(-0.15, 0.0),
                    Vec2::new(0.15, 0.0),
                ),
                level(1.0, 0.0, 0.25),
            ],
        ),
        preset(
            "invert",
            "Invert",
            vec![falling(0.0, 1.0), falling(1.0, 0.0)],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linear_curve_is_the_identity() {
        let points = linear_curve();
        for i in 0..=10 {
            let x = f64::from(i) / 10.0;
            assert!((evaluate_curve(&points, x) - x).abs() < 1e-6, "{x}");
        }
    }

    #[test]
    fn a_curve_stays_flat_past_its_ends() {
        let mut points = linear_curve();
        points[0].y = 0.25;
        assert_eq!(evaluate_curve(&points, -1.0), 0.25);
        assert_eq!(evaluate_curve(&points, 2.0), 1.0);
        assert_eq!(evaluate_curve(&[], 0.5), 0.0);
    }

    #[test]
    fn splitting_a_segment_keeps_its_shape() {
        let points = linear_curve();
        let Some([a, added, b]) = split_segment(&points, 0, 0.3) else {
            panic!("splits")
        };
        for i in 0..=20 {
            let x = f64::from(i) / 20.0;
            let split = evaluate_curve(&[a, added, b], x);
            assert!((split - evaluate_curve(&points, x)).abs() < 1e-5, "{x}");
        }
    }

    #[test]
    fn the_segment_parameter_reaches_every_x() {
        let skewed = [
            CurvePoint {
                x: 0.0,
                y: 0.0,
                kind: CurvePointKind::Free,
                left: ZERO,
                right: Vec2::new(0.9, -0.3),
            },
            CurvePoint {
                x: 1.0,
                y: 1.0,
                kind: CurvePointKind::Free,
                left: Vec2::new(-0.2, 0.8),
                right: ZERO,
            },
        ];
        let mut curves = vec![skewed.to_vec()];
        curves.extend(curve_presets().into_iter().map(|p| p.points));
        for points in curves {
            for segment in curve_segments(&points) {
                let (start, end) = (segment[0].x, segment[3].x);
                for s in samples() {
                    let x = start + (end - start) * s;
                    let t = segment_parameter(&segment, x);
                    near(segment_point(&segment, t).x, x, 1e-9);
                }
            }
        }
    }

    fn preset(id: &str) -> Vec<CurvePoint> {
        curve_presets()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.points)
            .expect("the preset exists")
    }

    fn corner(x: f64, y: f64) -> CurvePoint {
        CurvePoint {
            x,
            y,
            kind: CurvePointKind::Vector,
            left: Vec2::new(-0.2, 0.3),
            right: Vec2::new(0.2, 0.3),
        }
    }

    fn samples() -> impl Iterator<Item = f64> {
        (0..=20).map(|i| f64::from(i) / 20.0)
    }

    fn near(a: f64, b: f64, tolerance: f64) {
        assert!((a - b).abs() < tolerance, "{a} vs {b}");
    }

    fn near_vec(a: Vec2, b: Vec2) {
        near(a.x, b.x, 1e-6);
        near(a.y, b.y, 1e-6);
    }

    #[test]
    fn a_flat_curve_keeps_its_ends_past_vector_points() {
        let points = [corner(0.25, 0.1), corner(0.75, 0.9)];
        assert_eq!(evaluate_curve(&points, -3.0), 0.1);
        assert_eq!(evaluate_curve(&points, 0.0), 0.1);
        assert_eq!(evaluate_curve(&points, 1.0), 0.9);
        assert_eq!(evaluate_curve(&points, 4.0), 0.9);
    }

    #[test]
    fn vector_points_are_joined_by_straight_lines_whatever_handles_they_keep() {
        let points = [corner(0.0, 0.0), corner(0.5, 1.0), corner(1.0, 0.0)];
        near(evaluate_curve(&points, 0.25), 0.5, 1e-5);
        near(evaluate_curve(&points, 0.75), 0.5, 1e-5);
    }

    #[test]
    fn eases_in_and_out_symmetrically() {
        let points = preset("ease-in-out");
        near(evaluate_curve(&points, 0.5), 0.5, 1e-5);
        assert!(evaluate_curve(&points, 0.25) < 0.25);
        near(
            evaluate_curve(&points, 0.75),
            1.0 - evaluate_curve(&points, 0.25),
            1e-5,
        );
    }

    #[test]
    fn handles_reaching_past_each_other_are_shortened_so_x_keeps_growing() {
        let free = |x, y, left, right| CurvePoint {
            x,
            y,
            kind: CurvePointKind::Free,
            left,
            right,
        };
        let points = [
            free(0.0, 0.0, ZERO, Vec2::new(2.0, 0.0)),
            free(1.0, 1.0, Vec2::new(-2.0, 0.0), ZERO),
        ];
        let segments = curve_segments(&points);
        assert_eq!(segments[0][1], Vec2::new(0.5, 0.0));
        assert_eq!(segments[0][2], Vec2::new(0.5, 1.0));
        let mut previous = f64::NEG_INFINITY;
        for x in samples() {
            let y = evaluate_curve(&points, x);
            assert!(y >= previous - 1e-6);
            previous = y;
        }
    }

    #[test]
    fn inserting_a_point_on_the_curve_keeps_its_shape() {
        let points = preset("ease-in-out");
        let (split, index) = insert_curve_point(&points, 0.3);
        assert_eq!(index, 1);
        assert_eq!(split.len(), 3);
        near(split[1].x, 0.3, 1e-5);
        assert_eq!(split[1].kind, CurvePointKind::Aligned);
        for x in samples() {
            near(evaluate_curve(&split, x), evaluate_curve(&points, x), 1e-4);
        }
    }

    #[test]
    fn inserting_on_a_straight_segment_adds_a_vector_point() {
        let (points, _) = insert_curve_point(&[corner(0.0, 0.0), corner(1.0, 1.0)], 0.5);
        assert_eq!(points[1].kind, CurvePointKind::Vector);
        near(points[1].x, 0.5, 1e-9);
        near(points[1].y, 0.5, 1e-9);
    }

    #[test]
    fn adds_a_smooth_point_anywhere_kept_in_order() {
        let (points, index) = add_curve_point(&linear_curve(), 0.5, 0.9);
        assert_eq!(index, 1);
        assert_eq!(
            points.iter().map(|p| p.x).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(points[1].kind, CurvePointKind::Aligned);
        // Parallel to the line between its neighbours.
        near(points[1].right.y, points[1].right.x, 1e-5);
    }

    #[test]
    fn a_moved_point_stays_between_its_neighbours_and_in_the_square() {
        let points = [corner(0.0, 0.0), corner(0.5, 0.5), corner(1.0, 1.0)];
        let moved = move_curve_point(&points, 1, 1.2, -1.0);
        assert_eq!((moved[1].x, moved[1].y), (1.0, 0.0));
        assert_eq!(move_curve_point(&points, 0, 0.8, 0.5)[0].x, 0.5);
        assert_eq!(move_curve_point(&points, 9, 0.8, 0.5), points);
    }

    #[test]
    fn flips_left_to_right() {
        let points = insert_curve_point(&preset("ease-in"), 0.3).0;
        let flipped = flip_curve_x(&points);
        let expected: Vec<f64> = points.iter().map(|p| 1.0 - p.x).rev().collect();
        assert_eq!(flipped.iter().map(|p| p.x).collect::<Vec<_>>(), expected);
        for x in samples() {
            near(
                evaluate_curve(&flipped, x),
                evaluate_curve(&points, 1.0 - x),
                1e-5,
            );
        }
        let back = flip_curve_x(&flipped);
        for (a, b) in back.iter().zip(&points) {
            near(a.x, b.x, 1e-9);
            near_vec(a.left, b.left);
            near_vec(a.right, b.right);
        }
    }

    #[test]
    fn flips_top_to_bottom() {
        let points = insert_curve_point(&preset("ease-in"), 0.3).0;
        let flipped = flip_curve_y(&points);
        for x in samples() {
            near(
                evaluate_curve(&flipped, x),
                1.0 - evaluate_curve(&points, x),
                1e-5,
            );
        }
    }

    #[test]
    fn keeps_at_least_two_points() {
        let points = [corner(0.0, 0.0), corner(0.5, 0.5), corner(1.0, 1.0)];
        let fewer = remove_curve_point(&points, 1);
        assert_eq!(fewer.len(), 2);
        assert_eq!(remove_curve_point(&fewer, 0).len(), 2);
    }

    fn lone(kind: CurvePointKind) -> Vec<CurvePoint> {
        vec![CurvePoint {
            x: 0.5,
            y: 0.5,
            kind,
            left: Vec2::new(-0.1, 0.0),
            right: Vec2::new(0.3, 0.0),
        }]
    }

    #[test]
    fn a_free_points_other_handle_stays() {
        let moved = move_curve_handle(
            &lone(CurvePointKind::Free),
            0,
            HandleSide::Right,
            Vec2::new(0.2, 0.2),
        );
        assert_eq!(moved[0].left, Vec2::new(-0.1, 0.0));
    }

    #[test]
    fn an_aligned_points_other_handle_turns_keeping_its_length() {
        let moved = move_curve_handle(
            &lone(CurvePointKind::Aligned),
            0,
            HandleSide::Right,
            Vec2::new(0.0, 0.4),
        );
        near(moved[0].left.x, 0.0, 1e-6);
        near(moved[0].left.y, -0.1, 1e-6);
    }

    #[test]
    fn a_mirrored_points_other_handle_mirrors_it() {
        let moved = move_curve_handle(
            &lone(CurvePointKind::Mirrored),
            0,
            HandleSide::Right,
            Vec2::new(0.2, 0.1),
        );
        assert_eq!(moved[0].left, Vec2::new(-0.2, -0.1));
    }

    #[test]
    fn a_handle_never_points_the_wrong_way_in_x() {
        let moved = move_curve_handle(
            &lone(CurvePointKind::Free),
            0,
            HandleSide::Left,
            Vec2::new(0.2, 0.1),
        );
        assert_eq!(moved[0].left, Vec2::new(0.0, 0.1));
    }

    #[test]
    fn a_vector_point_has_no_handle_to_move() {
        let points = lone(CurvePointKind::Vector);
        assert_eq!(
            move_curve_handle(&points, 0, HandleSide::Right, Vec2::new(0.2, 0.1)),
            points
        );
    }

    #[test]
    fn a_point_turned_from_vector_without_handles_gets_smooth_ones() {
        let bare = |x, y| CurvePoint {
            left: ZERO,
            right: ZERO,
            ..corner(x, y)
        };
        let points = [bare(0.0, 0.0), bare(0.5, 0.25), bare(1.0, 1.0)];
        let smooth = set_curve_point_kind(&points, 1, CurvePointKind::Mirrored)[1];
        assert_eq!(smooth.kind, CurvePointKind::Mirrored);
        near_vec(smooth.right, Vec2::new(0.5 / 3.0, 0.5 / 3.0));
        near_vec(smooth.left, Vec2::new(-0.5 / 3.0, -0.5 / 3.0));
    }

    #[test]
    fn a_point_made_aligned_aligns_its_handles() {
        let points = [
            corner(0.0, 0.0),
            CurvePoint {
                x: 0.5,
                y: 0.5,
                kind: CurvePointKind::Free,
                left: Vec2::new(-0.1, 0.1),
                right: Vec2::new(0.2, 0.0),
            },
            corner(1.0, 1.0),
        ];
        let aligned = set_curve_point_kind(&points, 1, CurvePointKind::Aligned)[1];
        assert_eq!(aligned.right, Vec2::new(0.2, 0.0));
        near(aligned.left.x, -std::f64::consts::SQRT_2 * 0.1, 1e-6);
        near(aligned.left.y, 0.0, 1e-6);
    }

    #[test]
    fn the_presets_start_at_the_corners_and_stay_in_order() {
        let presets = curve_presets();
        assert_eq!(presets[0].points, linear_curve());
        for preset in presets {
            assert!(
                preset.points.windows(2).all(|w| w[0].x < w[1].x),
                "{}",
                preset.id
            );
            near(
                evaluate_curve(&preset.points, 0.0),
                preset.points[0].y,
                1e-9,
            );
        }
    }
}
