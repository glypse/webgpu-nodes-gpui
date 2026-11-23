use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use stdlib::build::{BuiltGroup, build_groups};
use stdlib::{files, groups, layout, validate};

/// Builds the standard library's groups from their descriptions in Rust into the library's JSON files.
///
/// Every described group is built, but only the ones whose file no longer
/// matches its description (and so the groups calling them), the groups named
/// and, with --all, every group are written.
#[derive(Debug, Parser)]
#[command(name = "stdlib")]
struct Cli {
    /// Groups to build whether or not their file drifted.
    ids: Vec<String>,

    /// Build every described group.
    #[arg(long)]
    all: bool,

    /// Validate the WGSL and report what would change, writing nothing.
    #[arg(long)]
    dry: bool,

    /// Lay the groups out with node sizes measured by the editor
    /// (`--layout-project`), not estimated.
    #[arg(long)]
    measured: bool,

    /// The built editor to measure with; without it the editor is run through cargo.
    #[arg(long, value_name = "PATH", requires = "measured")]
    app: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let specs = groups::all();
    let mut built = build_groups(&specs)?;

    let unknown: Vec<&String> = cli
        .ids
        .iter()
        .filter(|id| !built.iter().any(|g| g.spec.id == id.as_str()))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "no group described as {}",
            unknown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut selected: Vec<&str> = Vec::new();
    for group in &built {
        let wanted = cli.all || cli.ids.iter().any(|id| id == group.spec.id);
        if wanted || !files::is_current(group)? {
            selected.push(group.spec.id);
        }
    }
    let latest: BTreeMap<String, String> = built
        .iter()
        .map(|g| (g.spec.id.to_owned(), g.reference.clone()))
        .collect();
    if selected.is_empty() {
        println!("Every library file matches its description: nothing to build.");
        report(&files::update_refs(&latest, &[], cli.dry)?);
        return Ok(());
    }
    for group in built.iter().filter(|g| selected.contains(&g.spec.id)) {
        println!(
            "{}: {} nodes, {} links",
            group.reference,
            group.definition.nodes.len(),
            group.definition.edges.len()
        );
    }

    let problems = validate::validate_groups(&built, &selected);
    if !problems.is_empty() {
        bail!("{}", problems.join("\n\n"));
    }

    layout::layout_headless(&mut built, &selected).context("laying out")?;
    if cli.measured {
        layout::layout_measured(&mut built, &selected, cli.app.as_deref())?;
    }

    let rebuilt: Vec<&BuiltGroup> = built
        .iter()
        .filter(|g| selected.contains(&g.spec.id))
        .collect();
    let mut old = Vec::new();
    for group in &rebuilt {
        old.extend(files::current_ref(group.spec.id)?);
    }
    if !cli.dry {
        for group in &rebuilt {
            files::write_group(group)?;
        }
    }
    report(&files::update_refs(&latest, &old, cli.dry)?);
    Ok(())
}

fn report(rewritten: &files::Rewritten) {
    for (old, new) in &rewritten.renames {
        println!("{old} -> {new}");
    }
    for id in &rewritten.library {
        println!("refs updated in the library file {id}");
    }
    for path in &rewritten.scenes {
        println!("refs updated in {}", path.display());
    }
}
