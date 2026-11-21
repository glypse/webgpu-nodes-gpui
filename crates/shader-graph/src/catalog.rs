//! The add menu's catalog: every entry a graph can add, grouped by heading,
//! and the search over them.

use std::collections::HashSet;

use crate::graph::{
    FRAG_OUTPUT, GROUP_INPUT, GROUP_OUTPUT, GraphModel, GraphNode, NODE_GROUP, Point, ROOT_ID,
};
use crate::nodes::{default_data_for_type, registered, registered_nodes, resolve_descriptor};
use crate::types::{
    EnumVariant, InputValue, NodeCategory, NodeData, NodeDescriptor, WgslType, enum_value,
    is_connection_compatible,
};
use crate::zones::{NEW_ITEM_HANDLE, ZoneKind, paired_output_id};

#[derive(Clone, Debug, PartialEq)]
pub enum EntryKind {
    Node { node_type: String },
    /// An instance of an existing group definition.
    Group { definition_ref: String },
    Zone(ZoneKind),
}

/// One of an entry's choices: `label` names it after the entry's own.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryOption {
    pub label: String,
    pub data: NodeData,
    /// It's the option the entry is created with anyway.
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddMenuEntry {
    pub key: String,
    pub label: String,
    pub aliases: Vec<String>,
    pub heading: String,
    pub kind: EntryKind,
    pub data: Option<NodeData>,
    /// Choices made when creating it (a type, an enum): search picks the
    /// options a query names.
    pub choices: Vec<Vec<EntryOption>>,
    /// Adding it here would make a group contain itself.
    pub disabled: bool,
    /// An unused group definition, which the menu offers to delete.
    pub deletable: bool,
}

pub const GROUPS_HEADING: &str = "Groups";

pub fn category_label(category: NodeCategory) -> &'static str {
    match category {
        NodeCategory::Input => "Input",
        NodeCategory::Math => "Math",
        NodeCategory::Vector => "Vector",
        NodeCategory::Logic => "Logic",
        NodeCategory::Integer => "Integer",
        NodeCategory::Converter => "Converter",
        NodeCategory::Zone => "Zones",
        NodeCategory::Color => "Color",
        NodeCategory::Misc => "Miscellaneous",
        NodeCategory::Layout => "Layout",
    }
}

fn merge_data(base: &NodeData, over: &NodeData) -> NodeData {
    let mut data = over.with_defaults(base);
    data.label = over.label.clone().or_else(|| base.label.clone());
    data
}

/// `entry` created with `options` picked, named after them.
pub fn with_options(entry: &AddMenuEntry, options: &[EntryOption]) -> AddMenuEntry {
    if options.is_empty() {
        return entry.clone();
    }
    let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
    let mut key = vec![entry.key.as_str()];
    key.extend(&labels);
    let mut label = vec![entry.label.as_str()];
    label.extend(&labels);
    let data = options
        .iter()
        .fold(entry.data.clone().unwrap_or_default(), |data, o| merge_data(&data, &o.data));
    AddMenuEntry {
        key: key.join("~"),
        label: label.join(" "),
        data: Some(data),
        choices: Vec::new(),
        ..entry.clone()
    }
}

/// Choices with more than one option, their labels qualified where two
/// choices share one.
fn choice_options(choices: Vec<(String, Vec<EntryOption>)>) -> Vec<Vec<EntryOption>> {
    let useful: Vec<_> = choices.into_iter().filter(|(_, o)| o.len() > 1).collect();
    let labels: Vec<&str> = useful.iter().flat_map(|(_, o)| o.iter().map(|o| o.label.as_str())).collect();
    let ambiguous: HashSet<String> = labels
        .iter()
        .enumerate()
        .filter(|(i, l)| labels.iter().position(|x| x == *l) != Some(*i))
        .map(|(_, l)| (*l).to_owned())
        .collect();
    useful
        .into_iter()
        .map(|(name, options)| {
            options
                .into_iter()
                .map(|o| {
                    if ambiguous.contains(&o.label) {
                        EntryOption { label: format!("{name} {}", o.label), ..o }
                    } else {
                        o
                    }
                })
                .collect()
        })
        .collect()
}

fn node_choices(descriptor: &NodeDescriptor) -> Vec<Vec<EntryOption>> {
    let mut choices = Vec::new();
    if let Some(pickable) = descriptor.pickable_types() {
        let default = if pickable.inferable {
            None
        } else {
            descriptor
                .default_data
                .types
                .get(&pickable.key)
                .copied()
                .or_else(|| pickable.types.first().copied())
        };
        choices.push((
            "Type".to_owned(),
            pickable
                .types
                .iter()
                .map(|t| {
                    let mut data = NodeData::default();
                    data.types.insert(pickable.key.clone(), *t);
                    EntryOption { label: t.id().to_owned(), data, is_default: Some(*t) == default }
                })
                .collect(),
        ));
    }
    for setting in &descriptor.settings {
        let default = setting.value(&descriptor.default_data).to_owned();
        choices.push((
            setting.label.to_owned(),
            setting
                .options
                .iter()
                .map(|(value, label)| EntryOption {
                    label: (*label).to_owned(),
                    data: NodeData::default().setting(setting.key, value),
                    is_default: *value == default,
                })
                .collect(),
        ));
    }
    choice_options(choices)
}

fn group_choices(definition: &crate::graph::GraphDocument) -> Vec<Vec<EntryOption>> {
    let mut choices = Vec::new();
    for variable in &definition.type_variables {
        choices.push((
            variable.name.clone(),
            variable
                .types
                .iter()
                .map(|t| {
                    let mut data = NodeData::default();
                    data.types.insert(variable.id.clone(), *t);
                    EntryOption { label: t.id().to_owned(), data, is_default: false }
                })
                .collect(),
        ));
    }
    for input in &definition.inputs {
        let Some(variants) = input.variants.as_ref().filter(|_| input.owner.is_none()) else {
            continue;
        };
        let default = enum_value(definition.defaults.get(&input.id), variants);
        choices.push((
            input.label.clone(),
            variants
                .iter()
                .map(|v: &EnumVariant| {
                    let mut data = NodeData::default();
                    data.values.insert(input.id.clone(), InputValue::from(v.value));
                    EntryOption { label: v.label.clone(), data, is_default: Some(v.int()) == default }
                })
                .collect(),
        ));
    }
    choice_options(choices)
}

/// Every entry of the add menu in menu order: registered nodes by category,
/// then the groups of the project.
pub fn add_menu_entries(model: &GraphModel) -> Vec<AddMenuEntry> {
    let mut entries = Vec::new();
    let zone = |key: &str, label: &str, aliases: &[&str], kind| AddMenuEntry {
        key: key.to_owned(),
        label: label.to_owned(),
        aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
        heading: category_label(NodeCategory::Zone).to_owned(),
        kind: EntryKind::Zone(kind),
        data: None,
        choices: Vec::new(),
        disabled: false,
        deletable: false,
    };
    entries.push(zone(
        "zone:repeat",
        "Repeat Zone",
        &["Loop", "For Loop", "For Each", "Iterate", "While"],
        ZoneKind::Repeat,
    ));
    entries.push(zone(
        "zone:simulation",
        "Simulation Zone",
        &["Feedback", "Previous Frame", "Persistent", "State", "Ping Pong"],
        ZoneKind::Simulation,
    ));
    for descriptor in registered_nodes() {
        if descriptor.hidden {
            continue;
        }
        entries.push(AddMenuEntry {
            key: format!("node:{}", descriptor.type_id),
            label: descriptor.label.clone(),
            aliases: descriptor.aliases.clone(),
            heading: category_label(descriptor.category).to_owned(),
            kind: EntryKind::Node { node_type: descriptor.type_id.clone() },
            data: None,
            choices: node_choices(descriptor),
            disabled: false,
            deletable: false,
        });
    }
    if model.active_definition_ref() != ROOT_ID {
        for (ty, label, category, aliases) in [
            (GROUP_INPUT, "Group Input", NodeCategory::Input, ["Parameters", "Arguments"]),
            (GROUP_OUTPUT, "Group Output", NodeCategory::Misc, ["Result", "Return"]),
        ] {
            entries.push(AddMenuEntry {
                key: format!("node:{ty}"),
                label: label.to_owned(),
                aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
                heading: category_label(category).to_owned(),
                kind: EntryKind::Node { node_type: ty.to_owned() },
                data: None,
                choices: Vec::new(),
                disabled: false,
                deletable: false,
            });
        }
    }
    let used: HashSet<&str> = model
        .project()
        .definitions
        .values()
        .flat_map(|d| d.group_refs())
        .collect();
    for (reference, doc) in &model.project().definitions {
        if reference == ROOT_ID {
            continue;
        }
        entries.push(AddMenuEntry {
            key: format!("group:{reference}"),
            label: doc.label.clone().unwrap_or_else(|| "Group".to_owned()),
            aliases: Vec::new(),
            heading: GROUPS_HEADING.to_owned(),
            kind: EntryKind::Group { definition_ref: reference.clone() },
            data: None,
            choices: group_choices(doc),
            disabled: model.would_create_group_cycle(reference),
            deletable: !used.contains(reference.as_str()),
        });
    }
    let rank = |heading: &str| {
        NodeCategory::ALL
            .iter()
            .position(|c| category_label(*c) == heading)
            .unwrap_or(if heading == GROUPS_HEADING { NodeCategory::ALL.len() } else { usize::MAX })
    };
    entries.sort_by_key(|e| rank(&e.heading));
    entries
}

/// Entries under their heading, keeping order.
pub fn group_by_heading(entries: &[AddMenuEntry]) -> Vec<(String, Vec<&AddMenuEntry>)> {
    let mut groups: Vec<(String, Vec<&AddMenuEntry>)> = Vec::new();
    for entry in entries {
        match groups.iter_mut().find(|(h, _)| *h == entry.heading) {
            Some((_, list)) => list.push(entry),
            None => groups.push((entry.heading.clone(), vec![entry])),
        }
    }
    groups
}

/// Creates what an entry stands for at `position`; returns the id of the
/// created node (a zone's input).
pub fn add_entry(model: &mut GraphModel, entry: &AddMenuEntry, position: Point) -> Option<String> {
    if entry.disabled {
        return None;
    }
    match &entry.kind {
        EntryKind::Zone(kind) => model.create_zone(*kind, position),
        EntryKind::Group { definition_ref } => {
            let id = model.add_group_instance(definition_ref, position)?;
            if let Some(data) = &entry.data {
                let data = data.clone();
                model.update_node_data(&id, |d| {
                    d.types.extend(data.types);
                    d.values.extend(data.values);
                });
            }
            Some(id)
        }
        EntryKind::Node { node_type } if node_type == NODE_GROUP => model.create_group(position),
        EntryKind::Node { node_type } => {
            model.add_node_with(node_type, position, entry.data.clone())
        }
    }
}

/// The link a dragged connection started from.
#[derive(Clone, Debug)]
pub struct Pending {
    pub node: String,
    pub handle: String,
    /// Dragged from an output.
    pub from_output: bool,
}

fn fresh_node(entry: &AddMenuEntry) -> Option<GraphNode> {
    match &entry.kind {
        EntryKind::Group { definition_ref } => {
            let mut node = GraphNode::new(NODE_GROUP, Point::default());
            node.data = entry.data.clone().unwrap_or_default();
            node.group = Some(crate::graph::GroupRef {
                definition_ref: definition_ref.clone(),
                locked: true,
            });
            Some(node)
        }
        EntryKind::Node { node_type } if node_type != NODE_GROUP => {
            registered(node_type)?;
            let mut node = GraphNode::new(node_type, Point::default());
            node.data = match &entry.data {
                Some(d) => d.with_defaults(&default_data_for_type(node_type)),
                None => default_data_for_type(node_type),
            };
            Some(node)
        }
        _ => None,
    }
}

/// The handle of a freshly created `entry` a link dragged from `pending`
/// would connect to; handles of the dragged value's scalar kind first.
pub fn entry_handle_for(model: &GraphModel, entry: &AddMenuEntry, pending: &Pending) -> Option<String> {
    if matches!(entry.kind, EntryKind::Zone(_)) {
        return Some(NEW_ITEM_HANDLE.to_owned());
    }
    let node = fresh_node(entry)?;
    let descriptor = resolve_descriptor(&node, model.project());
    let rank = |candidates: &[WgslType], value: WgslType, value_is_source: bool| -> u8 {
        let fits: Vec<_> = candidates
            .iter()
            .filter(|t| {
                if value_is_source {
                    is_connection_compatible(value, **t)
                } else {
                    is_connection_compatible(**t, value)
                }
            })
            .collect();
        if fits.is_empty() {
            2
        } else if fits.iter().any(|t| t.kind() == value.kind()) {
            0
        } else {
            1
        }
    };
    let mut found: Option<String> = None;
    let mut best = 2;
    if pending.from_output {
        let value = model.resolve_output_handle_type(&pending.node, &pending.handle);
        for input in &descriptor.inputs {
            let score = rank(&input.types, value, true);
            if score < best {
                best = score;
                found = Some(input.id.clone());
            }
        }
    } else {
        let value = model.resolve_input_type(&pending.node, &pending.handle);
        for output in &descriptor.outputs {
            let score = rank(&[output.ty], value, false);
            if score < best {
                best = score;
                found = Some(output.id.clone());
            }
        }
    }
    found
}

/// The entries a link dragged from `pending` can be dropped into.
pub fn connectable_entries(model: &GraphModel, entries: &[AddMenuEntry], pending: &Pending) -> Vec<AddMenuEntry> {
    entries
        .iter()
        .filter(|e| entry_handle_for(model, e, pending).is_some())
        .cloned()
        .collect()
}

/// Creates the entry and links it to the dragged connection's handle.
pub fn add_entry_connected(
    model: &mut GraphModel,
    entry: &AddMenuEntry,
    position: Point,
    pending: &Pending,
) -> Option<String> {
    let handle = entry_handle_for(model, entry, pending);
    // Feeding an input: the new node's outputs sit at the drop point.
    let upstream = !pending.from_output;
    let offset = if !upstream {
        0.0
    } else if matches!(entry.kind, EntryKind::Zone(_)) {
        500.0 + 180.0
    } else {
        180.0
    };
    let created = add_entry(model, entry, Point::new(position.x - offset, position.y))?;
    let Some(handle) = handle else { return Some(created) };
    let node = if matches!(entry.kind, EntryKind::Zone(_)) && upstream {
        model
            .active_document()
            .node(&created)
            .and_then(paired_output_id)
            .map_or(created.clone(), str::to_owned)
    } else {
        created.clone()
    };
    let connection = if upstream {
        crate::graph::Connection::new(node, handle, &pending.node, &pending.handle)
    } else {
        crate::graph::Connection::new(&pending.node, &pending.handle, node, handle)
    };
    if model.is_valid_connection(&connection) {
        model.connect(connection);
    }
    Some(created)
}

// --- Search ---

/// An entry search found, with where the query matched.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub entry: AddMenuEntry,
    /// The alias matched, when the label didn't.
    pub alias: Option<String>,
    /// Byte ranges of the label that matched.
    pub label_ranges: Vec<(usize, usize)>,
    pub alias_ranges: Vec<(usize, usize)>,
}

/// Text split into words with their byte offsets.
fn words_of(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, &text[s..i]));
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out
}

fn within_one_edit(a: &[char], b: &[char]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        if a.len() > b.len() {
            i += 1;
        } else if a.len() < b.len() {
            j += 1;
        } else {
            i += 1;
            j += 1;
        }
    }
    edits + (a.len() - i) + (b.len() - j) <= 1
}

/// How one query word matches one text word: 0 as a contiguous piece,
/// 1 with letters skipped, 2 with a typo; the matched byte ranges within it.
fn match_word(query: &str, word: &str) -> Option<(u8, Vec<(usize, usize)>)> {
    let lower = word.to_lowercase();
    if lower.len() == word.len()
        && let Some(at) = lower.find(query)
    {
        return Some((0, vec![(at, at + query.len())]));
    }
    // Skipped letters, the first one matching the word's start.
    let mut q = query.chars().peekable();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, c) in word.char_indices() {
        if let Some(&want) = q.peek()
            && c.to_lowercase().next() == Some(want)
        {
            match ranges.last_mut() {
                Some(last) if last.1 == i => last.1 = i + c.len_utf8(),
                _ => ranges.push((i, i + c.len_utf8())),
            }
            q.next();
        }
    }
    if q.peek().is_none() && ranges.first().is_some_and(|r| r.0 == 0) {
        return Some((1, ranges));
    }
    let (qc, wc): (Vec<char>, Vec<char>) = (query.chars().collect(), lower.chars().collect());
    (qc.len() >= 4 && within_one_edit(&qc, &wc)).then(|| (2, vec![(0, word.len())]))
}

/// Every query word matched in `name`'s words, else in `heading`'s; the
/// worst kind of match and the ranges within `name`.
fn match_text(query: &[String], name: &str, heading: &str) -> Option<(u8, Vec<(usize, usize)>)> {
    let name_words = words_of(name);
    let heading_words = words_of(heading);
    let mut used_name = vec![false; name_words.len()];
    let mut used_heading = vec![false; heading_words.len()];
    let mut tier = 0u8;
    let mut ranges = Vec::new();
    for q in query {
        let mut best: Option<(u8, usize, Vec<(usize, usize)>, bool)> = None;
        for (i, (offset, word)) in name_words.iter().enumerate() {
            if used_name[i] {
                continue;
            }
            if let Some((kind, r)) = match_word(q, word)
                && best.as_ref().is_none_or(|b| kind < b.0)
            {
                best = Some((kind, i, r.into_iter().map(|(a, b)| (a + offset, b + offset)).collect(), false));
            }
        }
        if best.is_none() {
            for (i, (_, word)) in heading_words.iter().enumerate() {
                if used_heading[i] {
                    continue;
                }
                if let Some((kind, _)) = match_word(q, word)
                    && best.as_ref().is_none_or(|b| kind < b.0)
                {
                    best = Some((kind, i, Vec::new(), true));
                }
            }
        }
        let (kind, i, r, in_heading) = best?;
        if in_heading {
            used_heading[i] = true;
            tier = tier.max(4 + kind);
        } else {
            used_name[i] = true;
            tier = tier.max(kind);
            ranges.extend(r);
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => merged.push(r),
        }
    }
    Some((tier, merged))
}

const MAX_COMBINATIONS: usize = 64;

/// Entries matching `query`, best first. Options of an entry's choices the
/// query names are picked: "value vec2i" finds "Value vec2i". `accepts`
/// rules out entries, with their options picked, that can't be used.
pub fn search_entries(
    entries: &[AddMenuEntry],
    query: &str,
    accepts: &dyn Fn(&AddMenuEntry) -> bool,
) -> Vec<SearchResult> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let named = |label: &str| {
        let l = label.to_lowercase();
        words.iter().any(|w| (w.len() >= 2 || l == *w) && l.contains(w.as_str()))
    };
    struct Hit {
        key: String,
        tier: u8,
        picked: usize,
        result: SearchResult,
    }
    let mut hits: Vec<Hit> = Vec::new();
    for entry in entries {
        // Every way of picking none or one named option in each choice.
        let mut combos: Vec<Vec<EntryOption>> = vec![Vec::new()];
        for choice in &entry.choices {
            let options: Vec<&EntryOption> = choice.iter().filter(|o| named(&o.label)).collect();
            if options.is_empty() {
                continue;
            }
            let mut next = Vec::new();
            for combo in &combos {
                next.push(combo.clone());
                for o in &options {
                    let mut c = combo.clone();
                    c.push((*o).clone());
                    next.push(c);
                }
            }
            next.truncate(MAX_COMBINATIONS);
            combos = next;
        }
        for picked in combos {
            let created = with_options(entry, &picked);
            let key = std::iter::once(entry.key.clone())
                .chain(picked.iter().filter(|o| !o.is_default).map(|o| o.label.clone()))
                .collect::<Vec<_>>()
                .join("~");
            if !picked.is_empty() && !accepts(&created) {
                continue;
            }
            let option_text = picked.iter().map(|o| o.label.as_str()).collect::<Vec<_>>().join(" ");
            let names = std::iter::once((entry.label.as_str(), None))
                .chain(entry.aliases.iter().map(|a| (a.as_str(), Some(a.as_str()))));
            for (name, alias) in names {
                let text = if option_text.is_empty() { name.to_owned() } else { format!("{name} {option_text}") };
                let Some((mut tier, ranges)) = match_text(&words, &text, &entry.heading) else {
                    continue;
                };
                // Picked options must have been matched by a word.
                if !picked.is_empty() {
                    let start = name.len() + 1;
                    if !ranges.iter().any(|r| r.1 > start) {
                        continue;
                    }
                }
                if alias.is_some() {
                    tier += 1;
                }
                // A label shifts nothing; an alias keeps its own ranges.
                let (label_ranges, alias_ranges) = if alias.is_some() {
                    (Vec::new(), ranges.iter().map(|r| (r.0, r.1.min(name.len()))).collect())
                } else {
                    let label_len = created.label.len();
                    (ranges.iter().map(|r| (r.0, r.1.min(label_len))).collect(), Vec::new())
                };
                hits.push(Hit {
                    key: key.clone(),
                    tier,
                    picked: picked.len(),
                    result: SearchResult {
                        entry: created.clone(),
                        alias: alias.map(str::to_owned),
                        label_ranges,
                        alias_ranges,
                    },
                });
            }
        }
    }
    hits.sort_by_key(|h| (h.tier, h.result.entry.label.len()));
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut out: Vec<Hit> = Vec::new();
    for hit in hits {
        match seen.iter().position(|(k, _)| *k == hit.key) {
            Some(i) => {
                if hit.picked > out[seen[i].1].picked {
                    let at = seen[i].1;
                    out[at] = hit;
                }
            }
            None => {
                seen.push((hit.key.clone(), out.len()));
                out.push(hit);
            }
        }
    }
    out.into_iter().map(|h| h.result).collect()
}

/// The frag output node is never offered.
pub fn is_output(entry: &AddMenuEntry) -> bool {
    matches!(&entry.kind, EntryKind::Node { node_type } if node_type == FRAG_OUTPUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<AddMenuEntry> {
        add_menu_entries(&GraphModel::default())
    }

    fn found(query: &str) -> Vec<String> {
        search_entries(&entries(), query, &|_| true).into_iter().map(|r| r.entry.label).collect()
    }

    #[test]
    fn entries_are_grouped_in_menu_order() {
        let e = entries();
        assert!(e.iter().any(|e| e.label == "Repeat Zone"));
        let headings: Vec<_> = group_by_heading(&e).into_iter().map(|(h, _)| h).collect();
        assert_eq!(headings.first().map(String::as_str), Some("Input"));
        assert!(!e.iter().any(|e| e.disabled));
    }

    #[test]
    fn search_finds_by_label_alias_and_skipped_letters() {
        assert!(found("smoothstep").iter().any(|l| l.contains("Smoothstep")));
        assert!(found("smst").iter().any(|l| l.contains("Smoothstep")));
        assert!(found("loop").contains(&"Repeat Zone".to_owned()));
    }

    #[test]
    fn search_tolerates_a_typo() {
        assert!(found("smoth").iter().any(|l| l.contains("Smoothstep")));
    }

    #[test]
    fn search_picks_named_options() {
        let hits = found("value vec2i");
        assert!(hits.contains(&"Declare vec2i".to_owned()), "{hits:?}");
    }

    #[test]
    fn empty_query_finds_nothing() {
        assert!(found("  ").is_empty());
    }

    #[test]
    fn adding_a_node_entry_creates_it() {
        let mut model = GraphModel::default();
        let entry = entries().into_iter().find(|e| e.label == "Time").unwrap();
        let id = add_entry(&mut model, &entry, Point::new(1.0, 2.0)).unwrap();
        assert!(model.node(&id).is_some());
    }

    #[test]
    fn dropped_links_only_offer_nodes_that_can_take_them() {
        let model = GraphModel::default();
        let output = model.root().nodes.iter().find(|n| n.is(FRAG_OUTPUT)).unwrap().id.clone();
        let input = model.resolve_node_inputs(&output)[0].id.clone();
        let pending = Pending { node: output, handle: input, from_output: false };
        let offered = connectable_entries(&model, &entries(), &pending);
        assert!(!offered.is_empty() && offered.len() < entries().len());
    }
}
