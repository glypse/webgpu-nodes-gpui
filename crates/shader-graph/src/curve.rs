//! Curves mapping x to y within the unit square through control points
//! sorted by x. Neighbouring points make a cubic Bezier segment, shaped by
//! the first one's right handle and the second one's left handle; before the
//! first point and after the last one the curve stays flat.

use serde::{Deserialize, Serialize};

/// Iterations solving a segment's parameter for an x: Newton steps kept
/// within a bisected bracket. The shader runs the same ones.
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

    fn lerp(self, other: Vec2, t: f64) -> Vec2 {
        Vec2::new(self.x + (other.x - self.x) * t, self.y + (other.y - self.y) * t)
    }
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

fn bezier(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}

fn bezier_slope(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * u * u * (b - a) + 6.0 * u * t * (c - b) + 3.0 * t * t * (d - c)
}

/// The parameter at which a segment reaches `x` (within its x range).
pub fn segment_parameter(segment: &CurveSegment, x: f64) -> f64 {
    let [p0, c0, c1, p1] = *segment;
    let (mut low, mut high) = (0.0, 1.0);
    let mut t = if p1.x > p0.x { ((x - p0.x) / (p1.x - p0.x)).clamp(0.0, 1.0) } else { 0.5 };
    for _ in 0..CURVE_SOLVE_STEPS {
        let error = bezier(p0.x, c0.x, c1.x, p1.x, t) - x;
        if error.abs() < 1e-7 {
            break;
        }
        if error < 0.0 {
            low = t;
        } else {
            high = t;
        }
        let slope = bezier_slope(p0.x, c0.x, c1.x, p1.x, t);
        let next = t - error / slope;
        t = if slope > 0.0 && next > low && next < high { next } else { (low + high) / 2.0 };
    }
    t
}

/// The point of a segment at parameter `t`.
pub fn segment_point(segment: &CurveSegment, t: f64) -> Vec2 {
    let [p0, c0, c1, p1] = *segment;
    Vec2::new(bezier(p0.x, c0.x, c1.x, p1.x, t), bezier(p0.y, c0.y, c1.y, p1.y, t))
}

/// The curve's y at `x`.
pub fn evaluate_curve(points: &[CurvePoint], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return 0.0 };
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
    let [p0, c0, c1, p1] = *segment;
    let t = segment_parameter(segment, x);
    let (q0, q1, q2) = (p0.lerp(c0, t), c0.lerp(c1, t), c1.lerp(p1, t));
    let (r0, r1) = (q0.lerp(q1, t), q1.lerp(q2, t));
    let s = r0.lerp(r1, t);
    let offset = |from: Vec2, to: Vec2| Vec2::new(from.x - to.x, from.y - to.y);
    let straight = a.kind == CurvePointKind::Vector && b.kind == CurvePointKind::Vector;
    let added = CurvePoint {
        x: s.x.clamp(a.x, b.x.max(a.x)),
        y: s.y.clamp(0.0, 1.0),
        kind: if straight { CurvePointKind::Vector } else { CurvePointKind::Aligned },
        left: offset(r0, s),
        right: offset(r1, s),
    };
    let shaped = |kind| matches!(kind, CurvePointKind::Free | CurvePointKind::Aligned);
    let before = if shaped(a.kind) { CurvePoint { right: offset(q0, p0), ..*a } } else { *a };
    let after = if shaped(b.kind) { CurvePoint { left: offset(q2, p1), ..*b } } else { *b };
    Some([before, added, after])
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
        let Some([a, added, b]) = split_segment(&points, 0, 0.3) else { panic!("splits") };
        for i in 0..=20 {
            let x = f64::from(i) / 20.0;
            let split = evaluate_curve(&[a, added, b], x);
            assert!((split - evaluate_curve(&points, x)).abs() < 1e-5, "{x}");
        }
    }
}
