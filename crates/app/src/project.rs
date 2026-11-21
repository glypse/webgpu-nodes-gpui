use std::fs;
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use shader_graph::graph::GraphProject;
use zip::write::SimpleFileOptions;

/// Name of the project's JSON inside a bundle; any other entry is an asset's.
const BUNDLE_JSON: &str = "project.json";
pub const EXTENSION: &str = "wgnodes";
const ZIP_MAGIC: &[u8] = b"PK";
/// What a bundle's JSON may inflate to, so a crafted file can't exhaust memory.
const MAX_JSON_BYTES: u64 = shader_graph::graph::MAX_JSON_BYTES as u64;

/// Reads a project from a bundle (zip with `project.json`) or from bare JSON.
pub fn read(path: &Path) -> Result<GraphProject> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<GraphProject> {
    let json = if bytes.starts_with(ZIP_MAGIC) {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| anyhow!("it isn't a readable bundle: {e}"))?;
        let entry = archive
            .by_name(BUNDLE_JSON)
            .map_err(|_| anyhow!("the bundle has no {BUNDLE_JSON}"))?;
        let mut json = String::new();
        entry
            .take(MAX_JSON_BYTES + 1)
            .read_to_string(&mut json)
            .map_err(|e| anyhow!("{BUNDLE_JSON} can't be read: {e}"))?;
        json
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| anyhow!("it isn't text or a bundle"))?
    };
    Ok(GraphProject::from_json(&json)?)
}

fn encode(project: &GraphProject) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file(BUNDLE_JSON, SimpleFileOptions::default())?;
    writer.write_all(project.to_json().as_bytes())?;
    Ok(writer.finish()?.into_inner())
}

/// Writes the project as a bundle, replacing the file only once it is whole.
pub fn write(path: &Path, project: &GraphProject) -> Result<()> {
    let bytes = encode(project)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

fn last_project_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("webgpu-nodes").join("last-project"))
}

/// Remembers which project to reopen on the next start; `None` forgets it.
pub fn remember(path: Option<&Path>) {
    let Some(file) = last_project_file() else { return };
    let result = match path {
        Some(path) => file
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&file, path.to_string_lossy().as_bytes())),
        None => match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    };
    if let Err(e) = result {
        log::warn!("couldn't remember the last project: {e}");
    }
}

pub fn last() -> Option<PathBuf> {
    let text = fs::read_to_string(last_project_file()?).ok()?;
    let path = PathBuf::from(text.trim());
    path.is_file().then_some(path)
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wgnodes-test-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_saved_project_reads_back_the_same() {
        let dir = scratch("roundtrip");
        let path = dir.join("a.wgnodes");
        let project = GraphProject::default();
        write(&path, &project).expect("write");
        assert_eq!(read(&path).expect("read"), project);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bare_json_reads_too() {
        let project = GraphProject::default();
        assert_eq!(parse(project.to_json().as_bytes()).expect("parse"), project);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse(b"not a project").is_err());
        assert!(parse(b"{}").is_err());
        assert!(parse(b"PK garbage").is_err());
        assert!(parse(&[0xff, 0xfe, 0x00]).is_err());
        assert!(read(Path::new("/definitely/not/here.wgnodes")).is_err());
    }

    #[test]
    fn a_bundle_without_its_json_is_an_error() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("assets/x.png", SimpleFileOptions::default()).expect("start");
        writer.write_all(b"x").expect("write");
        let bytes = writer.finish().expect("finish").into_inner();
        assert!(parse(&bytes).is_err());
    }
}
