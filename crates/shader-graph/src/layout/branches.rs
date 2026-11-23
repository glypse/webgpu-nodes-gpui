//! Splitting branching outputs at reroutes, so each branch leaves from a point of its own.

use std::collections::{HashMap, HashSet};

use crate::graph::{GraphEdge, GraphNode, Point, REROUTE};

/// Where a reroute goes, right of the output it splits.
const REROUTE_OFFSET: f32 = 240.0;

#[derive(Default)]
pub struct RerouteBranches {
    pub reroutes: Vec<GraphNode>,
    /// The links from each output to its reroute.
    pub links: Vec<GraphEdge>,
    /// The links moved to leave from a reroute (same ids, new source).
    pub moved: Vec<GraphEdge>,
}

/// One reroute after each output linked to several inputs, its links then
/// leaving from the reroute. Only outputs and inputs of nodes `among` takes
/// count; reroutes, which already are such points, and links in `skipped`
/// (broken ones) are left alone. `new_id` gives a fresh id for each node and
/// link added.
pub fn reroute_branches(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    among: impl Fn(&str) -> bool,
    mut new_id: impl FnMut() -> String,
    skipped: &HashSet<String>,
) -> RerouteBranches {
    let by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    // Outputs in the order of their first link, so the reroutes come out in document order.
    let mut groups: Vec<Vec<&GraphEdge>> = Vec::new();
    let mut slot_of: HashMap<(&str, &str), usize> = HashMap::new();
    for edge in edges {
        if by_id
            .get(edge.source.as_str())
            .is_some_and(|s| s.is(REROUTE))
            || skipped.contains(&edge.id)
            || !among(&edge.source)
            || !among(&edge.target)
        {
            continue;
        }
        let slot = *slot_of
            .entry((edge.source.as_str(), edge.source_handle.as_str()))
            .or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
        groups[slot].push(edge);
    }

    let mut out = RerouteBranches::default();
    for group in groups {
        let Some(&first) = group.first() else {
            continue;
        };
        if group.len() < 2 {
            continue;
        }
        let Some(source) = by_id.get(first.source.as_str()) else {
            continue;
        };
        let id = new_id();
        let mut reroute = GraphNode::new(
            REROUTE,
            Point::new(source.position.x + REROUTE_OFFSET, source.position.y),
        );
        reroute.id = id.clone();
        reroute.frame = source.frame.clone();
        out.reroutes.push(reroute);
        out.links.push(GraphEdge {
            id: new_id(),
            source: first.source.clone(),
            source_handle: first.source_handle.clone(),
            target: id.clone(),
            target_handle: "in".to_owned(),
        });
        for edge in group {
            out.moved.push(GraphEdge {
                source: id.clone(),
                source_handle: "out".to_owned(),
                ..(*edge).clone()
            });
        }
    }
    out
}
