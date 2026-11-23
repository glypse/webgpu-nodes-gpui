use std::collections::{HashMap, HashSet};

use thiserror::Error;

use super::hydrate::normalise_project;
use super::limits::*;
use super::{GraphDocument, GraphNode, GraphProject, ROOT_ID};
use crate::library::is_library_ref;
use crate::nodes::{has_valid_socket_names, is_known_node_type};
use crate::types::{EnumVariant, Handle, HandleType, NodeData, TypeOwner, TypeVariable};
use crate::zones::{is_zone_output, paired_output_id};

/// A rule of the graph model a file breaks, though its shape is right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphIssue {
    NoRoot,
    DuplicateId {
        definition: String,
        id: String,
    },
    DuplicateAsset {
        id: String,
    },
    NodeIdInTwoDefinitions {
        id: String,
        definition: String,
    },
    DanglingEdge {
        definition: String,
        edge: String,
    },
    EdgeCycle {
        definition: String,
    },
    ChainTooLong {
        definition: String,
    },
    GroupCycle,
    GroupsTooDeep,
    RootInstance {
        definition: String,
    },
    ReservedDefinition {
        definition: String,
    },
    UnknownNodeType {
        definition: String,
        node: String,
        node_type: String,
    },
    MissingDefinition {
        definition: String,
        node: String,
        reference: String,
    },
    TooManyDefinitions,
    TooManyNodes,
    InvalidSocketNames {
        definition: String,
        node: String,
    },
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

/// Collects the schema's violations, each prefixed with the path of its value.
#[derive(Default)]
struct Schema(Vec<String>);

impl Schema {
    fn fail(&mut self, path: &str, message: impl std::fmt::Display) {
        self.0.push(format!("{path}: {message}"));
    }

    fn length(&mut self, path: &str, len: usize, max: usize) {
        if len > max {
            self.fail(path, format!("too many: {len}, at most {max}"));
        }
    }

    fn text(&mut self, path: &str, text: &str, max: usize) {
        if text.len() > max {
            self.fail(path, format!("too long: {} bytes, at most {max}", text.len()));
        }
    }

    fn id(&mut self, path: &str, id: &str) {
        self.text(path, id, MAX_ID_LENGTH);
    }

    fn label(&mut self, path: &str, label: &str) {
        self.text(path, label, MAX_LABEL_LENGTH);
    }

    fn list<T>(&mut self, path: &str, items: &[T]) {
        self.length(path, items.len(), MAX_LIST_LENGTH);
    }

    fn handle<T>(&mut self, path: &str, handle: &Handle<T>) {
        self.id(&format!("{path}.id"), &handle.id);
        self.label(&format!("{path}.label"), &handle.label);
        if let Some(variants) = &handle.variants {
            self.list(&format!("{path}.variants"), variants);
            for (i, variant) in variants.iter().enumerate() {
                self.variant(&format!("{path}.variants[{i}]"), variant);
                if variants[..i].iter().any(|earlier| earlier.value == variant.value) {
                    self.fail(&format!("{path}.variants[{i}].value"), "duplicates an earlier variant's value");
                }
            }
        }
        if let Some(owner) = &handle.type_owner {
            self.type_owner(&format!("{path}.typeOwner"), owner);
        }
        if let Some(id) = &handle.variants_by {
            self.id(&format!("{path}.variantsBy"), id);
        }
        if let Some(owner) = &handle.owner {
            self.id(&format!("{path}.owner.enumId"), &owner.enum_id);
        }
    }

    /// A group or zone socket, whose type may name one of the group's type variables.
    fn group_handle(&mut self, path: &str, handle: &Handle<HandleType>) {
        self.handle(path, handle);
        if let HandleType::Variable { variable } = &handle.ty {
            self.id(&format!("{path}.type.variable"), variable);
        }
    }

    fn variant(&mut self, path: &str, variant: &EnumVariant) {
        if u32::try_from(variant.value).is_err() {
            self.fail(&format!("{path}.value"), "must fit an unsigned 32-bit integer");
        }
        self.label(&format!("{path}.label"), &variant.label);
        if let Some(types) = &variant.types {
            self.list(&format!("{path}.types"), types);
        }
    }

    fn type_owner(&mut self, path: &str, owner: &TypeOwner) {
        self.id(&format!("{path}.variable"), &owner.variable);
        self.list(&format!("{path}.types"), &owner.types);
    }

    fn type_variable(&mut self, path: &str, variable: &TypeVariable) {
        self.id(&format!("{path}.id"), &variable.id);
        self.label(&format!("{path}.name"), &variable.name);
        self.list(&format!("{path}.types"), &variable.types);
    }

    fn node_data(&mut self, path: &str, data: &NodeData) {
        if let Some(label) = &data.label {
            self.label(&format!("{path}.label"), label);
        }
        for key in data.values.keys().chain(data.types.keys()) {
            self.id(&format!("{path}.values"), key);
        }
        self.length(&format!("{path}.values"), data.values.len(), MAX_LIST_LENGTH);
        self.length(&format!("{path}.types"), data.types.len(), MAX_LIST_LENGTH);
        for (key, value) in &data.settings {
            self.id(&format!("{path}.settings"), key);
            self.label(&format!("{path}.settings.{key}"), value);
        }
        self.length(&format!("{path}.settings"), data.settings.len(), MAX_LIST_LENGTH);
        self.length(&format!("{path}.extra"), data.extra.len(), MAX_LIST_LENGTH);
        if let Some(items) = &data.items {
            self.list(&format!("{path}.items"), items);
            for (i, item) in items.iter().enumerate() {
                self.group_handle(&format!("{path}.items[{i}]"), item);
            }
        }
        if let Some(id) = &data.paired_output {
            self.id(&format!("{path}.pairedOutput"), id);
        }
        if let Some(max) = data.max_iterations.filter(|m| *m > MAX_ITERATIONS) {
            self.fail(
                &format!("{path}.maxIterations"),
                format!("too large: {max}, at most {MAX_ITERATIONS}"),
            );
        }
        if let Some(cases) = data.cases.filter(|c| !(1..=MAX_CASES).contains(c)) {
            self.fail(
                &format!("{path}.cases"),
                format!("out of range: {cases}, must be 1 to {MAX_CASES}"),
            );
        }
        if let Some(code) = &data.code {
            self.text(&format!("{path}.code"), code, MAX_CODE_LENGTH);
        }
        if let Some(sockets) = &data.sockets {
            for (side, list) in [("inputs", &sockets.inputs), ("outputs", &sockets.outputs)] {
                self.list(&format!("{path}.sockets.{side}"), list);
                for (i, socket) in list.iter().enumerate() {
                    self.handle(&format!("{path}.sockets.{side}[{i}]"), socket);
                }
            }
        }
        if let Some(curve) = &data.curve {
            self.list(&format!("{path}.curve"), curve);
        }
        if let Some(ramp) = &data.ramp {
            self.list(&format!("{path}.ramp"), ramp);
            let mut seen = HashSet::new();
            for stop in ramp {
                self.id(&format!("{path}.ramp.id"), &stop.id);
                if !seen.insert(stop.id.as_str()) {
                    self.fail(
                        &format!("{path}.ramp"),
                        "Every ramp stop needs an id of its own.",
                    );
                }
            }
        }
        if let Some(asset) = &data.asset {
            self.id(&format!("{path}.asset"), asset);
        }
        if let Some(note) = data.extra.get("note").and_then(|n| n.as_str()) {
            self.text(&format!("{path}.note"), note, MAX_NOTE_LENGTH);
        }
    }

    fn node(&mut self, path: &str, node: &GraphNode) {
        self.id(&format!("{path}.id"), &node.id);
        self.node_data(&format!("{path}.data"), &node.data);
        if let Some(group) = &node.group {
            self.id(
                &format!("{path}.group.definitionRef"),
                &group.definition_ref,
            );
        }
        if let Some(frame) = &node.frame {
            self.id(&format!("{path}.frame"), frame);
        }
    }

    fn document(&mut self, path: &str, doc: &GraphDocument) {
        self.length(
            &format!("{path}.nodes"),
            doc.nodes.len(),
            MAX_NODES_PER_DOCUMENT,
        );
        self.length(&format!("{path}.defaults"), doc.defaults.len(), MAX_LIST_LENGTH);
        self.length(
            &format!("{path}.edges"),
            doc.edges.len(),
            MAX_EDGES_PER_DOCUMENT,
        );
        for (i, node) in doc.nodes.iter().enumerate() {
            self.node(&format!("{path}.nodes[{i}]"), node);
        }
        for (i, edge) in doc.edges.iter().enumerate() {
            let edge_path = format!("{path}.edges[{i}]");
            self.id(&format!("{edge_path}.id"), &edge.id);
            self.id(&format!("{edge_path}.source"), &edge.source);
            self.id(&format!("{edge_path}.target"), &edge.target);
            self.id(&format!("{edge_path}.sourceHandle"), &edge.source_handle);
            self.id(&format!("{edge_path}.targetHandle"), &edge.target_handle);
        }
        if let Some(label) = &doc.label {
            self.label(&format!("{path}.label"), label);
        }
        self.list(&format!("{path}.typeVariables"), &doc.type_variables);
        for (i, variable) in doc.type_variables.iter().enumerate() {
            self.type_variable(&format!("{path}.typeVariables[{i}]"), variable);
        }
        for (side, list) in [("inputs", &doc.inputs), ("outputs", &doc.outputs)] {
            self.list(&format!("{path}.{side}"), list);
            for (i, handle) in list.iter().enumerate() {
                self.group_handle(&format!("{path}.{side}[{i}]"), handle);
            }
        }
        for key in doc.defaults.keys() {
            self.id(&format!("{path}.defaults"), key);
        }
        if let Some(viewport) = doc
            .viewport
            .filter(|v| v.zoom.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater))
        {
            self.fail(
                &format!("{path}.viewport.zoom"),
                format!("zoom must be above 0, got {}", viewport.zoom),
            );
        }
    }
}

/// The schema's limits on a parsed project, each violation with the path of its value.
/// Serde has already checked the shape; this checks the sizes, counts and ranges the
/// schema's caps.
pub fn schema_issues(project: &GraphProject) -> Vec<String> {
    let mut schema = Schema::default();
    schema.length("definitions", project.definitions.len(), MAX_DEFINITIONS);
    // Counted before the library's definitions are added, so only what the file holds is judged.
    let nodes: usize = project.definitions.values().map(|d| d.nodes.len()).sum();
    if nodes > MAX_PROJECT_NODES {
        schema.fail("definitions", format!("too many nodes: {nodes}, at most {MAX_PROJECT_NODES}"));
    }
    for (key, doc) in &project.definitions {
        schema.id("definitions", key);
        schema.document(&format!("definitions.{key}"), doc);
    }
    schema.length("extra", project.extra.len(), MAX_LIST_LENGTH);
    schema.length("assets", project.assets.len(), MAX_ASSETS);
    for asset in &project.assets {
        schema.id("assets.id", asset.id());
        if !asset.is_valid() {
            schema.fail("assets", format!("\"{}\" isn't a valid asset", asset.name()));
        }
    }
    schema.0
}

/// The longest path through a directed graph, in links; `None` when it has a
/// cycle. Iterative: what it checks is what would overflow the stack.
pub(super) fn longest_path(ids: &[&str], links: &[(&str, &str)]) -> Option<usize> {
    let mut incoming: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in links {
        if !incoming.contains_key(from) {
            continue;
        }
        let Some(count) = incoming.get_mut(to) else {
            continue;
        };
        *count += 1;
        next.entry(from).or_default().push(to);
    }
    let mut lengths: HashMap<&str, usize> = HashMap::new();
    let mut ready: Vec<&str> = incoming
        .iter()
        .filter(|(_, c)| **c == 0)
        .map(|(id, _)| *id)
        .collect();
    let (mut visited, mut longest) = (0, 0);
    while let Some(id) = ready.pop() {
        visited += 1;
        let length = lengths.get(id).copied().unwrap_or(0);
        longest = longest.max(length);
        for target in next.get(id).into_iter().flatten() {
            let l = lengths.entry(target).or_insert(0);
            *l = (*l).max(length + 1);
            let Some(left) = incoming.get_mut(target) else {
                continue;
            };
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
    ids.filter(|id| !seen.insert(*id))
        .map(str::to_owned)
        .collect()
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
            n.data
                .items
                .iter()
                .flatten()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            n.data
                .sockets
                .iter()
                .flat_map(|s| s.inputs.iter().map(|h| h.id.as_str()))
                .collect(),
            n.data
                .sockets
                .iter()
                .flat_map(|s| s.outputs.iter().map(|h| h.id.as_str()))
                .collect(),
        ]
    }))
    .collect();
    for ids in lists {
        for id in duplicates(ids.into_iter()) {
            issues.push(GraphIssue::DuplicateId {
                definition: definition.to_owned(),
                id,
            });
        }
    }
    let nodes: HashSet<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &doc.edges {
        if !nodes.contains(edge.source.as_str()) || !nodes.contains(edge.target.as_str()) {
            issues.push(GraphIssue::DanglingEdge {
                definition: definition.to_owned(),
                edge: edge.id.clone(),
            });
        }
    }
    // A zone's output depends on its input without a link saying so.
    let outputs: HashSet<&str> = doc
        .nodes
        .iter()
        .filter(|n| is_zone_output(&n.node_type))
        .map(|n| n.id.as_str())
        .collect();
    let mut links: Vec<(&str, &str)> = doc
        .edges
        .iter()
        .map(|e| (e.source.as_str(), e.target.as_str()))
        .collect();
    for n in &doc.nodes {
        if let Some(output) = paired_output_id(n).filter(|o| outputs.contains(o)) {
            links.push((&n.id, output));
        }
    }
    let ids: Vec<&str> = nodes.iter().copied().collect();
    match longest_path(&ids, &links) {
        None => issues.push(GraphIssue::EdgeCycle {
            definition: definition.to_owned(),
        }),
        Some(l) if l > MAX_CHAIN_LENGTH => {
            issues.push(GraphIssue::ChainTooLong {
                definition: definition.to_owned(),
            });
        }
        _ => {}
    }
    issues
}

/// A node id is looked up across the whole project, so one in two definitions
/// would shadow the other. Within one definition `document_issues` reports it.
fn node_id_issues(project: &GraphProject) -> Vec<GraphIssue> {
    let mut owner: HashMap<&str, &str> = HashMap::new();
    let mut issues = Vec::new();
    for (key, doc) in &project.definitions {
        for node in &doc.nodes {
            match owner.get(node.id.as_str()) {
                Some(first) if *first != key.as_str() => issues.push(GraphIssue::NodeIdInTwoDefinitions {
                    id: node.id.clone(),
                    definition: key.clone(),
                }),
                Some(_) => {}
                None => {
                    owner.insert(&node.id, key);
                }
            }
        }
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
    let asset_ids: Vec<&str> = project.assets.iter().map(|a| a.id()).collect();
    for id in duplicates(asset_ids.into_iter()) {
        issues.push(GraphIssue::DuplicateAsset { id });
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
            if node
                .data
                .sockets
                .as_ref()
                .is_some_and(|s| !has_valid_socket_names(s))
            {
                issues.push(GraphIssue::InvalidSocketNames {
                    definition: key.clone(),
                    node: node.id.clone(),
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
                    issues.push(GraphIssue::RootInstance {
                        definition: key.clone(),
                    });
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
    /// Read a saved project: parse it, check the schema's limits, fill in the
    /// defaults, then check it keeps every rule the editor relies on.
    pub fn from_json(text: &str) -> Result<GraphProject, LoadError> {
        if text.len() > MAX_JSON_BYTES {
            return Err(LoadError::TooLarge);
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| LoadError::InvalidJson)?;
        let mut project: GraphProject =
            serde_json::from_value(value).map_err(|e| LoadError::InvalidSchema(e.to_string()))?;
        let schema = schema_issues(&project);
        if !schema.is_empty() {
            return Err(LoadError::InvalidSchema(schema.join("; ")));
        }
        // Checked before the library is registered, so a file can't pose as one of its entries.
        let reserved = project
            .definitions
            .keys()
            .filter(|key| is_library_ref(key))
            .map(|key| GraphIssue::ReservedDefinition {
                definition: key.clone(),
            });
        let mut issues: Vec<GraphIssue> = reserved.collect();
        issues.extend(node_id_issues(&project));
        normalise_project(&mut project);
        crate::library::register(&mut project);
        project.reindex();
        issues.extend(project_issues(&project));
        if issues.is_empty() {
            Ok(project)
        } else {
            Err(LoadError::InvalidGraph(issues))
        }
    }

    /// The project as saved: pretty JSON of the file format.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a project serialises")
    }
}
