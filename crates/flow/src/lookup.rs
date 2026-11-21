//! Resolved node geometry and the connection logic built on it: handle
//! positions, edge ends, the closest handle to the pointer, and validity.
//! Ports of `@xyflow/system`'s `getHandlePosition`, `getEdgePosition`,
//! `getClosestHandle` and `isValidHandle`.

use std::collections::HashMap;

use crate::edge_path::Position;
use crate::geometry::{Rect, Xy};
use crate::model::{Connection, ConnectionMode, HandleBounds, HandleType, Id};

#[derive(Clone, Debug)]
pub struct InternalNode {
    pub id: Id,
    /// Top-left corner in flow units, origin applied.
    pub position: Xy,
    /// Measured or fixed; `None` before the node has been laid out.
    pub size: Option<(f32, f32)>,
    pub handles: Vec<HandleBounds>,
    pub connectable: bool,
    pub z: i32,
}

impl InternalNode {
    pub fn rect(&self) -> Rect {
        let (w, h) = self.size.unwrap_or((0.0, 0.0));
        Rect::new(self.position.x, self.position.y, w, h)
    }

    pub fn initialized(&self) -> bool {
        self.size.is_some() && !self.handles.is_empty()
    }
}

pub type Lookup = HashMap<Id, InternalNode>;

/// Where an edge meets a handle (the handle's edge facing outwards), or its
/// centre when `center`.
pub fn handle_position(node: &InternalNode, handle: Option<&HandleBounds>, center: bool) -> Xy {
    let (hx, hy, w, h, pos) = match handle {
        Some(h) => (h.x, h.y, h.width, h.height, h.position),
        None => {
            let (w, h) = node.size.unwrap_or((0.0, 0.0));
            (0.0, 0.0, w, h, Position::Left)
        }
    };
    let x = hx + node.position.x;
    let y = hy + node.position.y;
    if center {
        return Xy::new(x + w / 2.0, y + h / 2.0);
    }
    match pos {
        Position::Top => Xy::new(x + w / 2.0, y),
        Position::Right => Xy::new(x + w, y + h / 2.0),
        Position::Bottom => Xy::new(x + w / 2.0, y + h),
        Position::Left => Xy::new(x, y + h / 2.0),
    }
}

pub fn find_handle<'a>(
    node: &'a InternalNode,
    kind: HandleType,
    id: Option<&Id>,
    mode: ConnectionMode,
) -> Option<&'a HandleBounds> {
    let mut candidates = node
        .handles
        .iter()
        .filter(|h| mode == ConnectionMode::Loose || h.kind == kind);
    match id {
        Some(id) => candidates.find(|h| h.id.as_ref() == Some(id)),
        None => candidates.next(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeEnds {
    pub source: Xy,
    pub source_position: Position,
    pub target: Xy,
    pub target_position: Position,
}

/// `None` until both nodes are measured and both handles exist.
pub fn edge_ends(
    source: &InternalNode,
    source_handle: Option<&Id>,
    target: &InternalNode,
    target_handle: Option<&Id>,
    mode: ConnectionMode,
) -> Option<EdgeEnds> {
    if !source.initialized() || !target.initialized() {
        return None;
    }
    let s = find_handle(source, HandleType::Source, source_handle, ConnectionMode::Strict)?;
    let t = find_handle(target, HandleType::Target, target_handle, mode)
        .or_else(|| {
            (mode == ConnectionMode::Loose)
                .then(|| find_handle(target, HandleType::Source, target_handle, mode))
                .flatten()
        })?;
    Some(EdgeEnds {
        source: handle_position(source, Some(s), false),
        source_position: s.position,
        target: handle_position(target, Some(t), false),
        target_position: t.position,
    })
}

/// A handle within reach of the pointer, with its centre.
#[derive(Clone, Debug, PartialEq)]
pub struct Near {
    pub node: Id,
    pub handle: HandleBounds,
    pub center: Xy,
}

const ADDITIONAL_DISTANCE: f32 = 250.0;

/// The handle closest to `at` within `radius`, skipping `from`; on a tie the
/// one of the opposite type wins.
pub fn closest_handle(
    at: Xy,
    radius: f32,
    lookup: &Lookup,
    from: (&Id, HandleType, Option<&Id>),
) -> Option<Near> {
    let reach = radius + ADDITIONAL_DISTANCE;
    let area = Rect::new(at.x - reach, at.y - reach, reach * 2.0, reach * 2.0);
    let mut best: Vec<Near> = Vec::new();
    let mut min = f32::INFINITY;
    for node in lookup.values() {
        if node.rect().overlap_area(&area) <= 0.0 {
            continue;
        }
        for handle in &node.handles {
            if *from.0 == node.id && from.1 == handle.kind && from.2 == handle.id.as_ref() {
                continue;
            }
            let center = handle_position(node, Some(handle), true);
            let d = center.distance(at);
            if d > radius {
                continue;
            }
            let near = Near { node: node.id.clone(), handle: handle.clone(), center };
            if d < min {
                min = d;
                best = vec![near];
            } else if d == min {
                best.push(near);
            }
        }
    }
    if best.len() > 1 {
        let wanted = from.1.opposite();
        if let Some(i) = best.iter().position(|n| n.handle.kind == wanted) {
            return Some(best.swap_remove(i));
        }
    }
    best.into_iter().next()
}

/// The handle whose box contains `at`, preferred over a merely close one.
pub fn handle_under(at: Xy, lookup: &Lookup, from: (&Id, HandleType, Option<&Id>)) -> Option<Near> {
    lookup.values().find_map(|node| {
        node.handles.iter().find_map(|h| {
            if *from.0 == node.id && from.1 == h.kind && from.2 == h.id.as_ref() {
                return None;
            }
            let rect = Rect::new(node.position.x + h.x, node.position.y + h.y, h.width, h.height);
            rect.contains(at).then(|| Near {
                node: node.id.clone(),
                handle: h.clone(),
                center: rect.center(),
            })
        })
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Validated {
    pub connection: Connection,
    pub valid: bool,
}

/// Builds the connection that dropping a connection started at `from` on
/// `to` would make, and whether it's allowed.
pub fn validate(
    from: (&Id, HandleType, Option<&Id>),
    to: &Near,
    mode: ConnectionMode,
    is_valid: Option<&dyn Fn(&Connection) -> bool>,
) -> Validated {
    let from_is_target = from.1 == HandleType::Target;
    let connection = if from_is_target {
        Connection {
            source: to.node.clone(),
            source_handle: to.handle.id.clone(),
            target: from.0.clone(),
            target_handle: from.2.cloned(),
        }
    } else {
        Connection {
            source: from.0.clone(),
            source_handle: from.2.cloned(),
            target: to.node.clone(),
            target_handle: to.handle.id.clone(),
        }
    };
    let connectable = to.handle.connectable && to.handle.connectable_end;
    let allowed = connectable
        && match mode {
            ConnectionMode::Strict => {
                (from_is_target && to.handle.kind == HandleType::Source)
                    || (!from_is_target && to.handle.kind == HandleType::Target)
            }
            ConnectionMode::Loose => *from.0 != to.node || from.2 != to.handle.id.as_ref(),
        };
    let valid = allowed && is_valid.is_none_or(|f| f(&connection));
    Validated { connection, valid }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(id: &str, kind: HandleType, position: Position, x: f32, y: f32) -> HandleBounds {
        HandleBounds {
            id: Some(id.to_string().into()),
            kind,
            position,
            x,
            y,
            width: 8.0,
            height: 8.0,
            connectable: true,
            connectable_start: true,
            connectable_end: true,
        }
    }

    fn node(id: &str, x: f32, y: f32, handles: Vec<HandleBounds>) -> InternalNode {
        InternalNode {
            id: id.to_string().into(),
            position: Xy::new(x, y),
            size: Some((100.0, 50.0)),
            handles,
            connectable: true,
            z: 0,
        }
    }

    fn lookup() -> Lookup {
        let a = node("a", 0.0, 0.0, vec![handle("out", HandleType::Source, Position::Right, 96.0, 21.0)]);
        let b = node("b", 300.0, 100.0, vec![
            handle("in", HandleType::Target, Position::Left, -4.0, 21.0),
            handle("out", HandleType::Source, Position::Right, 96.0, 21.0),
        ]);
        [a, b].into_iter().map(|n| (n.id.clone(), n)).collect()
    }

    #[test]
    fn edge_ends_sit_on_the_handle_edge() {
        let l = lookup();
        let ends = edge_ends(&l["a"], Some(&"out".into()), &l["b"], Some(&"in".into()), ConnectionMode::Strict)
            .unwrap();
        assert_eq!(ends.source, Xy::new(104.0, 25.0));
        assert_eq!(ends.target, Xy::new(296.0, 125.0));
        assert_eq!(ends.source_position, Position::Right);
        assert_eq!(ends.target_position, Position::Left);
    }

    #[test]
    fn edge_ends_wait_for_measurement() {
        let mut l = lookup();
        l.get_mut("a").unwrap().size = None;
        assert!(edge_ends(&l["a"], None, &l["b"], None, ConnectionMode::Strict).is_none());
    }

    #[test]
    fn closest_handle_respects_radius_and_skips_origin() {
        let l = lookup();
        let a: Id = "a".into();
        let out: Id = "out".into();
        let near = closest_handle(Xy::new(298.0, 126.0), 20.0, &l, (&a, HandleType::Source, Some(&out)));
        assert_eq!(near.unwrap().handle.id.as_ref().map(|s| s.as_str()), Some("in"));
        assert!(closest_handle(Xy::new(200.0, 400.0), 20.0, &l, (&a, HandleType::Source, Some(&out))).is_none());
        // The handle being dragged from isn't a candidate for itself.
        assert!(closest_handle(Xy::new(100.0, 25.0), 20.0, &l, (&a, HandleType::Source, Some(&out))).is_none());
    }

    #[test]
    fn strict_mode_pairs_source_with_target() {
        let l = lookup();
        let a: Id = "a".into();
        let out: Id = "out".into();
        let from = (&a, HandleType::Source, Some(&out));
        let to_target = Near {
            node: "b".into(),
            handle: l["b"].handles[0].clone(),
            center: Xy::ZERO,
        };
        let v = validate(from, &to_target, ConnectionMode::Strict, None);
        assert!(v.valid);
        assert_eq!(v.connection.target.as_ref(), "b");
        let to_source = Near { node: "b".into(), handle: l["b"].handles[1].clone(), center: Xy::ZERO };
        assert!(!validate(from, &to_source, ConnectionMode::Strict, None).valid);
        assert!(validate(from, &to_source, ConnectionMode::Loose, None).valid);
    }

    #[test]
    fn dragging_from_a_target_flips_the_connection() {
        let l = lookup();
        let b: Id = "b".into();
        let input: Id = "in".into();
        let to = Near { node: "a".into(), handle: l["a"].handles[0].clone(), center: Xy::ZERO };
        let v = validate((&b, HandleType::Target, Some(&input)), &to, ConnectionMode::Strict, None);
        assert!(v.valid);
        assert_eq!(v.connection.source.as_ref(), "a");
        assert_eq!(v.connection.target.as_ref(), "b");
    }

    #[test]
    fn caller_validation_vetoes() {
        let l = lookup();
        let a: Id = "a".into();
        let out: Id = "out".into();
        let to = Near { node: "b".into(), handle: l["b"].handles[0].clone(), center: Xy::ZERO };
        let veto = |_: &Connection| false;
        assert!(!validate((&a, HandleType::Source, Some(&out)), &to, ConnectionMode::Strict, Some(&veto)).valid);
    }

    #[test]
    fn non_connectable_handles_refuse() {
        let l = lookup();
        let a: Id = "a".into();
        let out: Id = "out".into();
        let mut h = l["b"].handles[0].clone();
        h.connectable_end = false;
        let to = Near { node: "b".into(), handle: h, center: Xy::ZERO };
        assert!(!validate((&a, HandleType::Source, Some(&out)), &to, ConnectionMode::Strict, None).valid);
    }

    #[test]
    fn handle_under_prefers_the_box_containing_the_pointer() {
        let l = lookup();
        let a: Id = "a".into();
        let out: Id = "out".into();
        let hit = handle_under(Xy::new(297.0, 126.0), &l, (&a, HandleType::Source, Some(&out)));
        assert_eq!(hit.unwrap().handle.id.as_ref().map(|s| s.as_str()), Some("in"));
    }
}
