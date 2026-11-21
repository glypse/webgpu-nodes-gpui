use std::collections::{HashMap, HashSet};

use thiserror::Error;

use super::{GraphDocument, GraphProject, ROOT_ID};
use crate::nodes::is_known_node_type;
use crate::zones::{is_zone_output, paired_output_id};

/// What a file may ask for. Far above anything drawn by hand, yet low enough
/// that the work a file can cause stays bounded.
pub const MAX_DEFINITIONS: usize = 2000;
pub const MAX_NODES_PER_DOCUMENT: usize = 20_000;
pub const MAX_EDGES_PER_DOCUMENT: usize = 80_000;
pub const MAX_PROJECT_NODES: usize = 100_000;
/// Links in a row, which type inference and codegen follow recursively.
pub const MAX_CHAIN_LENGTH: usize = 2000;
/// Groups nested in a row, for the same reason.
pub const MAX_GROUP_DEPTH: usize = 64;
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;

/// A rule of the graph model a file breaks, though its shape is right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphIssue {
    NoRoot,
    DuplicateId { definition: String, id: String },
    DanglingEdge { definition: String, edge: String },
    EdgeCycle { definition: String },
    ChainTooLong { definition: String },
    GroupCycle,
    GroupsTooDeep,
    RootInstance { definition: String },
    UnknownNodeType { definition: String, node: String, node_type: String },
    MissingDefinition { definition: String, node: String, reference: String },
    TooManyDefinitions,
    TooManyNodes,
    TooManyInDocument { definition: String },
}

/// Why a file can't be read.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LoadError {
    #[error("It's too large: its JSON.")]
    TooLarge,
    #[error("Its JSON can't be read.")]
    InvalidJson,
    #[error("Its contents aren't what this editor saves: {0}")]
    InvalidSchema(String),
    #[error("Its graphs break the editor's rules, so it may be damaged or crafted: {0:?}")]
    InvalidGraph(Vec<GraphIssue>),
}

/// The longest path through a directed graph, in links; `None` when it has a
/// cycle. Iterative: what it checks is what would overflow the stack.
fn longest_path(ids: &[&str], links: &[(&str, &str)]) -> Option<usize> {
    let mut incoming: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in links {
        if !incoming.contains_key(from) || !incoming.contains_key(to) {
            continue;
        }
        next.entry(from).or_default().push(to);
        *incoming.get_mut(to).expect("checked") += 1;
    }
    let mut lengths: HashMap<&str, usize> = HashMap::new();
    let mut ready: Vec<&str> = incoming.iter().filter(|(_, c)| **c == 0).map(|(id, _)| *id).collect();
    let (mut visited, mut longest) = (0, 0);
    while let Some(id) = ready.pop() {
        visited += 1;
        let length = lengths.get(id).copied().unwrap_or(0);
        longest = longest.max(length);
        for target in next.get(id).into_iter().flatten() {
            let l = lengths.entry(target).or_insert(0);
            *l = (*l).max(length + 1);
            let left = incoming.get_mut(target).expect("known");
            *left -= 1;
            if *left == 0 {
                ready.push(target);
            }
        }
    }
    (visited == incoming.len()).then_some(longest)
}

fn duplicates<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.filter(|id| !seen.insert(*id)).map(str::to_owned).collect()
}

/// What's wrong within one document, `definition` naming it.
pub fn document_issues(definition: &str, doc: &GraphDocument) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    let lists: Vec<Vec<&str>> = [
        doc.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        doc.edges.iter().map(|e| e.id.as_str()).collect(),
        doc.type_variables.iter().map(|v| v.id.as_str()).collect(),
        doc.inputs.iter().map(|h| h.id.as_str()).collect(),
        doc.outputs.iter().map(|h| h.id.as_str()).collect(),
    ]
    .into_iter()
    .chain(doc.nodes.iter().flat_map(|n| {
        [
            n.data.items.iter().flatten().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            n.data.sockets.iter().flat_map(|s| s.inputs.iter().map(|h| h.id.as_str())).collect(),
            n.data.sockets.iter().flat_map(|s| s.outputs.iter().map(|h| h.id.as_str())).collect(),
        ]
    }))
    .collect();
    for ids in lists {
        for id in duplicates(ids.into_iter()) {
            issues.push(GraphIssue::DuplicateId { definition: definition.to_owned(), id });
        }
    }
    if doc.nodes.len() > MAX_NODES_PER_DOCUMENT || doc.edges.len() > MAX_EDGES_PER_DOCUMENT {
        issues.push(GraphIssue::TooManyInDocument { definition: definition.to_owned() });
    }
    let nodes: HashSet<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &doc.edges {
        if !nodes.contains(edge.source.as_str()) || !nodes.contains(edge.target.as_str()) {
            issues.push(GraphIssue::DanglingEdge { definition: definition.to_owned(), edge: edge.id.clone() });
        }
    }
    // A zone's output depends on its input without a link saying so.
    let outputs: HashSet<&str> =
        doc.nodes.iter().filter(|n| is_zone_output(&n.node_type)).map(|n| n.id.as_str()).collect();
    let mut links: Vec<(&str, &str)> =
        doc.edges.iter().map(|e| (e.source.as_str(), e.target.as_str())).collect();
    for n in &doc.nodes {
        if let Some(output) = paired_output_id(n).filter(|o| outputs.contains(o)) {
            links.push((&n.id, output));
        }
    }
    let ids: Vec<&str> = nodes.iter().copied().collect();
    match longest_path(&ids, &links) {
        None => issues.push(GraphIssue::EdgeCycle { definition: definition.to_owned() }),
        Some(l) if l > MAX_CHAIN_LENGTH => {
            issues.push(GraphIssue::ChainTooLong { definition: definition.to_owned() });
        }
        _ => {}
    }
    issues
}

/// Every rule a saved project keeps.
pub fn project_issues(project: &GraphProject) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    if !project.definitions.contains_key(ROOT_ID) {
        issues.push(GraphIssue::NoRoot);
    }
    if project.definitions.len() > MAX_DEFINITIONS {
        issues.push(GraphIssue::TooManyDefinitions);
    }
    if project.definitions.values().map(|d| d.nodes.len()).sum::<usize>() > MAX_PROJECT_NODES {
        issues.push(GraphIssue::TooManyNodes);
    }
    for (key, doc) in &project.definitions {
        issues.extend(document_issues(key, doc));
        for node in &doc.nodes {
            if !is_known_node_type(&node.node_type) {
                issues.push(GraphIssue::UnknownNodeType {
                    definition: key.clone(),
                    node: node.id.clone(),
                    node_type: node.node_type.clone(),
                });
            }
            if let Some(reference) = node.definition_ref() {
                if !project.definitions.contains_key(reference) {
                    issues.push(GraphIssue::MissingDefinition {
                        definition: key.clone(),
                        node: node.id.clone(),
                        reference: reference.to_owned(),
                    });
                }
                if reference == ROOT_ID {
                    issues.push(GraphIssue::RootInstance { definition: key.clone() });
                }
            }
        }
    }
    let ids: Vec<&str> = project.definitions.keys().map(String::as_str).collect();
    let links: Vec<(&str, &str)> = project
        .definitions
        .iter()
        .flat_map(|(key, doc)| doc.group_refs().map(move |r| (key.as_str(), r)))
        .collect();
    match longest_path(&ids, &links) {
        None => issues.push(GraphIssue::GroupCycle),
        Some(d) if d > MAX_GROUP_DEPTH => issues.push(GraphIssue::GroupsTooDeep),
        _ => {}
    }
    issues
}

impl GraphProject {
    /// Read a saved project, checking it keeps every rule the editor relies on.
    pub fn from_json(text: &str) -> Result<GraphProject, LoadError> {
        if text.len() > MAX_JSON_BYTES {
            return Err(LoadError::TooLarge);
        }
        let value: serde_json::Value = serde_json::from_str(text).map_err(|_| LoadError::InvalidJson)?;
        let mut project: GraphProject =
            serde_json::from_value(value).map_err(|e| LoadError::InvalidSchema(e.to_string()))?;
        crate::library::register(&mut project);
        project.reindex();
        let issues = project_issues(&project);
        if issues.is_empty() { Ok(project) } else { Err(LoadError::InvalidGraph(issues)) }
    }

    /// The project as saved: pretty JSON of the original's format.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a project serialises")
    }
}
