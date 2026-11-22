//! Group bundles: exporting a group with the user groups and assets it
//! depends on, and importing the groups of bundles into a project.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use super::hydrate::normalise_project;
use super::limits::{MAX_DEFINITIONS, MAX_GROUP_DEPTH, MAX_JSON_BYTES, MAX_PROJECT_NODES};
use super::validate::{GraphIssue, LoadError, document_issues, longest_path, schema_issues};
use super::{GraphDocument, GraphModel, GraphProject, ROOT_ID, new_id};
use crate::assets::{Asset, BundleError, BundleKind, asset_file, unpack_bundle, verified_files};
use crate::library::{self, is_library_ref};

/// One group of a groups bundle: its definition under `id`, the user groups it
/// nests, and how it is listed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupEntry {
    pub id: String,
    pub label: String,
    pub category: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub definition: GraphDocument,
}

/// The JSON of a groups bundle, beside the files of the assets its groups read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupsFile {
    pub groups: Vec<GroupEntry>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A group's export: the JSON of its groups bundle, the assets it reads, and a
/// name suiting its file.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupsExport {
    pub name: String,
    pub json: String,
    pub assets: Vec<Asset>,
}

/// Where an imported asset's file is in the bundle it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetSource {
    /// The index of the file among those imported.
    pub bundle: usize,
    pub path: String,
}

/// An asset new to the project, with the file it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedAsset {
    pub asset: Asset,
    pub source: AssetSource,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupImport {
    pub imported: usize,
    /// Each dependency the project and the imported groups don't have, as a sentence.
    pub missing: Vec<String>,
    pub added: Vec<ImportedAsset>,
}

/// A groups bundle as read: its groups, without the assets whose files aren't
/// what they say, and the bundle's files by path.
#[derive(Debug)]
pub struct GroupsBundle {
    pub file: GroupsFile,
    pub altered: Vec<Asset>,
    pub files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Error)]
pub enum GroupsReadError {
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error(transparent)]
    Load(#[from] LoadError),
}

/// Reads a groups bundle's JSON, checking the limits a file keeps.
pub fn parse_groups(text: &str) -> Result<GroupsFile, LoadError> {
    if text.len() > MAX_JSON_BYTES {
        return Err(LoadError::TooLarge);
    }
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| LoadError::InvalidJson)?;
    let file: GroupsFile =
        serde_json::from_value(value).map_err(|e| LoadError::InvalidSchema(e.to_string()))?;
    let mut problems = Vec::new();
    if file.groups.len() > MAX_DEFINITIONS {
        problems.push(format!("groups: Expected <={MAX_DEFINITIONS} but received {}", file.groups.len()));
    }
    for group in &file.groups {
        if group.dependencies.len() > MAX_DEFINITIONS {
            problems.push(format!("{}.dependencies: too many", group.id));
        }
    }
    if let Some(asset) = file.assets.iter().find(|asset| !asset.is_valid()) {
        problems.push(format!("assets: \"{}\" isn't a valid asset", asset.name()));
    }
    let mut probe = GraphProject::from_definitions(
        file.groups.iter().map(|g| (g.id.clone(), g.definition.clone())).collect(),
    );
    probe.assets = file.assets.clone();
    problems.extend(schema_issues(&probe));
    if problems.is_empty() { Ok(file) } else { Err(LoadError::InvalidSchema(problems.join("; "))) }
}

/// Reads a groups bundle, leaving out the assets whose files aren't what they say.
pub fn read_groups(bytes: &[u8]) -> Result<GroupsBundle, GroupsReadError> {
    let bundle = unpack_bundle(bytes, BundleKind::Groups)?;
    let mut file = parse_groups(&bundle.json)?;
    let altered = verified_files(&bundle, &file.assets).altered;
    file.assets.retain(|asset| !altered.contains(asset));
    Ok(GroupsBundle { file, altered, files: bundle.files })
}

/// The groups of a bundle's definitions, as one document each for import checks.
#[derive(Default)]
struct Staged {
    definitions: BTreeMap<String, GraphDocument>,
    /// The ref each entry's id became, for those given a new one.
    ids: HashMap<String, String>,
}

/// What a definition is for comparing with the project's: its nodes, links and interface, not its label or view.
fn content_of(doc: &GraphDocument) -> serde_json::Value {
    json!({
        "nodes": doc.nodes,
        "edges": doc.edges,
        "typeVariables": doc.type_variables,
        "inputs": doc.inputs,
        "outputs": doc.outputs,
        "defaults": doc.defaults,
    })
}

/// Lower-cased, with each run of other characters as one dash.
fn file_stem(label: &str) -> String {
    let mut stem = String::new();
    let mut dash = false;
    for c in label.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            stem.push(c);
            dash = false;
        } else if !dash {
            stem.push('-');
            dash = true;
        }
    }
    stem
}

/// The problems of definitions taken together: their number and size, and the
/// links among them.
fn reference_issues(definitions: &BTreeMap<&str, &GraphDocument>) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    if definitions.len() > MAX_DEFINITIONS {
        issues.push(GraphIssue::TooManyDefinitions);
    }
    let nodes: usize = definitions.values().map(|doc| doc.nodes.len()).sum();
    if nodes > MAX_PROJECT_NODES {
        issues.push(GraphIssue::TooManyNodes);
    }
    let mut links: Vec<(&str, &str)> = Vec::new();
    for (definition, doc) in definitions {
        for target in doc.group_refs() {
            links.push((definition, target));
        }
    }
    for (definition, target) in &links {
        if *target == ROOT_ID {
            issues.push(GraphIssue::RootInstance { definition: (*definition).to_owned() });
        }
    }
    let ids: Vec<&str> = definitions.keys().copied().collect();
    match longest_path(&ids, &links) {
        None => issues.push(GraphIssue::GroupCycle),
        Some(depth) if depth > MAX_GROUP_DEPTH => issues.push(GraphIssue::GroupsTooDeep),
        Some(_) => {}
    }
    issues
}

impl GraphModel {
    /// User groups reachable through group nodes from `seeds`, standard-library
    /// definitions excluded: they never reference user groups.
    fn reachable_user_refs(&self, seeds: Vec<String>) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut found = Vec::new();
        let mut queue = seeds;
        while let Some(reference) = queue.pop() {
            if is_library_ref(&reference) || !seen.insert(reference.clone()) {
                continue;
            }
            if let Some(doc) = self.project().document(&reference) {
                queue.extend(doc.group_refs().map(str::to_owned));
            }
            found.push(reference);
        }
        found
    }

    /// The user groups `definition_ref` nests, at any depth, not counting itself.
    fn nested_user_refs(&self, definition_ref: &str) -> Vec<String> {
        let seeds = self.project().document(definition_ref).map_or_else(Vec::new, |doc| {
            doc.group_refs().map(str::to_owned).collect()
        });
        self.reachable_user_refs(seeds).into_iter().filter(|r| r != definition_ref).collect()
    }

    fn export_entry(&self, definition_ref: &str) -> Option<GroupEntry> {
        let doc = self.project().document(definition_ref)?;
        let library_entry = library::entries().iter().find(|e| e.reference == definition_ref);
        let label = doc
            .label
            .clone()
            .or_else(|| library_entry.map(|e| e.label.clone()))
            .unwrap_or_else(|| "Group".to_owned());
        Some(GroupEntry {
            id: definition_ref.to_owned(),
            label,
            category: library_entry.map_or_else(|| "group".to_owned(), |e| e.category.clone()),
            dependencies: self.nested_user_refs(definition_ref),
            definition: doc.clone(),
        })
    }

    /// The group `definition_ref` with the user groups it nests, and the assets
    /// they read, as the JSON of a groups bundle. Standard-library dependencies
    /// are left out.
    pub fn export_group(&self, definition_ref: &str) -> Option<GroupsExport> {
        let mut refs = vec![definition_ref.to_owned()];
        refs.extend(self.nested_user_refs(definition_ref));
        let groups: Vec<GroupEntry> = refs.iter().filter_map(|r| self.export_entry(r)).collect();
        let used: HashSet<&str> = groups
            .iter()
            .flat_map(|g| g.definition.nodes.iter().filter_map(|n| n.data.asset.as_deref()))
            .collect();
        let assets: Vec<Asset> =
            self.project().assets.iter().filter(|a| used.contains(a.id())).cloned().collect();
        let label = groups.first().map_or("group", |g| g.label.as_str());
        let name = file_stem(label);
        let json = serde_json::to_string_pretty(&GroupsFile { groups, assets: assets.clone() }).ok()?;
        Some(GroupsExport { name, json, assets })
    }

    /// Imports the groups of `files`, all of them or none. They never replace
    /// what's there: a group differing from the project's with the same id comes
    /// in as a copy, and the groups nesting it follow; the standard library's
    /// stay the app's own. An asset whose file the project already has (the same
    /// hash) stands in for its copy; the new ones are in `added`, each with the
    /// file it came from. Missing dependencies don't block the import.
    pub fn import_groups(&mut self, files: &[GroupsFile]) -> Result<GroupImport, LoadError> {
        let (asset_ids, added) = self.stage_assets(files);
        let mut entries: Vec<(&GroupEntry, GraphDocument)> = Vec::new();
        for (index, file) in files.iter().enumerate() {
            for group in file.groups.iter().filter(|g| !is_library_ref(&g.id)) {
                let mut definition = group.definition.clone();
                for node in &mut definition.nodes {
                    let mapped = node
                        .data
                        .asset
                        .as_ref()
                        .and_then(|asset| asset_ids.get(index).and_then(|ids| ids.get(asset)));
                    if let Some(id) = mapped.cloned() {
                        node.data.asset = Some(id);
                    }
                }
                definition.label = Some(group.label.clone());
                definition.locked = true;
                entries.push((group, definition));
            }
        }
        let staged = self.stage_definitions(&entries);

        let mut issues: Vec<GraphIssue> = entries
            .iter()
            .filter(|(group, _)| group.id == ROOT_ID)
            .map(|_| GraphIssue::ReservedDefinition { definition: ROOT_ID.to_owned() })
            .collect();
        let mut checked: BTreeMap<&str, &GraphDocument> = self
            .project()
            .definitions
            .iter()
            .filter(|(key, _)| !is_library_ref(key))
            .map(|(key, doc)| (key.as_str(), doc))
            .collect();
        for (reference, doc) in &staged.definitions {
            checked.insert(reference.as_str(), doc);
        }
        for (reference, doc) in &staged.definitions {
            issues.extend(document_issues(reference, doc));
        }
        issues.extend(reference_issues(&checked));
        if !issues.is_empty() {
            return Err(LoadError::InvalidGraph(issues));
        }

        let mut probe = GraphProject::from_definitions(staged.definitions);
        normalise_project(&mut probe);
        self.project.assets.extend(added.iter().map(|a| a.asset.clone()));
        self.project.definitions.extend(probe.definitions);
        self.changed();

        let missing = entries
            .iter()
            .flat_map(|(group, _)| {
                group.dependencies.iter().filter_map(|dep| {
                    let target = staged.ids.get(dep).unwrap_or(dep);
                    (!self.project().definitions.contains_key(target) && !is_library_ref(target))
                        .then(|| format!("{} depends on {target}", group.id))
                })
            })
            .collect();
        Ok(GroupImport { imported: entries.len(), missing, added })
    }

    /// Each file's assets, by the id each one has in the project, and the assets
    /// that are new to it.
    fn stage_assets(&self, files: &[GroupsFile]) -> (Vec<HashMap<String, String>>, Vec<ImportedAsset>) {
        let mut added: Vec<ImportedAsset> = Vec::new();
        let mut all_ids = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let mut ids = HashMap::new();
            for asset in &file.assets {
                if ids.contains_key(asset.id()) {
                    continue;
                }
                let known = || self.project().assets.iter().chain(added.iter().map(|a| &a.asset));
                if let Some(existing) = known().find(|a| a.hash() == asset.hash()) {
                    ids.insert(asset.id().to_owned(), existing.id().to_owned());
                    continue;
                }
                // Ids are only unique within a project: this one may name another file.
                let id = if known().any(|a| a.id() == asset.id()) {
                    new_id()
                } else {
                    asset.id().to_owned()
                };
                let mut imported = asset.clone();
                let info = imported.file_info_mut();
                info.file = asset_file(asset.kind(), &id, asset.file());
                info.id = id.clone();
                ids.insert(asset.id().to_owned(), id);
                added.push(ImportedAsset {
                    asset: imported,
                    source: AssetSource { bundle: index, path: asset.file().to_owned() },
                });
            }
            all_ids.push(ids);
        }
        (all_ids, added)
    }

    /// The definitions `entries` add, by ref. One the project has, the same,
    /// adds nothing; one that differs takes a new ref, and so do those nesting
    /// it, which then differ too. Of entries sharing an id, the first counts.
    fn stage_definitions(&self, entries: &[(&GroupEntry, GraphDocument)]) -> Staged {
        let contents: Vec<serde_json::Value> = entries.iter().map(|(_, doc)| content_of(doc)).collect();
        let existing: HashMap<&str, serde_json::Value> = entries
            .iter()
            .filter_map(|(group, _)| {
                let doc = self.project().document(&group.id)?;
                Some((group.id.as_str(), content_of(doc)))
            })
            .collect();

        let mut ids: HashMap<String, String> = HashMap::new();
        let mut changed = true;
        while changed {
            changed = false;
            for ((group, doc), content) in entries.iter().zip(&contents) {
                let Some(existing_content) = existing.get(group.id.as_str()) else { continue };
                if ids.contains_key(&group.id) {
                    continue;
                }
                let nested = doc.group_refs().any(|r| ids.contains_key(r));
                if existing_content == content && !nested {
                    continue;
                }
                ids.insert(group.id.clone(), new_id());
                changed = true;
            }
        }

        let mut definitions: BTreeMap<String, GraphDocument> = BTreeMap::new();
        for (group, doc) in entries {
            let renamed = ids.get(&group.id);
            let reference = renamed.cloned().unwrap_or_else(|| group.id.clone());
            if definitions.contains_key(&reference) || (renamed.is_none() && existing.contains_key(group.id.as_str())) {
                continue;
            }
            let mut copy = doc.clone();
            if renamed.is_some() {
                copy.label = Some(self.unique_copy_label(doc.label.as_deref()));
            }
            for node in &mut copy.nodes {
                if let Some(group_ref) = node.group.as_mut()
                    && let Some(target) = ids.get(&group_ref.definition_ref)
                {
                    group_ref.definition_ref = target.clone();
                }
            }
            definitions.insert(reference, copy);
        }
        Staged { definitions, ids }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetFile, asset_hash};
    use crate::graph::{GraphEdge, GraphNode, GroupRef, NODE_GROUP, Point};
    use crate::library;
    use crate::types::{GroupHandleDef, InputValue, WgslType};

    const AT: Point = Point { x: 0.0, y: 0.0 };

    fn sin_document(id: &str) -> GraphDocument {
        let mut node = GraphNode::new("sin", AT);
        node.id = id.to_owned();
        GraphDocument { nodes: vec![node], ..GraphDocument::default() }
    }

    fn nesting(id: &str, definition_ref: &str) -> GraphDocument {
        let mut node = GraphNode::new(NODE_GROUP, AT);
        node.id = id.to_owned();
        node.group = Some(GroupRef { definition_ref: definition_ref.to_owned(), locked: false });
        GraphDocument { nodes: vec![node], ..GraphDocument::default() }
    }

    fn entry(id: &str, definition: GraphDocument, dependencies: &[&str]) -> GroupEntry {
        GroupEntry {
            id: id.to_owned(),
            label: id.to_owned(),
            category: "group".to_owned(),
            dependencies: dependencies.iter().map(|d| (*d).to_owned()).collect(),
            definition,
        }
    }

    fn file_of(groups: Vec<GroupEntry>) -> GroupsFile {
        GroupsFile { groups, assets: Vec::new() }
    }

    fn mine() -> GraphModel {
        let mut project = GraphProject::default();
        project.definitions.insert("user".to_owned(), sin_document("user-node"));
        if let Some(root) = project.definitions.get_mut(ROOT_ID) {
            let mut node = GraphNode::new("sin", AT);
            node.id = "mine".to_owned();
            root.nodes.push(node);
        }
        GraphModel::new(project)
    }

    fn image(hash: &str) -> Asset {
        Asset::Image(AssetFile {
            id: "a1".into(),
            name: "Photo".into(),
            file: "assets/images/a1.png".into(),
            mime: "image/png".into(),
            bytes: 3,
            hash: hash.into(),
            width: 1,
            height: 1,
        })
    }

    #[test]
    fn can_not_replace_the_root() {
        let mut model = mine();
        let before = model.project().clone();
        let result = model.import_groups(&[file_of(vec![entry(ROOT_ID, sin_document("evil"), &[])])]);
        assert!(result.is_err());
        assert_eq!(model.project(), &before);
        assert!(!model.root().locked);
    }

    #[test]
    fn can_not_replace_the_standard_library() {
        let library = library::entries().first().expect("the library has entries");
        let mut model = mine();
        let before = model.project().definitions.get(&library.reference).cloned();
        let result = model.import_groups(&[file_of(vec![entry(&library.reference, sin_document("evil"), &[])])]);
        assert_eq!(result.map(|r| r.imported), Ok(0));
        assert_eq!(model.project().definitions.get(&library.reference).cloned(), before);
        assert!(model.project().document(&library.reference).is_some_and(|d| d.node("evil").is_none()));
    }

    #[test]
    fn brings_a_group_differing_from_the_projects_in_as_a_copy() {
        let mut model = mine();
        let before = model.project().document("user").cloned();
        let file = file_of(vec![
            entry("user", sin_document("evil"), &[]),
            entry("outer", nesting("n", "user"), &["user"]),
        ]);
        let result = model.import_groups(&[file]).expect("imports");
        assert_eq!(result.imported, 2);
        assert!(result.missing.is_empty());
        assert_eq!(model.project().document("user").cloned(), before);

        let copy = model
            .project()
            .document("outer")
            .and_then(|outer| outer.nodes.first())
            .and_then(|n| n.definition_ref())
            .expect("outer nests a group")
            .to_owned();
        assert_ne!(copy, "user");
        assert!(model.project().document(&copy).is_some_and(|d| d.node("evil").is_some()));
    }

    #[test]
    fn copies_a_group_the_project_has_too_when_what_it_nests_had_to_be_copied() {
        let mut model = mine();
        let shared = nesting("n", "user");
        model.import_groups(&[file_of(vec![entry("outer", shared.clone(), &[])])]).expect("imports");
        let before = model.project().document("outer").cloned();

        let file = file_of(vec![entry("user", sin_document("evil"), &[]), entry("outer", shared, &["user"])]);
        assert!(model.import_groups(&[file]).is_ok());
        assert_eq!(model.project().document("outer").cloned(), before);
        assert_eq!(
            before.as_ref().and_then(|d| d.nodes.first()).and_then(|n| n.definition_ref()),
            Some("user")
        );
    }

    #[test]
    fn adds_nothing_for_groups_the_project_has_already() {
        let mut model = mine();
        model.group_selection(&["mine".to_owned()]);
        let reference = model.root().nodes.iter().find_map(|n| n.definition_ref()).expect("grouped").to_owned();
        let export = model.export_group(&reference).expect("exports");
        let file = parse_groups(&export.json).expect("parses");
        let count = model.project().definitions.len();
        assert!(model.import_groups(&[file]).is_ok());
        assert_eq!(model.project().definitions.len(), count);
    }

    #[test]
    fn keeps_a_groups_interface_through_export_and_import() {
        let mut project = GraphProject::default();
        let mut group = sin_document("s");
        group.inputs = vec![GroupHandleDef::new("a", "A", WgslType::F32.into())];
        group.outputs = vec![GroupHandleDef::new("b", "B", WgslType::F32.into())];
        group.defaults.insert("a".to_owned(), InputValue::from(2.0_f64));
        project.definitions.insert("g".to_owned(), group);
        let export = GraphModel::new(project).export_group("g").expect("exports");
        let file = parse_groups(&export.json).expect("parses");

        let mut target = GraphModel::default();
        assert!(target.import_groups(&[file]).is_ok());
        let imported = target.project().document("g").expect("imported");
        assert_eq!(imported.inputs.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(imported.outputs.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(imported.defaults.get("a"), Some(&InputValue::from(2.0_f64)));
    }

    #[test]
    fn rejects_cycles_within_the_file_or_through_the_projects_groups() {
        let mut model = mine();
        let before = model.project().clone();
        let cycle = file_of(vec![entry("a", nesting("x", "b"), &[]), entry("b", nesting("y", "a"), &[])]);
        assert_eq!(model.import_groups(&[cycle]), Err(LoadError::InvalidGraph(vec![GraphIssue::GroupCycle])));

        let self_nesting = file_of(vec![entry("c", nesting("z", "c"), &[])]);
        assert!(model.import_groups(&[self_nesting]).is_err());
        assert_eq!(model.project(), &before);
    }

    #[test]
    fn rejects_a_broken_definition_without_importing_anything() {
        let mut model = mine();
        let before = model.project().clone();
        let mut broken = sin_document("a");
        broken.edges.push(GraphEdge {
            id: "e".to_owned(),
            source: "a".to_owned(),
            source_handle: "result".to_owned(),
            target: "a".to_owned(),
            target_handle: "e".to_owned(),
        });
        let file = GroupsFile {
            groups: vec![entry("good", sin_document("g"), &[]), entry("bad", broken, &[])],
            assets: vec![image(&"a".repeat(64))],
        };
        assert!(model.import_groups(&[file]).is_err());
        assert_eq!(model.project(), &before);
        assert!(model.assets().is_empty());
    }

    #[test]
    fn rejects_more_definitions_than_a_project_can_hold() {
        let groups = (0..=MAX_DEFINITIONS).map(|i| entry(&i.to_string(), sin_document(&i.to_string()), &[])).collect();
        assert!(mine().import_groups(&[file_of(groups)]).is_err());
    }

    #[test]
    fn gives_an_asset_whose_id_names_another_file_an_id_of_its_own() {
        let mut model = mine();
        let hash_a = "a".repeat(64);
        assert!(model.add_asset(image(&hash_a)));
        let other = image(&"b".repeat(64));
        let mut document = sin_document("i");
        if let Some(node) = document.nodes.first_mut() {
            node.data.asset = Some(other.id().to_owned());
        }
        let file = GroupsFile { groups: vec![entry("g", document, &[])], assets: vec![other.clone()] };
        let result = model.import_groups(&[file]).expect("imports");
        let [added] = result.added.as_slice() else { panic!("one asset is added") };
        assert_eq!(added.source, AssetSource { bundle: 0, path: other.file().to_owned() });
        assert_ne!(added.asset.id(), "a1");
        assert_eq!(added.asset.file(), format!("assets/images/{}.png", added.asset.id()));
        assert_eq!(model.asset("a1").map(Asset::hash), Some(hash_a.as_str()));
        let node = model.project().document("g").and_then(|d| d.nodes.first());
        assert_eq!(node.and_then(|n| n.data.asset.as_deref()), Some(added.asset.id()));
    }

    #[test]
    fn an_asset_the_project_has_already_stands_in_for_its_copy() {
        let mut model = mine();
        let hash = "c".repeat(64);
        assert!(model.add_asset(image(&hash)));
        let file = GroupsFile { groups: Vec::new(), assets: vec![image(&hash)] };
        let result = model.import_groups(&[file]).expect("imports");
        assert!(result.added.is_empty());
        assert_eq!(model.assets().len(), 1);
    }

    #[test]
    fn reports_what_it_cant_find() {
        let result = mine().import_groups(&[file_of(vec![entry("a", sin_document("x"), &["gone"])])]);
        assert_eq!(
            result.map(|r| r.missing),
            Ok(vec!["a depends on gone".to_owned()])
        );
    }

    #[test]
    fn file_stems_lower_case_and_dash_each_run_of_other_characters() {
        assert_eq!(file_stem("My Group (v2)!"), "my-group-v2-");
        assert_eq!(file_stem("Group"), "group");
    }

    #[test]
    fn a_bundle_with_an_altered_file_keeps_the_others() {
        let honest = [1u8, 2, 3];
        let good = image(&asset_hash(&honest));
        let bad = Asset::Image(AssetFile {
            id: "a2".into(),
            name: "Bad".into(),
            file: "assets/images/a2.png".into(),
            mime: "image/png".into(),
            bytes: 3,
            hash: asset_hash(&honest),
            width: 1,
            height: 1,
        });
        let file = GroupsFile { groups: Vec::new(), assets: vec![good.clone(), bad.clone()] };
        let json = serde_json::to_string(&file).expect("serialises");
        let files = [(good.file(), &honest[..]), (bad.file(), &[9u8, 9, 9][..])];
        let bytes = crate::assets::pack_bundle(BundleKind::Groups, &json, files).expect("packs");
        let read = read_groups(&bytes).expect("reads");
        assert_eq!(read.file.assets, [good]);
        assert_eq!(read.altered, [bad]);
    }
}
