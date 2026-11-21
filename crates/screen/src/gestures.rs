//! What dragging from an area's corner does, as Blender's corners do: split
//! the area, join a neighbour, dock into another area, or swap with one.

use crate::{Area, AreaId, Axis, DockTarget, MinSize, Part, Rect, Screen, Side, neighbour_side};

/// How far a corner drag goes before it splits, joins or docks.
pub const CORNER_DRAG_PX: f64 = 16.0;
/// Within this distance of the shared edge, dragging into a neighbour joins rather than docks.
pub const JOIN_ZONE_PX: f64 = 5.0 * 36.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Split { axis: Axis, at: f64, part: Part },
    Join { target: AreaId },
    Dock { target: AreaId, place: DockTarget, factor: f64 },
    Swap { target: AreaId },
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Split { .. } => "Split",
            Outcome::Join { .. } => "Join",
            Outcome::Swap { .. } => "Swap",
            Outcome::Dock { place: DockTarget::Center, .. } => "Replace",
            Outcome::Dock { .. } => "Dock",
        }
    }
}

/// Blender's slight snap of halves, and its minimum part size.
pub fn snap_half(fraction: f64, min_fraction: f64) -> f64 {
    let snapped = if (fraction - 0.5).abs() < 0.02 { 0.5 } else { fraction };
    snapped.clamp(min_fraction, 1.0 - min_fraction)
}

/// The size of the screen in pixels, to turn fractions into distances.
#[derive(Clone, Copy, Debug)]
pub struct Pixels {
    pub width: f64,
    pub height: f64,
}

/// A point in pixels from the screen's top-left corner.
pub type Point = (f64, f64);

/// Where a split along `axis` through a point would go: the area under it and the line.
pub fn split_at<E: Clone>(
    screen: &Screen<E>,
    axis: Axis,
    pointer: Point,
    size: Pixels,
    min: MinSize,
) -> Option<(AreaId, f64)> {
    let area = screen.area_at(pointer.0 / size.width, pointer.1 / size.height)?;
    let rect = area.rect;
    let (w, h) = (rect.width(), rect.height());
    let at = match axis {
        Axis::X => rect.left + snap_half((pointer.0 / size.width - rect.left) / w, min.width / w) * w,
        Axis::Y => rect.top + snap_half((pointer.1 / size.height - rect.top) / h, min.height / h) * h,
    };
    Some((area.id, at))
}

/// What dragging the corner of `source` from `start` to `pointer` would do.
pub fn corner_outcome<E: Clone>(
    screen: &Screen<E>,
    source: &Area<E>,
    start: Point,
    pointer: Point,
    swap: bool,
    size: Pixels,
    min: MinSize,
) -> Option<Outcome> {
    let (dx, dy) = (pointer.0 - start.0, pointer.1 - start.1);
    if dx.abs().max(dy.abs()) < CORNER_DRAG_PX {
        return None;
    }
    let (x, y) = (pointer.0 / size.width, pointer.1 / size.height);
    let target = screen.area_at(x, y)?;
    if swap {
        return (target.id != source.id).then_some(Outcome::Swap { target: target.id });
    }
    let rect = target.rect;
    let (w, h) = (rect.width(), rect.height());
    if target.id == source.id {
        // Split along the dominant direction, the new area on the side the drag came from.
        let axis = if dx.abs() > dy.abs() { Axis::X } else { Axis::Y };
        let (fraction, from) = match axis {
            Axis::X => (snap_half((x - rect.left) / w, min.width / w), start.0 / size.width),
            Axis::Y => (snap_half((y - rect.top) / h, min.height / h), start.1 / size.height),
        };
        let at = match axis {
            Axis::X => rect.left + fraction * w,
            Axis::Y => rect.top + fraction * h,
        };
        let part = if from < at { Part::Before } else { Part::After };
        return Some(Outcome::Split { axis, at, part });
    }

    // Close to the edge it shares with the source, a neighbour is joined.
    if let Some(side) = neighbour_side(source, target, min) {
        let distance = match side {
            Side::Right => (x - rect.left) * size.width,
            Side::Left => (rect.right - x) * size.width,
            Side::Bottom => (y - rect.top) * size.height,
            Side::Top => (rect.bottom - y) * size.height,
        };
        let extent = if matches!(side, Side::Left | Side::Right) { w * size.width } else { h * size.height };
        if distance < JOIN_ZONE_PX.min(extent / 4.0) {
            return Some(Outcome::Join { target: target.id });
        }
    }

    // Elsewhere, docked: in its middle, or towards the nearest side along its diagonals.
    let (fx, fy) = ((x - rect.left) / w, (y - rect.top) / h);
    if fx > 0.4 && fx < 0.6 && fy > 0.4 && fy < 0.6 {
        return Some(Outcome::Dock { target: target.id, place: DockTarget::Center, factor: 0.5 });
    }
    let horizontal = (fx - 0.5).abs() * w * size.width > (fy - 0.5).abs() * h * size.height;
    let (side, position) = if horizontal {
        if fx < 0.5 { (Side::Left, fx) } else { (Side::Right, 1.0 - fx) }
    } else if fy < 0.5 {
        (Side::Top, fy)
    } else {
        (Side::Bottom, 1.0 - fy)
    };
    let min_fraction = if horizontal { min.width / w } else { min.height / h };
    Some(Outcome::Dock {
        target: target.id,
        place: DockTarget::Side(side),
        factor: snap_half(position * 2.0, min_fraction),
    })
}

/// The part of the window an outcome would affect, for highlighting.
pub fn highlight<E: Clone>(screen: &Screen<E>, source: AreaId, outcome: &Outcome) -> Option<Rect> {
    match *outcome {
        Outcome::Split { axis, at, part } => {
            let r = screen.area(source)?.rect;
            Some(match (axis, part) {
                (Axis::X, Part::Before) => Rect { right: at, ..r },
                (Axis::X, Part::After) => Rect { left: at, ..r },
                (Axis::Y, Part::Before) => Rect { bottom: at, ..r },
                (Axis::Y, Part::After) => Rect { top: at, ..r },
            })
        }
        Outcome::Join { target } | Outcome::Swap { target } => Some(screen.area(target)?.rect),
        Outcome::Dock { target, place, factor } => {
            let r = screen.area(target)?.rect;
            let (w, h) = (r.width(), r.height());
            Some(match place {
                DockTarget::Side(Side::Left) => Rect { right: r.left + w * factor, ..r },
                DockTarget::Side(Side::Right) => Rect { left: r.right - w * factor, ..r },
                DockTarget::Side(Side::Top) => Rect { bottom: r.top + h * factor, ..r },
                DockTarget::Side(Side::Bottom) => Rect { top: r.bottom - h * factor, ..r },
                DockTarget::Center => r,
            })
        }
    }
}

/// Applies an outcome, or `None` when the screen refuses it.
pub fn apply<E: Clone>(
    screen: &Screen<E>,
    source: AreaId,
    outcome: &Outcome,
    min: MinSize,
) -> Option<Screen<E>> {
    match *outcome {
        Outcome::Split { axis, at, part } => screen.split(source, axis, at, part, min).map(|(s, _)| s),
        Outcome::Join { target } => screen.join(source, target, min),
        Outcome::Dock { target, place, factor } => screen.dock(source, target, place, factor, min),
        Outcome::Swap { target } => screen.swap(source, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Pixels = Pixels { width: 1000.0, height: 800.0 };
    const MIN: MinSize = MinSize { width: 0.1, height: 0.1 };

    fn two_columns() -> Screen<u8> {
        Screen::new([(Rect::new(0.0, 0.0, 0.5, 1.0), 1), (Rect::new(0.5, 0.0, 1.0, 1.0), 2)])
    }

    fn outcome(screen: &Screen<u8>, source: u64, start: Point, pointer: Point, swap: bool) -> Option<Outcome> {
        let source = screen.area(AreaId(source))?;
        corner_outcome(screen, source, start, pointer, swap, SIZE, MIN)
    }

    #[test]
    fn a_short_drag_does_nothing() {
        let screen = two_columns();
        assert_eq!(outcome(&screen, 0, (5.0, 5.0), (10.0, 10.0), false), None);
    }

    #[test]
    fn dragging_into_the_area_splits_it_along_the_dominant_direction() {
        let screen = two_columns();
        let result = outcome(&screen, 0, (5.0, 5.0), (300.0, 40.0), false);
        let Some(Outcome::Split { axis, at, part }) = result else { panic!("{result:?}") };
        assert_eq!(axis, Axis::X);
        assert!((at - 0.3).abs() < 1e-9);
        assert_eq!(part, Part::Before);
        let down = outcome(&screen, 0, (5.0, 5.0), (40.0, 400.0), false);
        assert!(matches!(down, Some(Outcome::Split { axis: Axis::Y, part: Part::Before, .. })));
    }

    #[test]
    fn halves_snap() {
        assert_eq!(snap_half(0.49, 0.1), 0.5);
        assert_eq!(snap_half(0.3, 0.1), 0.3);
        assert_eq!(snap_half(0.01, 0.1), 0.1);
        assert_eq!(snap_half(0.99, 0.1), 0.9);
    }

    #[test]
    fn dragging_next_to_the_shared_edge_joins_the_neighbour() {
        let screen = two_columns();
        let result = outcome(&screen, 0, (5.0, 5.0), (560.0, 300.0), false);
        assert_eq!(result, Some(Outcome::Join { target: AreaId(1) }));
        let applied = apply(&screen, AreaId(0), &result.expect("outcome"), MIN).expect("joins");
        assert_eq!(applied.areas().len(), 1);
    }

    #[test]
    fn dragging_into_the_middle_of_an_area_replaces_it_and_towards_a_side_docks() {
        let screen = Screen::new([
            (Rect::new(0.0, 0.0, 0.2, 1.0), 1),
            (Rect::new(0.2, 0.0, 1.0, 0.5), 2),
            (Rect::new(0.2, 0.5, 1.0, 1.0), 3),
        ]);
        let center = outcome(&screen, 0, (5.0, 5.0), (600.0, 600.0), false);
        assert_eq!(center, Some(Outcome::Dock { target: AreaId(2), place: DockTarget::Center, factor: 0.5 }));
        let side = outcome(&screen, 0, (5.0, 5.0), (950.0, 600.0), false);
        assert!(matches!(side, Some(Outcome::Dock { place: DockTarget::Side(Side::Right), .. })), "{side:?}");
    }

    #[test]
    fn control_swaps_with_the_area_under_the_pointer() {
        let screen = two_columns();
        let result = outcome(&screen, 0, (5.0, 5.0), (800.0, 300.0), true);
        assert_eq!(result, Some(Outcome::Swap { target: AreaId(1) }));
        assert_eq!(outcome(&screen, 0, (5.0, 5.0), (300.0, 300.0), true), None);
        let swapped = apply(&screen, AreaId(0), &result.expect("outcome"), MIN).expect("swaps");
        assert_eq!(swapped.area(AreaId(0)).map(|a| a.editor), Some(2));
    }

    #[test]
    fn the_highlight_covers_what_the_outcome_changes() {
        let screen = two_columns();
        let split = Outcome::Split { axis: Axis::X, at: 0.3, part: Part::Before };
        assert_eq!(highlight(&screen, AreaId(0), &split), Some(Rect::new(0.0, 0.0, 0.3, 1.0)));
        let dock = Outcome::Dock { target: AreaId(1), place: DockTarget::Side(Side::Left), factor: 0.5 };
        assert_eq!(highlight(&screen, AreaId(0), &dock), Some(Rect::new(0.5, 0.0, 0.75, 1.0)));
    }

    #[test]
    fn a_split_follows_the_pointer_over_any_area() {
        let screen = two_columns();
        let (area, at) = split_at(&screen, Axis::X, (750.0, 100.0), SIZE, MIN).expect("over an area");
        assert_eq!(area, AreaId(1));
        assert!((at - 0.75).abs() < 1e-9);
    }
}
