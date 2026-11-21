//! Editor panels tiling a window, after Blender's screens: every area is a
//! rectangle, neighbours share edges, and the editing operations (split, join,
//! close, dock, edge moves) keep the window tiled. Coordinates are fractions
//! of the window, from its top-left corner; a value shared by several areas is
//! always the same number, so equality tells which edges coincide.
//!
//! Operations are pure: they return a new screen, or `None` when refused.

use serde::{Deserialize, Serialize};

mod gestures;
mod view;

#[cfg(test)]
mod tests;

pub use gestures::{CORNER_DRAG_PX, JOIN_ZONE_PX, Outcome, Pixels as ScreenPixels, corner_outcome, split_at};
pub use view::{AreaContent, Editor, HEADER_HEIGHT, ScreenEvent, ScreenView, toolbar_slot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AreaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Rect {
    pub const FULL: Rect = Rect::new(0.0, 0.0, 1.0, 1.0);

    pub const fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    fn get(&self, side: Side) -> f64 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
            Side::Top => self.top,
            Side::Bottom => self.bottom,
        }
    }

    fn set(&mut self, side: Side, value: f64) {
        match side {
            Side::Left => self.left = value,
            Side::Right => self.right = value,
            Side::Top => self.top = value,
            Side::Bottom => self.bottom = value,
        }
    }

    /// The start and end sides along `axis`.
    fn bounds_on(axis: Axis) -> (Side, Side) {
        match axis {
            Axis::X => (Side::Left, Side::Right),
            Axis::Y => (Side::Top, Side::Bottom),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area<E> {
    pub id: AreaId,
    pub rect: Rect,
    pub editor: E,
}

/// "X" is a vertical line (left/right neighbours), "Y" a horizontal one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
}

/// Where a neighbour lies, seen from an area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn horizontal(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

/// Which part of a split area the new one takes: left/top or right/bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Before,
    After,
}

/// Sizes as fractions of the window: the smallest area, or how close counts.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MinSize {
    pub width: f64,
    pub height: f64,
}

impl MinSize {
    pub const NONE: MinSize = MinSize {
        width: 0.0,
        height: 0.0,
    };

    fn along(&self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.width,
            Axis::Y => self.height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockTarget {
    Side(Side),
    Center,
}

/// An area's side lying on an edge being moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeSide {
    pub area: AreaId,
    pub side: Side,
}

/// A set of area sides on one line, moved together.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeSelection {
    pub axis: Axis,
    pub position: f64,
    pub sides: Vec<EdgeSide>,
}

/// Tells apart floats meant to be the same coordinate from distinct ones.
const EPSILON: f64 = 1e-9;

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < EPSILON
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Screen<E> {
    areas: Vec<Area<E>>,
    next_id: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    Unsized(AreaId),
    OutOfWindow(AreaId),
    DuplicateId(AreaId),
    Overlap(AreaId, AreaId),
    NotCovered,
}

impl<E: Clone> Screen<E> {
    /// A screen of the given areas, which must tile the window.
    pub fn new(areas: impl IntoIterator<Item = (Rect, E)>) -> Self {
        let areas: Vec<Area<E>> = areas
            .into_iter()
            .enumerate()
            .map(|(i, (rect, editor))| Area {
                id: AreaId(i as u64),
                rect,
                editor,
            })
            .collect();
        let next_id = areas.len() as u64;
        Self { areas, next_id }
    }

    pub fn areas(&self) -> &[Area<E>] {
        &self.areas
    }

    pub fn area(&self, id: AreaId) -> Option<&Area<E>> {
        self.areas.iter().find(|a| a.id == id)
    }

    /// The area containing a point (window fractions).
    pub fn area_at(&self, x: f64, y: f64) -> Option<&Area<E>> {
        self.areas.iter().find(|a| a.rect.contains(x, y))
    }

    /// Checks what a deserialized screen must hold: sized areas of their own
    /// ids, within the window, covering it exactly once.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.areas.is_empty() {
            return Err(Invalid::Empty);
        }
        let mut total = 0.0;
        for (i, a) in self.areas.iter().enumerate() {
            let r = a.rect;
            if r.width() <= EPSILON || r.height() <= EPSILON {
                return Err(Invalid::Unsized(a.id));
            }
            if r.left < -EPSILON
                || r.top < -EPSILON
                || r.right > 1.0 + EPSILON
                || r.bottom > 1.0 + EPSILON
            {
                return Err(Invalid::OutOfWindow(a.id));
            }
            total += r.width() * r.height();
            for b in &self.areas[..i] {
                if b.id == a.id {
                    return Err(Invalid::DuplicateId(a.id));
                }
                let overlap_x = r.right.min(b.rect.right) - r.left.max(b.rect.left);
                let overlap_y = r.bottom.min(b.rect.bottom) - r.top.max(b.rect.top);
                if overlap_x > EPSILON && overlap_y > EPSILON {
                    return Err(Invalid::Overlap(b.id, a.id));
                }
            }
        }
        if (total - 1.0).abs() > 1e-6 {
            return Err(Invalid::NotCovered);
        }
        Ok(())
    }

    fn fresh_id(&mut self) -> AreaId {
        // Deserialized screens may hold ids past the counter.
        let max = self.areas.iter().map(|a| a.id.0 + 1).max().unwrap_or(0);
        self.next_id = self.next_id.max(max);
        let id = AreaId(self.next_id);
        self.next_id += 1;
        id
    }

    fn area_mut(&mut self, id: AreaId) -> Option<&mut Area<E>> {
        self.areas.iter_mut().find(|a| a.id == id)
    }

    fn remove(&mut self, id: AreaId) {
        self.areas.retain(|a| a.id != id);
    }

    pub fn set_editor(&mut self, id: AreaId, editor: E) -> bool {
        match self.area_mut(id) {
            Some(area) => {
                area.editor = editor;
                true
            }
            None => false,
        }
    }

    /// Exchange two areas' editors, as Blender's Ctrl-drag from a corner does.
    pub fn swap(&self, a: AreaId, b: AreaId) -> Option<Self> {
        let first = self.area(a)?.editor.clone();
        let second = self.area(b)?.editor.clone();
        if a == b {
            return None;
        }
        let mut next = self.clone();
        next.set_editor(a, second);
        next.set_editor(b, first);
        Some(next)
    }

    /// Split an area along `axis` at `at`. The new area, a copy of the
    /// editor, takes `part`. Refused when a part would be smaller than `min`.
    pub fn split(
        &self,
        id: AreaId,
        axis: Axis,
        at: f64,
        part: Part,
        min: MinSize,
    ) -> Option<(Self, AreaId)> {
        let area = self.area(id)?;
        let rect = area.rect;
        let (lo, hi) = Rect::bounds_on(axis);
        let (start, end, size) = (rect.get(lo), rect.get(hi), min.along(axis));
        if at - start < size || end - at < size || at <= start || at >= end {
            return None;
        }
        let mut before = rect;
        before.set(hi, at);
        let mut after = rect;
        after.set(lo, at);
        let (created, kept) = match part {
            Part::Before => (before, after),
            Part::After => (after, before),
        };
        let editor = area.editor.clone();
        let mut next = self.clone();
        let new_id = next.fresh_id();
        next.area_mut(id)?.rect = kept;
        next.areas.push(Area {
            id: new_id,
            rect: created,
            editor,
        });
        Some((next, new_id))
    }

    /// Move every coordinate equal to `from` on `axis` to `to`, keeping edges shared.
    fn move_line(&mut self, axis: Axis, from: f64, to: f64) {
        let (lo, hi) = Rect::bounds_on(axis);
        for a in &mut self.areas {
            if same(a.rect.get(lo), from) {
                a.rect.set(lo, to);
            }
            if same(a.rect.get(hi), from) {
                a.rect.set(hi, to);
            }
        }
    }

    fn is_tiled(&self) -> bool {
        self.areas
            .iter()
            .all(|a| a.rect.width() > EPSILON && a.rect.height() > EPSILON)
    }

    /// Line up two neighbours' ends by moving the edges at them (and every
    /// edge sharing their coordinates) to their average, when they differ by
    /// less than the tolerance.
    fn align_neighbours(
        &self,
        a: AreaId,
        b: AreaId,
        side: Side,
        tolerance: MinSize,
    ) -> Option<Self> {
        let ra = self.area(a)?.rect;
        let rb = self.area(b)?.rect;
        let axis = if side.horizontal() { Axis::Y } else { Axis::X };
        let limit = tolerance.along(axis);
        let (o1, o2) = offsets(&ra, &rb, side);
        if o1.abs() >= limit || o2.abs() >= limit {
            return None;
        }
        let (lo, hi) = Rect::bounds_on(axis);
        let start = (ra.get(lo) + rb.get(lo)) / 2.0;
        let end = (ra.get(hi) + rb.get(hi)) / 2.0;
        let mut next = self.clone();
        next.move_line(axis, ra.get(lo), start);
        next.move_line(axis, rb.get(lo), start);
        next.move_line(axis, ra.get(hi), end);
        next.move_line(axis, rb.get(hi), end);
        next.is_tiled().then_some(next)
    }

    /// `keep` grows over `remove`, which goes; their shared edge must span both.
    fn join_aligned(&self, keep: AreaId, remove: AreaId, tolerance: MinSize) -> Option<Self> {
        let side = neighbour_side(self.area(keep)?, self.area(remove)?, tolerance)?;
        let mut next = self.align_neighbours(keep, remove, side, tolerance)?;
        let r = next.area(remove)?.rect;
        let k = &mut next.area_mut(keep)?.rect;
        *k = Rect::new(
            k.left.min(r.left),
            k.top.min(r.top),
            k.right.max(r.right),
            k.bottom.max(r.bottom),
        );
        next.remove(remove);
        Some(next)
    }

    /// `keep` grows over its neighbour `remove`, which goes. Where their
    /// shared edge doesn't span both, the overhang is cut off first: a cut-off
    /// part of `remove` stays as an area of its own; one of `keep` is closed
    /// (its neighbours take its space). Ends closer than the tolerance are
    /// lined up instead.
    pub fn join(&self, keep: AreaId, remove: AreaId, tolerance: MinSize) -> Option<Self> {
        self.join_with(keep, remove, tolerance, false)
    }

    fn join_with(
        &self,
        keep: AreaId,
        remove: AreaId,
        tolerance: MinSize,
        close_remainders: bool,
    ) -> Option<Self> {
        let k = self.area(keep)?;
        let r = self.area(remove)?;
        let side = neighbour_side(k, r, tolerance)?;
        let axis = if side.horizontal() { Axis::Y } else { Axis::X };
        let limit = tolerance.along(axis);
        let (o1, o2) = offsets(&k.rect, &r.rect, side);
        let (lo, hi) = Rect::bounds_on(axis);
        let (k_rect, r_rect) = (k.rect, r.rect);

        let mut next = self.clone();
        let mut keep_pieces = Vec::new();
        let mut remove_pieces = Vec::new();
        // Overhang at the start: the one starting first is cut where the other starts.
        if o1.abs() >= limit {
            let cut_keep = o1 > 0.0;
            let (target, at) = if cut_keep {
                (keep, r_rect.get(lo))
            } else {
                (remove, k_rect.get(lo))
            };
            let (cut, piece) = next.split(target, axis, at, Part::Before, MinSize::NONE)?;
            next = cut;
            if cut_keep {
                &mut keep_pieces
            } else {
                &mut remove_pieces
            }
            .push(piece);
        }
        // Overhang at the end: the one ending last is cut where the other ends.
        if o2.abs() >= limit {
            let cut_keep = o2 < 0.0;
            let (target, at) = if cut_keep {
                (keep, r_rect.get(hi))
            } else {
                (remove, k_rect.get(hi))
            };
            let (cut, piece) = next.split(target, axis, at, Part::After, MinSize::NONE)?;
            next = cut;
            if cut_keep {
                &mut keep_pieces
            } else {
                &mut remove_pieces
            }
            .push(piece);
        }

        next = next.join_aligned(keep, remove, tolerance)?;
        let closing = if close_remainders {
            keep_pieces.into_iter().chain(remove_pieces).collect()
        } else {
            keep_pieces
        };
        for piece in closing {
            if let Some(closed) = next.close_except(piece, tolerance, Some(keep)) {
                next = closed;
            }
        }
        Some(next)
    }

    /// Remove an area, a neighbour growing over it: the one sharing the most
    /// of its edge (relative to the longer of the two).
    pub fn close(&self, id: AreaId, tolerance: MinSize) -> Option<Self> {
        self.close_except(id, tolerance, None)
    }

    fn close_except(&self, id: AreaId, tolerance: MinSize, except: Option<AreaId>) -> Option<Self> {
        let area = self.area(id)?;
        if self.areas.len() < 2 {
            return None;
        }
        let mut best: Option<(AreaId, f64)> = None;
        for neighbour in &self.areas {
            if neighbour.id == id || Some(neighbour.id) == except {
                continue;
            }
            let Some(side) = neighbour_side(area, neighbour, tolerance) else {
                continue;
            };
            let length = |r: &Rect| {
                if side.horizontal() {
                    r.height()
                } else {
                    r.width()
                }
            };
            let (a, b) = (length(&area.rect), length(&neighbour.rect));
            let alignment = a.min(b) / a.max(b);
            if best.is_none_or(|(_, best)| alignment > best) {
                best = Some((neighbour.id, alignment));
            }
        }
        self.join_with(best?.0, id, tolerance, true)
    }

    /// Move `source`'s editor into `target`: into a new part of it on a side
    /// (`factor` of its size), or in its place for the centre; `source` closes.
    pub fn dock(
        &self,
        source: AreaId,
        target: AreaId,
        place: DockTarget,
        factor: f64,
        tolerance: MinSize,
    ) -> Option<Self> {
        let editor = self.area(source)?.editor.clone();
        let rect = self.area(target)?.rect;
        if source == target {
            return None;
        }
        let mut next = self.clone();
        let mut slot = target;
        if let DockTarget::Side(side) = place {
            let axis = if side.horizontal() { Axis::X } else { Axis::Y };
            let (at, part) = match side {
                Side::Left => (rect.left + rect.width() * factor, Part::Before),
                Side::Right => (rect.right - rect.width() * factor, Part::After),
                Side::Top => (rect.top + rect.height() * factor, Part::Before),
                Side::Bottom => (rect.bottom - rect.height() * factor, Part::After),
            };
            let (split, id) = next.split(target, axis, at, part, tolerance)?;
            next = split;
            slot = id;
        }
        next.set_editor(slot, editor);
        next.close_except(source, tolerance, Some(slot))
            .or_else(|| next.close(source, tolerance))
    }

    /// The sides of areas lying on the line at `position` (not the window's
    /// border), with their extent along it.
    fn sides_on_line(&self, axis: Axis, position: f64) -> Vec<(EdgeSide, f64, f64)> {
        let mut sides = Vec::new();
        if same(position, 0.0) || same(position, 1.0) {
            return sides;
        }
        let (lo, hi) = Rect::bounds_on(axis);
        for a in &self.areas {
            for side in [lo, hi] {
                if !same(a.rect.get(side), position) {
                    continue;
                }
                let (start, end) = match axis {
                    Axis::X => (a.rect.top, a.rect.bottom),
                    Axis::Y => (a.rect.left, a.rect.right),
                };
                sides.push((EdgeSide { area: a.id, side }, start, end));
            }
        }
        sides
    }

    /// The interior edge within `threshold` of a point, as Blender picks it:
    /// the sides on that line connected to the one under the point (through
    /// T-junctions), or every side on it with `extend`.
    pub fn edge_at(
        &self,
        x: f64,
        y: f64,
        threshold: MinSize,
        extend: bool,
    ) -> Option<EdgeSelection> {
        for area in &self.areas {
            let r = area.rect;
            let lines = [
                (Axis::X, r.left, x, r.top, r.bottom),
                (Axis::X, r.right, x, r.top, r.bottom),
                (Axis::Y, r.top, y, r.left, r.right),
                (Axis::Y, r.bottom, y, r.left, r.right),
            ];
            for (axis, position, pointer, start, end) in lines {
                let along = if axis == Axis::X { y } else { x };
                if (pointer - position).abs() > threshold.along(axis)
                    || along < start
                    || along > end
                {
                    continue;
                }
                let sides = self.sides_on_line(axis, position);
                if sides.is_empty() {
                    continue;
                }
                if extend {
                    return Some(EdgeSelection {
                        axis,
                        position,
                        sides: sides.into_iter().map(|(s, _, _)| s).collect(),
                    });
                }
                let (mut lo, mut hi) = (start, end);
                let mut picked = vec![false; sides.len()];
                let mut grew = true;
                while grew {
                    grew = false;
                    for (i, (_, s, e)) in sides.iter().enumerate() {
                        if picked[i] || *e < lo - EPSILON || *s > hi + EPSILON {
                            continue;
                        }
                        picked[i] = true;
                        lo = lo.min(*s);
                        hi = hi.max(*e);
                        grew = true;
                    }
                }
                return Some(EdgeSelection {
                    axis,
                    position,
                    sides: sides
                        .into_iter()
                        .zip(picked)
                        .filter_map(|((s, _, _), p)| p.then_some(s))
                        .collect(),
                });
            }
        }
        None
    }

    /// How far an edge can move, keeping every area it bounds at least `min`.
    pub fn edge_limits(&self, edge: &EdgeSelection, min: MinSize) -> (f64, f64) {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        let size = min.along(edge.axis);
        for EdgeSide { area, side } in &edge.sides {
            let Some(area) = self.area(*area) else {
                continue;
            };
            let r = area.rect;
            match side {
                Side::Right => lo = lo.max(r.left + size),
                Side::Left => hi = hi.min(r.right - size),
                Side::Bottom => lo = lo.max(r.top + size),
                Side::Top => hi = hi.min(r.bottom - size),
            }
        }
        (lo, hi)
    }

    /// Move an edge to `position`, clamped to its limits.
    pub fn move_edge(&self, edge: &EdgeSelection, position: f64, min: MinSize) -> Self {
        let (lo, hi) = self.edge_limits(edge, min);
        let mut next = self.clone();
        if lo > hi {
            return next;
        }
        let to = position.clamp(lo, hi);
        for EdgeSide { area, side } in &edge.sides {
            if let Some(area) = next.area_mut(*area) {
                area.rect.set(*side, to);
            }
        }
        next
    }
}

/// Where the shared edge's ends differ: `b`'s start and end minus `a`'s, along the edge.
fn offsets(a: &Rect, b: &Rect, side: Side) -> (f64, f64) {
    if side.horizontal() {
        (b.top - a.top, b.bottom - a.bottom)
    } else {
        (b.left - a.left, b.right - a.right)
    }
}

/// The side of `a` that `b` lies against, when they share an edge over at
/// least `tolerance` (or all of the shorter one's).
pub fn neighbour_side<E>(a: &Area<E>, b: &Area<E>, tolerance: MinSize) -> Option<Side> {
    if a.id == b.id {
        return None;
    }
    let (ra, rb) = (a.rect, b.rect);
    let overlap_x = ra.right.min(rb.right) - ra.left.max(rb.left);
    let overlap_y = ra.bottom.min(rb.bottom) - ra.top.max(rb.top);
    let min_x = tolerance.width.min(ra.width()).min(rb.width()) - EPSILON;
    let min_y = tolerance.height.min(ra.height()).min(rb.height()) - EPSILON;
    if same(ra.top, rb.bottom) && overlap_x >= min_x {
        Some(Side::Top)
    } else if same(ra.bottom, rb.top) && overlap_x >= min_x {
        Some(Side::Bottom)
    } else if same(ra.left, rb.right) && overlap_y >= min_y {
        Some(Side::Left)
    } else if same(ra.right, rb.left) && overlap_y >= min_y {
        Some(Side::Right)
    } else {
        None
    }
}
