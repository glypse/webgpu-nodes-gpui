//! Edge geometry: ports of `getBezierPath`, `getSmoothStepPath` and
//! `getStraightPath`, returning drawable segments instead of SVG strings.

use crate::geometry::Xy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Position {
    Left,
    Top,
    Right,
    Bottom,
}

impl Position {
    pub fn opposite(self) -> Position {
        match self {
            Position::Left => Position::Right,
            Position::Right => Position::Left,
            Position::Top => Position::Bottom,
            Position::Bottom => Position::Top,
        }
    }

    fn direction(self) -> Xy {
        match self {
            Position::Left => Xy::new(-1.0, 0.0),
            Position::Right => Xy::new(1.0, 0.0),
            Position::Top => Xy::new(0.0, -1.0),
            Position::Bottom => Xy::new(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Move(Xy),
    Line(Xy),
    /// Control, end.
    Quad(Xy, Xy),
    /// Control 1, control 2, end.
    Cubic(Xy, Xy, Xy),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgePath {
    pub segments: Vec<Segment>,
    pub label: Xy,
    /// Distance from the source to the label, per axis.
    pub offset: Xy,
}

#[derive(Clone, Copy, Debug)]
pub struct PathParams {
    pub source: Xy,
    pub source_position: Position,
    pub target: Xy,
    pub target_position: Position,
}

impl PathParams {
    pub fn new(source: Xy, source_position: Position, target: Xy, target_position: Position) -> Self {
        Self { source, source_position, target, target_position }
    }
}

/// The midpoint and its distance from the source, as `getEdgeCenter`.
pub fn edge_center(source: Xy, target: Xy) -> (Xy, Xy) {
    let ox = (target.x - source.x).abs() / 2.0;
    let cx = if target.x < source.x { target.x + ox } else { target.x - ox };
    let oy = (target.y - source.y).abs() / 2.0;
    let cy = if target.y < source.y { target.y + oy } else { target.y - oy };
    (Xy::new(cx, cy), Xy::new(ox, oy))
}

fn control_offset(distance: f32, curvature: f32) -> f32 {
    if distance >= 0.0 {
        0.5 * distance
    } else {
        curvature * 25.0 * (-distance).sqrt()
    }
}

fn control_with_curvature(pos: Position, a: Xy, b: Xy, c: f32) -> Xy {
    match pos {
        Position::Left => Xy::new(a.x - control_offset(a.x - b.x, c), a.y),
        Position::Right => Xy::new(a.x + control_offset(b.x - a.x, c), a.y),
        Position::Top => Xy::new(a.x, a.y - control_offset(a.y - b.y, c)),
        Position::Bottom => Xy::new(a.x, a.y + control_offset(b.y - a.y, c)),
    }
}

pub const DEFAULT_CURVATURE: f32 = 0.25;

pub fn bezier_path(p: PathParams, curvature: f32) -> EdgePath {
    let c1 = control_with_curvature(p.source_position, p.source, p.target, curvature);
    let c2 = control_with_curvature(p.target_position, p.target, p.source, curvature);
    let label = Xy::new(
        p.source.x * 0.125 + c1.x * 0.375 + c2.x * 0.375 + p.target.x * 0.125,
        p.source.y * 0.125 + c1.y * 0.375 + c2.y * 0.375 + p.target.y * 0.125,
    );
    EdgePath {
        segments: vec![Segment::Move(p.source), Segment::Cubic(c1, c2, p.target)],
        label,
        offset: Xy::new((label.x - p.source.x).abs(), (label.y - p.source.y).abs()),
    }
}

pub fn straight_path(source: Xy, target: Xy) -> EdgePath {
    let (label, offset) = edge_center(source, target);
    EdgePath {
        segments: vec![Segment::Move(source), Segment::Line(target)],
        label,
        offset,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StepOptions {
    pub border_radius: f32,
    pub offset: f32,
    pub step_position: f32,
    pub center: (Option<f32>, Option<f32>),
}

impl Default for StepOptions {
    fn default() -> Self {
        Self { border_radius: 5.0, offset: 20.0, step_position: 0.5, center: (None, None) }
    }
}

fn axis(p: Xy, horizontal: bool) -> f32 {
    if horizontal { p.x } else { p.y }
}

fn set_axis(p: &mut Xy, horizontal: bool, v: f32) {
    if horizontal {
        p.x = v;
    } else {
        p.y = v;
    }
}

fn step_points(p: PathParams, opts: StepOptions) -> (Vec<Xy>, Xy, Xy) {
    let source_dir = p.source_position.direction();
    let target_dir = p.target_position.direction();
    let source_gapped = Xy::new(
        p.source.x + source_dir.x * opts.offset,
        p.source.y + source_dir.y * opts.offset,
    );
    let target_gapped = Xy::new(
        p.target.x + target_dir.x * opts.offset,
        p.target.y + target_dir.y * opts.offset,
    );
    let dir = if matches!(p.source_position, Position::Left | Position::Right) {
        if source_gapped.x < target_gapped.x { Xy::new(1.0, 0.0) } else { Xy::new(-1.0, 0.0) }
    } else if source_gapped.y < target_gapped.y {
        Xy::new(0.0, 1.0)
    } else {
        Xy::new(0.0, -1.0)
    };
    let horizontal = dir.x != 0.0;
    let curr_dir = axis(dir, horizontal);
    let (_, default_offset) = edge_center(p.source, p.target);

    let mut points: Vec<Xy>;
    let center;
    let mut source_gap_offset = Xy::ZERO;
    let mut target_gap_offset = Xy::ZERO;

    if axis(source_dir, horizontal) * axis(target_dir, horizontal) == -1.0 {
        let (cx, cy) = if horizontal {
            (
                opts.center.0.unwrap_or(
                    source_gapped.x + (target_gapped.x - source_gapped.x) * opts.step_position,
                ),
                opts.center.1.unwrap_or((source_gapped.y + target_gapped.y) / 2.0),
            )
        } else {
            (
                opts.center.0.unwrap_or((source_gapped.x + target_gapped.x) / 2.0),
                opts.center.1.unwrap_or(
                    source_gapped.y + (target_gapped.y - source_gapped.y) * opts.step_position,
                ),
            )
        };
        center = Xy::new(cx, cy);
        let vertical_split = vec![Xy::new(cx, source_gapped.y), Xy::new(cx, target_gapped.y)];
        let horizontal_split = vec![Xy::new(source_gapped.x, cy), Xy::new(target_gapped.x, cy)];
        points = if axis(source_dir, horizontal) == curr_dir {
            if horizontal { vertical_split } else { horizontal_split }
        } else if horizontal {
            horizontal_split
        } else {
            vertical_split
        };
    } else {
        let source_target = vec![Xy::new(source_gapped.x, target_gapped.y)];
        let target_source = vec![Xy::new(target_gapped.x, source_gapped.y)];
        points = if horizontal {
            if source_dir.x == curr_dir { target_source.clone() } else { source_target.clone() }
        } else if source_dir.y == curr_dir {
            source_target.clone()
        } else {
            target_source.clone()
        };

        if p.source_position == p.target_position {
            let diff = (axis(p.source, horizontal) - axis(p.target, horizontal)).abs();
            if diff <= opts.offset {
                let gap = (opts.offset - 1.0).min(opts.offset - diff);
                if axis(source_dir, horizontal) == curr_dir {
                    let sign = if axis(source_gapped, horizontal) > axis(p.source, horizontal) {
                        -1.0
                    } else {
                        1.0
                    };
                    set_axis(&mut source_gap_offset, horizontal, sign * gap);
                } else {
                    let sign = if axis(target_gapped, horizontal) > axis(p.target, horizontal) {
                        -1.0
                    } else {
                        1.0
                    };
                    set_axis(&mut target_gap_offset, horizontal, sign * gap);
                }
            }
        }

        if p.source_position != p.target_position {
            let opposite = !horizontal;
            let is_same_dir = axis(source_dir, horizontal) == axis(target_dir, opposite);
            let gt = axis(source_gapped, opposite) > axis(target_gapped, opposite);
            let lt = axis(source_gapped, opposite) < axis(target_gapped, opposite);
            let flip = (axis(source_dir, horizontal) == 1.0
                && ((!is_same_dir && gt) || (is_same_dir && lt)))
                || (axis(source_dir, horizontal) != 1.0
                    && ((!is_same_dir && lt) || (is_same_dir && gt)));
            if flip {
                points = if horizontal { source_target } else { target_source };
            }
        }

        let sgp = source_gapped + source_gap_offset;
        let tgp = target_gapped + target_gap_offset;
        let max_x = (sgp.x - points[0].x).abs().max((tgp.x - points[0].x).abs());
        let max_y = (sgp.y - points[0].y).abs().max((tgp.y - points[0].y).abs());
        center = if max_x >= max_y {
            Xy::new((sgp.x + tgp.x) / 2.0, points[0].y)
        } else {
            Xy::new(points[0].x, (sgp.y + tgp.y) / 2.0)
        };
    }

    let gapped_source = source_gapped + source_gap_offset;
    let gapped_target = target_gapped + target_gap_offset;
    let mut all = vec![p.source];
    if gapped_source != points[0] {
        all.push(gapped_source);
    }
    let last = *points.last().unwrap();
    all.extend(points);
    if gapped_target != last {
        all.push(gapped_target);
    }
    all.push(p.target);
    (all, center, default_offset)
}

fn bend(a: Xy, b: Xy, c: Xy, size: f32, out: &mut Vec<Segment>) {
    let bend_size = (a.distance(b) / 2.0).min(b.distance(c) / 2.0).min(size);
    if (a.x == b.x && b.x == c.x) || (a.y == b.y && b.y == c.y) {
        out.push(Segment::Line(b));
        return;
    }
    if a.y == b.y {
        let x_dir = if a.x < c.x { -1.0 } else { 1.0 };
        let y_dir = if a.y < c.y { 1.0 } else { -1.0 };
        out.push(Segment::Line(Xy::new(b.x + bend_size * x_dir, b.y)));
        out.push(Segment::Quad(b, Xy::new(b.x, b.y + bend_size * y_dir)));
    } else {
        let x_dir = if a.x < c.x { 1.0 } else { -1.0 };
        let y_dir = if a.y < c.y { -1.0 } else { 1.0 };
        out.push(Segment::Line(Xy::new(b.x, b.y + bend_size * y_dir)));
        out.push(Segment::Quad(b, Xy::new(b.x + bend_size * x_dir, b.y)));
    }
}

/// Orthogonal routing with rounded corners; a zero radius gives step edges.
pub fn smooth_step_path(p: PathParams, opts: StepOptions) -> EdgePath {
    let (points, label, offset) = step_points(p, opts);
    let mut segments = vec![Segment::Move(points[0])];
    for i in 1..points.len() - 1 {
        bend(points[i - 1], points[i], points[i + 1], opts.border_radius, &mut segments);
    }
    segments.push(Segment::Line(*points.last().unwrap()));
    EdgePath { segments, label, offset }
}

/// Polyline approximation, for hit testing and bounds.
pub fn flatten(segments: &[Segment], steps: usize) -> Vec<Xy> {
    let mut out = Vec::new();
    let mut current = Xy::ZERO;
    for s in segments {
        match *s {
            Segment::Move(p) | Segment::Line(p) => {
                out.push(p);
                current = p;
            }
            Segment::Quad(c, e) => {
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let u = 1.0 - t;
                    out.push(Xy::new(
                        u * u * current.x + 2.0 * u * t * c.x + t * t * e.x,
                        u * u * current.y + 2.0 * u * t * c.y + t * t * e.y,
                    ));
                }
                current = e;
            }
            Segment::Cubic(c1, c2, e) => {
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let u = 1.0 - t;
                    out.push(Xy::new(
                        u * u * u * current.x
                            + 3.0 * u * u * t * c1.x
                            + 3.0 * u * t * t * c2.x
                            + t * t * t * e.x,
                        u * u * u * current.y
                            + 3.0 * u * u * t * c1.y
                            + 3.0 * u * t * t * c2.y
                            + t * t * t * e.y,
                    ));
                }
                current = e;
            }
        }
    }
    out
}

/// Distance from `p` to a polyline.
pub fn distance_to_polyline(p: Xy, line: &[Xy]) -> f32 {
    line.windows(2)
        .map(|w| distance_to_segment(p, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

fn distance_to_segment(p: Xy, a: Xy, b: Xy) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(Xy::new(a.x + ab.x * t, a.y + ab.y * t))
}

/// The tangent angle (radians) of the cubic's midpoint, for direction arrows.
pub fn midpoint_angle(source: Xy, c1: Xy, c2: Xy, target: Xy) -> f32 {
    (target.y + c2.y - c1.y - source.y).atan2(target.x + c2.x - c1.x - source.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(sp: Position, tp: Position) -> PathParams {
        PathParams::new(Xy::new(0.0, 20.0), sp, Xy::new(150.0, 100.0), tp)
    }

    #[test]
    fn bezier_matches_the_reference_controls() {
        let path = bezier_path(params(Position::Right, Position::Left), DEFAULT_CURVATURE);
        // Source control: x1 + 0.5 * (x2 - x1); target control mirrors it.
        assert_eq!(
            path.segments[1],
            Segment::Cubic(Xy::new(75.0, 20.0), Xy::new(75.0, 100.0), Xy::new(150.0, 100.0))
        );
        assert_eq!(path.label, Xy::new(75.0, 60.0));
        assert_eq!(path.offset, Xy::new(75.0, 40.0));
    }

    #[test]
    fn bezier_loops_back_when_the_target_is_behind() {
        let p = PathParams::new(Xy::new(100.0, 0.0), Position::Right, Xy::new(0.0, 0.0), Position::Left);
        let path = bezier_path(p, DEFAULT_CURVATURE);
        let Segment::Cubic(c1, c2, _) = path.segments[1] else { panic!() };
        // 0.25 * 25 * sqrt(100)
        assert!((c1.x - 162.5).abs() < 1e-3);
        assert!((c2.x - -62.5).abs() < 1e-3);
    }

    #[test]
    fn straight_edge_label_is_the_midpoint() {
        let path = straight_path(Xy::new(0.0, 0.0), Xy::new(10.0, 20.0));
        assert_eq!(path.label, Xy::new(5.0, 10.0));
        assert_eq!(path.segments.len(), 2);
    }

    #[test]
    fn smooth_step_goes_through_the_centre_column() {
        let path = smooth_step_path(params(Position::Right, Position::Left), StepOptions::default());
        let points = flatten(&path.segments, 4);
        assert_eq!(points[0], Xy::new(0.0, 20.0));
        assert_eq!(*points.last().unwrap(), Xy::new(150.0, 100.0));
        // The vertical run sits halfway between the gapped ends.
        assert!(points.iter().any(|p| (p.x - 75.0).abs() < 1e-3));
        assert_eq!(path.label.x, 75.0);
    }

    #[test]
    fn step_edge_has_square_corners() {
        let opts = StepOptions { border_radius: 0.0, ..StepOptions::default() };
        let path = smooth_step_path(params(Position::Right, Position::Left), opts);
        let pts = flatten(&path.segments, 4);
        assert!(pts.iter().any(|p| *p == Xy::new(75.0, 20.0)));
        assert!(pts.iter().any(|p| *p == Xy::new(75.0, 100.0)));
    }

    #[test]
    fn mixed_positions_route_around() {
        let path = smooth_step_path(params(Position::Right, Position::Top), StepOptions::default());
        let pts = flatten(&path.segments, 4);
        assert_eq!(*pts.last().unwrap(), Xy::new(150.0, 100.0));
    }

    #[test]
    fn hit_testing_measures_distance_to_the_curve() {
        let path = bezier_path(params(Position::Right, Position::Left), DEFAULT_CURVATURE);
        let line = flatten(&path.segments, 32);
        assert!(distance_to_polyline(path.label, &line) < 1.0);
        assert!(distance_to_polyline(Xy::new(75.0, 300.0), &line) > 100.0);
    }
}
