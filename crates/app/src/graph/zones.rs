//! Zones are drawn as rounded convex bubbles around their nodes: the convex
//! hull of the nodes' rectangles, grown outward by a padding.

use std::rc::Rc;

use gpui::{Hsla, WeakEntity, hsla, px};
use gpui_flow::{Flow, Id, Layer, LayerContext, Rect, Xy};
use shader_graph::zones::{Zone, ZoneKind};

/// Base distance between a zone's nodes and its bubble.
const ZONE_PADDING: f32 = 28.0;
/// Extra room per nested zone level, so an outer bubble wraps inner ones.
const ZONE_NESTING_STEP: f32 = 22.0;
const CORNER_RADIUS: f32 = 6.0;
const ARC_STEPS: usize = 6;

fn cross(o: Xy, a: Xy, b: Xy) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn chain(sorted: &[Xy]) -> Vec<Xy> {
    let mut out: Vec<Xy> = Vec::new();
    for &p in sorted {
        while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
            out.pop();
        }
        out.push(p);
    }
    out.pop();
    out
}

/// Convex hull (Andrew's monotone chain), without collinear points.
pub fn convex_hull(points: &[Xy]) -> Vec<Xy> {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    if sorted.len() <= 2 {
        return sorted;
    }
    let mut hull = chain(&sorted);
    sorted.reverse();
    hull.extend(chain(&sorted));
    hull
}

/// The bubble outline around `rects` and `points`, `padding` away, with
/// corners rounded by `corner` (at most `padding`): shapes are grown by
/// `padding - corner` with square corners, then the hull by `corner`.
pub fn bubble(rects: &[Rect], points: &[Xy], padding: f32, corner: f32) -> Vec<Xy> {
    let radius = corner.clamp(0.0, padding);
    let grow = padding - radius;
    let mut corners = Vec::new();
    let shapes = rects
        .iter()
        .copied()
        .chain(points.iter().map(|p| Rect::new(p.x, p.y, 0.0, 0.0)));
    for r in shapes {
        corners.extend([
            Xy::new(r.x - grow, r.y - grow),
            Xy::new(r.right() + grow, r.y - grow),
            Xy::new(r.right() + grow, r.bottom() + grow),
            Xy::new(r.x - grow, r.bottom() + grow),
        ]);
    }
    let hull = convex_hull(&corners);
    if radius <= 0.0 {
        return hull;
    }
    // Growing a convex polygon by a disc: arcs around every vertex.
    let mut out = Vec::new();
    for v in hull {
        for step in 0..ARC_STEPS * 4 {
            let a = step as f32 / (ARC_STEPS * 4) as f32 * std::f32::consts::TAU;
            out.push(Xy::new(v.x + radius * a.cos(), v.y + radius * a.sin()));
        }
    }
    convex_hull(&out)
}

/// The points making a zone's outline cut through the middle of its
/// boundary nodes.
pub fn boundary_points(input: Rect, output: Rect, padding: f32) -> Vec<Xy> {
    let in_center = input.x + input.width / 2.0;
    let out_center = output.x + output.width / 2.0;
    let inward = if out_center > in_center {
        1.0
    } else if out_center < in_center {
        -1.0
    } else {
        1.0
    };
    let segment = |r: Rect, x: f32| [Xy::new(x, r.y), Xy::new(x, r.bottom())];
    let mut points = Vec::new();
    points.extend(segment(input, in_center + inward * padding));
    points.extend(segment(output, out_center - inward * padding));
    points
}

/// The distance between each zone's nodes and its bubble: more around zones containing zones.
pub fn paddings(zones: &[&Zone]) -> Vec<(String, f32)> {
    let mut height: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut deepest: Vec<&&Zone> = zones.iter().collect();
    deepest.sort_by_key(|z| std::cmp::Reverse(z.depth));
    for zone in deepest {
        height.entry(zone.id.as_str()).or_insert(0);
        if let Some(parent) = &zone.parent {
            let child = height[zone.id.as_str()] + 1;
            let entry = height.entry(parent.as_str()).or_insert(0);
            *entry = (*entry).max(child);
        }
    }
    zones
        .iter()
        .map(|z| {
            (
                z.id.clone(),
                ZONE_PADDING + ZONE_NESTING_STEP * *height.get(z.id.as_str()).unwrap_or(&0) as f32,
            )
        })
        .collect()
}

pub struct Bubble {
    pub kind: ZoneKind,
    pub input: Id,
    pub output: Id,
    pub members: Vec<Id>,
    pub padding: f32,
}

fn color(kind: ZoneKind) -> Hsla {
    match kind {
        ZoneKind::Repeat => hsla(0.0, 0.0, 0.62, 1.0),
        ZoneKind::Simulation => hsla(0.03, 0.55, 0.62, 1.0),
        ZoneKind::Passes => hsla(0.58, 0.45, 0.62, 1.0),
    }
}

/// A back layer painting the bubbles, outermost first.
pub fn layer(flow: WeakEntity<Flow>, bubbles: Vec<Bubble>) -> Layer {
    Rc::new(move |ctx: &LayerContext, window, cx| {
        guard::guard("zones layer", || {
            let Some(flow) = flow.upgrade() else { return };
            let flow = flow.read(cx);
            for b in &bubbles {
                let (Some(input), Some(output)) =
                    (flow.node_rect(&b.input), flow.node_rect(&b.output))
                else {
                    continue;
                };
                let rects: Vec<Rect> = b.members.iter().filter_map(|m| flow.node_rect(m)).collect();
                let points = boundary_points(input, output, b.padding);
                let outline = bubble(&rects, &points, b.padding, CORNER_RADIUS);
                if outline.len() < 3 {
                    continue;
                }
                let path: Vec<_> = outline.iter().map(|p| ctx.to_window(*p)).collect();
                let base = color(b.kind);
                let mut fill = gpui::PathBuilder::fill();
                fill.add_polygon(&path, true);
                if let Ok(p) = fill.build() {
                    window.paint_path(p, base.opacity(0.16));
                }
                let mut stroke = gpui::PathBuilder::stroke(px(1.5));
                stroke.add_polygon(&path, true);
                if let Ok(p) = stroke.build() {
                    window.paint_path(p, base.opacity(0.55));
                }
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_drops_interior_points() {
        let hull = convex_hull(&[
            Xy::new(0.0, 0.0),
            Xy::new(10.0, 0.0),
            Xy::new(10.0, 10.0),
            Xy::new(0.0, 10.0),
            Xy::new(5.0, 5.0),
        ]);
        assert_eq!(hull.len(), 4);
    }

    #[test]
    fn bubble_encloses_its_rects_with_padding() {
        let outline = bubble(&[Rect::new(0.0, 0.0, 100.0, 50.0)], &[], 28.0, 6.0);
        let min_x = outline.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_y = outline
            .iter()
            .map(|p| p.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x + 28.0).abs() < 0.5);
        assert!((max_y - 78.0).abs() < 0.5);
    }

    #[test]
    fn boundary_points_sit_inward_of_each_node() {
        let a = Rect::new(0.0, 0.0, 100.0, 50.0);
        let b = Rect::new(500.0, 0.0, 100.0, 50.0);
        let points = boundary_points(a, b, 28.0);
        assert_eq!(points[0].x, 78.0);
        assert_eq!(points[2].x, 522.0);
    }

    #[test]
    fn nested_zones_get_more_room() {
        let outer = Zone {
            id: "o".into(),
            kind: ZoneKind::Repeat,
            input_id: "i".into(),
            output_id: "x".into(),
            parent: None,
            depth: 0,
            members: Default::default(),
        };
        let inner = Zone {
            id: "n".into(),
            parent: Some("o".into()),
            depth: 1,
            ..outer.clone()
        };
        let padded = paddings(&[&outer, &inner]);
        let pad = |id: &str| padded.iter().find(|(i, _)| i == id).unwrap().1;
        assert!(pad("o") > pad("n"));
    }
}
