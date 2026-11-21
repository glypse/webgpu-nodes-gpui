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
/// (broken ones) are left alone. `new_id` gives the id of what's added from
/// a key unique to it.
pub fn reroute_branches(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    among: impl Fn(&str) -> bool,
    mut new_id: impl FnMut(&str) -> String,
    skipped: &HashSet<String>,
) -> RerouteBranches {
    let by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut keys: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&GraphEdge>> = HashMap::new();
    for edge in edges {
        let Some(source) = by_id.get(edge.source.as_str()) else {
            continue;
        };
        if source.is(REROUTE) || skipped.contains(&edge.id) || !among(&edge.source) || !among(&edge.target) {
            continue;
        }
        let key = format!("{}\0{}", edge.source, edge.source_handle);
        let group = groups.entry(key.clone()).or_insert_with(|| {
            keys.push(key);
            Vec::new()
        });
        group.push(edge);
    }

    let mut out = RerouteBranches::default();
    for key in &keys {
        let Some(group) = groups.get(key) else { continue };
        let Some(first) = group.first().copied() else { continue };
        let Some(source) = by_id.get(first.source.as_str()) else {
            continue;
        };
        if group.len() < 2 {
            continue;
        }
        let id = new_id(&format!("reroute/{key}"));
        let mut reroute = GraphNode::new(
            REROUTE,
            Point::new(source.position.x + REROUTE_OFFSET, source.position.y),
        );
        reroute.id = id.clone();
        reroute.frame = source.frame.clone();
        out.reroutes.push(reroute);
        out.links.push(GraphEdge {
            id: new_id(&format!("link/{key}")),
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
