//! Placing the nodes of built groups: with the graph crate's estimated node
//! sizes, or with sizes measured as the editor draws them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use shader_graph::graph::{GraphDocument, GraphModel, GraphProject, Point, ROOT_ID};
use shader_graph::layout::LayoutOptions;
use thiserror::Error;

use crate::build::BuiltGroup;

#[derive(Debug, Error)]
pub enum LayoutError {
    #[error("laying out {group}: {source}")]
    Engine {
        group: String,
        source: shader_graph::layout::LayoutError,
    },
    #[error("laying out {0} changed its nodes")]
    Changed(String),
    #[error("the measured layout is missing {0}")]
    Missing(String),
    #[error("{what}: {source}")]
    Io {
        what: String,
        source: std::io::Error,
    },
    #[error("the measured layout can't be read: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the editor failed to lay the groups out: {0}")]
    Editor(String),
}

/// The key a built group has in the projects made here, as a group of the user's.
pub fn alias(group: &BuiltGroup) -> String {
    format!("build-{}", group.spec.id)
}

/// A project holding `groups` as groups of the user's, and `root` as its
/// root document (a fragment output alone by default). The groups they call
/// are the library's own unless among them.
pub fn project_of(groups: &[BuiltGroup], root: Option<GraphDocument>) -> GraphProject {
    let aliases: BTreeMap<&str, String> = groups
        .iter()
        .map(|g| (g.reference.as_str(), alias(g)))
        .collect();
    let mut project = GraphProject::default();
    if let Some(root) = root {
        project.definitions.insert(ROOT_ID.to_owned(), root);
    }
    for group in groups {
        let mut definition = group.definition.clone();
        definition.label = Some(group.spec.name.to_owned());
        for node in &mut definition.nodes {
            if let Some(group_ref) = &mut node.group
                && let Some(alias) = aliases.get(group_ref.definition_ref.as_str())
            {
                group_ref.definition_ref.clone_from(alias);
            }
        }
        project.definitions.insert(alias(group), definition);
    }
    project.reindex();
    project
}

fn take_positions(group: &mut BuiltGroup, laid_out: &GraphDocument) -> Result<(), LayoutError> {
    if laid_out.nodes.len() != group.definition.nodes.len() {
        return Err(LayoutError::Changed(group.spec.id.to_owned()));
    }
    let positions: BTreeMap<&str, Point> = laid_out
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.position))
        .collect();
    for node in &mut group.definition.nodes {
        let p = positions
            .get(node.id.as_str())
            .ok_or_else(|| LayoutError::Changed(group.spec.id.to_owned()))?;
        node.position = Point::new(p.x.round(), p.y.round());
    }
    Ok(())
}

/// Lay out the groups of `groups` named in `ids`, with estimated node sizes.
pub fn layout_headless(groups: &mut [BuiltGroup], ids: &[&str]) -> Result<(), LayoutError> {
    let mut model = GraphModel::new(project_of(groups, None));
    for group in groups.iter_mut().filter(|g| ids.contains(&g.spec.id)) {
        let key = alias(group);
        model
            .layout_document(&key, None, &LayoutOptions::default())
            .map_err(|source| LayoutError::Engine {
                group: group.spec.id.to_owned(),
                source,
            })?;
        let laid_out = model
            .project()
            .document(&key)
            .ok_or_else(|| LayoutError::Missing(key.clone()))?
            .clone();
        take_positions(group, &laid_out)?;
    }
    Ok(())
}

/// How to run the editor: a built binary, or through cargo.
fn editor_command(app: Option<&Path>) -> Command {
    match app {
        Some(app) => Command::new(app),
        None => {
            let mut command = Command::new("cargo");
            command.args(["run", "-p", "webgpu-nodes", "--release", "--"]);
            command
        }
    }
}

/// Lay out the groups named in `ids` as the editor measures its nodes: write
/// them as groups of a temporary project, have the editor lay the project
/// out and read the positions back. `app` is a built editor, else it's run
/// through cargo.
pub fn layout_measured(
    groups: &mut [BuiltGroup],
    ids: &[&str],
    app: Option<&Path>,
) -> Result<(), LayoutError> {
    let io = |what: &str| {
        let what = what.to_owned();
        move |source| LayoutError::Io { what, source }
    };
    let dir = tempfile::tempdir().map_err(io("making a temporary folder"))?;
    let (input, output): (PathBuf, PathBuf) =
        (dir.path().join("in.json"), dir.path().join("out.json"));
    let project = project_of(groups, None);
    std::fs::write(&input, serde_json::to_string(&project)?).map_err(io("writing the project"))?;
    let status = editor_command(app)
        .arg("--layout-project")
        .arg(&input)
        .arg(&output)
        .status()
        .map_err(io("running the editor"))?;
    if !status.success() {
        return Err(LayoutError::Editor(status.to_string()));
    }
    let text = std::fs::read_to_string(&output).map_err(io("reading the laid-out project"))?;
    let laid_out: GraphProject = serde_json::from_str(&text)?;
    for group in groups.iter_mut().filter(|g| ids.contains(&g.spec.id)) {
        let key = alias(group);
        let document = laid_out
            .document(&key)
            .ok_or_else(|| LayoutError::Missing(key.clone()))?;
        take_positions(group, document)?;
    }
    Ok(())
}
