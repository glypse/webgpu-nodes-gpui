use crate::geometry::Xy;

/// Where segments ab and cd cross, if they do.
pub fn segment_crossing(a: Xy, b: Xy, c: Xy, d: Xy) -> Option<Xy> {
    let (r, s) = (Xy::new(b.x - a.x, b.y - a.y), Xy::new(d.x - c.x, d.y - c.y));
    let denominator = r.x * s.y - r.y * s.x;
    if denominator == 0.0 {
        return None;
    }
    let t = ((c.x - a.x) * s.y - (c.y - a.y) * s.x) / denominator;
    let u = ((c.x - a.x) * r.y - (c.y - a.y) * r.x) / denominator;
    if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
        return None;
    }
    Some(Xy::new(a.x + t * r.x, a.y + t * r.y))
}

fn bounds(points: &[Xy]) -> (Xy, Xy) {
    points.iter().fold(
        (Xy::new(f32::INFINITY, f32::INFINITY), Xy::new(f32::NEG_INFINITY, f32::NEG_INFINITY)),
        |(lo, hi), p| (Xy::new(lo.x.min(p.x), lo.y.min(p.y)), Xy::new(hi.x.max(p.x), hi.y.max(p.y))),
    )
}

/// The first point, along `stroke`, where it crosses the polyline `line`.
pub fn first_crossing(stroke: &[Xy], line: &[Xy]) -> Option<Xy> {
    if stroke.len() < 2 || line.len() < 2 {
        return None;
    }
    let ((a_lo, a_hi), (b_lo, b_hi)) = (bounds(stroke), bounds(line));
    if a_hi.x < b_lo.x || b_hi.x < a_lo.x || a_hi.y < b_lo.y || b_hi.y < a_lo.y {
        return None;
    }
    stroke.windows(2).find_map(|p| line.windows(2).find_map(|l| segment_crossing(p[0], p[1], l[0], l[1])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_segments_meet_where_they_cross() {
        let at = segment_crossing(Xy::new(0.0, 0.0), Xy::new(2.0, 2.0), Xy::new(0.0, 2.0), Xy::new(2.0, 0.0));
        assert_eq!(at, Some(Xy::new(1.0, 1.0)));
        assert_eq!(segment_crossing(Xy::new(0.0, 0.0), Xy::new(1.0, 0.0), Xy::new(0.0, 1.0), Xy::new(1.0, 1.0)), None);
    }

    #[test]
    fn the_first_crossing_is_the_first_along_the_stroke() {
        let stroke = [Xy::new(0.0, 5.0), Xy::new(10.0, 5.0)];
        let line = [Xy::new(7.0, 0.0), Xy::new(7.0, 10.0), Xy::new(3.0, 10.0), Xy::new(3.0, 0.0)];
        assert_eq!(first_crossing(&stroke, &line), Some(Xy::new(7.0, 5.0)));
        assert_eq!(first_crossing(&stroke, &[Xy::new(0.0, 20.0), Xy::new(5.0, 20.0)]), None);
    }
}
