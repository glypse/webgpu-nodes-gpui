use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read as _, Write as _};

use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zip::write::SimpleFileOptions;

use super::asset::{Asset, is_asset_path};
use crate::graph::{MAX_ASSETS, MAX_BUNDLE_BYTES, MAX_JSON_BYTES};

/// What a bundle holds: a whole project, or groups exported to be imported
/// into others. Either way, its JSON sits beside the files of the assets it uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleKind {
    Project,
    Groups,
}

impl BundleKind {
    /// The kind's JSON inside a bundle; every other file is an asset's.
    pub const fn json_name(self) -> &'static str {
        match self {
            Self::Project => "project.json",
            Self::Groups => "groups.json",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Project => "wgnodes",
            Self::Groups => "wggroups",
        }
    }
}

/// How much more a bundle's assets can weigh than the bundle. We store media
/// as it is, so anything near this was made to inflate; some room is left for
/// bundles zipped again by other tools.
const MAX_ASSET_EXPANSION: u64 = 4;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BundleError {
    #[error("It's too large: {0}.")]
    TooLarge(String),
    #[error("It isn't a readable bundle.")]
    NotABundle,
    #[error("The bundle has no {0}.")]
    MissingJson(&'static str),
    #[error("\"{0}\" isn't an asset's path.")]
    NotAnAssetPath(String),
    #[error("The bundle can't be written: {0}")]
    Write(String),
}

/// A bundle's contents: its JSON, and its assets' files by bundle path.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    pub kind: BundleKind,
    pub json: String,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// SHA-256 of a file's bytes, hex.
pub fn asset_hash(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut hex, b| {
        // Writing to a String can't fail.
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// A bundle of `json` and the files of the assets it uses, by bundle path.
pub fn pack_bundle<'a>(
    kind: BundleKind,
    json: &str,
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<Vec<u8>, BundleError> {
    let write = |e: zip::result::ZipError| BundleError::Write(e.to_string());
    let io = |e: std::io::Error| BundleError::Write(e.to_string());
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file(kind.json_name(), SimpleFileOptions::default()).map_err(write)?;
    writer.write_all(json.as_bytes()).map_err(io)?;
    for (path, data) in files {
        if !is_asset_path(path) {
            return Err(BundleError::NotAnAssetPath(path.to_owned()));
        }
        // Media is compressed already: storing it saves time for nothing lost.
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(path, options).map_err(write)?;
        writer.write_all(data).map_err(io)?;
    }
    Ok(writer.finish().map_err(write)?.into_inner())
}

/// Read a bundle of `kind`, keeping only its JSON and what can be assets'
/// files. Nothing is inflated past the sizes checked here.
pub fn unpack_bundle(bytes: &[u8], kind: BundleKind) -> Result<Bundle, BundleError> {
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge("the bundle".to_owned()));
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| BundleError::NotABundle)?;
    if archive.len() > MAX_ASSETS + 1 {
        return Err(BundleError::TooLarge("too many files".to_owned()));
    }
    let name = kind.json_name();
    let mut names = BTreeSet::new();
    let mut asset_bytes = 0u64;
    let mut json = None;
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| BundleError::NotABundle)?;
        let Ok(entry_name) = entry.name().map(|n| n.into_owned()) else {
            log::warn!("a bundle entry with a name that isn't UTF-8 is left out");
            continue;
        };
        // A name given twice would inflate twice, for only one result.
        if !names.insert(entry_name.clone()) {
            return Err(BundleError::NotABundle);
        }
        let is_json = entry_name == name;
        if !is_json && !is_asset_path(&entry_name) {
            continue;
        }
        let declared = entry.size();
        if is_json {
            if declared > MAX_JSON_BYTES as u64 {
                return Err(BundleError::TooLarge("its JSON".to_owned()));
            }
        } else {
            asset_bytes += declared;
            if asset_bytes > (bytes.len() as u64).saturating_mul(MAX_ASSET_EXPANSION) {
                return Err(BundleError::TooLarge("its files".to_owned()));
            }
        }
        let mut data = Vec::new();
        entry
            .take(declared.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|_| BundleError::NotABundle)?;
        if data.len() as u64 != declared {
            return Err(BundleError::NotABundle);
        }
        if is_json {
            json = Some(String::from_utf8(data).map_err(|_| BundleError::NotABundle)?);
        } else {
            files.insert(entry_name, data);
        }
    }
    let json = json.ok_or(BundleError::MissingJson(name))?;
    Ok(Bundle { kind, json, files })
}

/// The files of a bundle that are what their assets say.
#[derive(Debug, Default)]
pub struct VerifiedFiles {
    /// By asset id.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Assets whose file isn't what it says: a file's hash decides which
    /// asset stands for it, so it can't be taken at the bundle's word.
    pub altered: Vec<Asset>,
}

/// Moves each asset's file out of `bundle` into the result, by asset id.
pub fn verified_files(mut bundle: Bundle, assets: &[Asset]) -> VerifiedFiles {
    let mut verified = VerifiedFiles::default();
    for asset in assets {
        let Some(data) = bundle.files.remove(asset.file()) else { continue };
        if data.len() as u64 == asset.file_info().bytes && asset_hash(&data) == asset.hash() {
            verified.files.insert(asset.id().to_owned(), data);
        } else {
            verified.altered.push(asset.clone());
        }
    }
    verified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetFile, AssetKind, asset_file};

    fn image_of(id: &str, bytes: &[u8]) -> Asset {
        Asset::Image(AssetFile {
            id: id.into(),
            name: id.into(),
            file: asset_file(AssetKind::Image, id, &format!("{id}.png")),
            mime: "image/png".into(),
            bytes: bytes.len() as u64,
            hash: asset_hash(bytes),
            width: 1,
            height: 1,
        })
    }

    fn zipped(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            writer.start_file(*name, SimpleFileOptions::default()).expect("starts");
            writer.write_all(data).expect("writes");
        }
        writer.finish().expect("finishes").into_inner()
    }

    #[test]
    fn hashes_are_64_lowercase_hex_digits() {
        let hash = asset_hash(b"abc");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn a_bundle_round_trips_its_json_and_files() {
        let path = "assets/images/a1.png";
        let bytes = pack_bundle(BundleKind::Project, "{}", [(path, &[1u8, 2, 3][..])]).expect("packs");
        let bundle = unpack_bundle(&bytes, BundleKind::Project).expect("unpacks");
        assert_eq!(bundle.json, "{}");
        assert_eq!(bundle.files.keys().collect::<Vec<_>>(), [path]);
        assert_eq!(bundle.files[path], [1, 2, 3]);
    }

    #[test]
    fn rejects_what_isnt_a_bundle_of_the_kind_asked_for() {
        let groups = pack_bundle(BundleKind::Groups, "{}", []).expect("packs");
        assert_eq!(unpack_bundle(&groups, BundleKind::Project), Err(BundleError::MissingJson("project.json")));
        assert_eq!(unpack_bundle(b"{}", BundleKind::Project), Err(BundleError::NotABundle));
    }

    #[test]
    fn refuses_to_pack_a_path_that_isnt_an_assets() {
        for path in ["project.json", "../evil", "__proto__"] {
            let result = pack_bundle(BundleKind::Project, "{}", [(path, &[0u8][..])]);
            assert!(matches!(result, Err(BundleError::NotAnAssetPath(_))), "{path}");
        }
    }

    #[test]
    fn leaves_out_entries_that_cant_be_assets_files() {
        let bytes = zipped(&[
            ("project.json", b"{}"),
            ("../evil", b"x"),
            ("assets/images/", b""),
            ("assets/images/a.png", b"x"),
        ]);
        let bundle = unpack_bundle(&bytes, BundleKind::Project).expect("unpacks");
        assert_eq!(bundle.files.keys().collect::<Vec<_>>(), ["assets/images/a.png"]);
    }

    #[test]
    fn rejects_more_entries_than_a_project_has_assets() {
        let names: Vec<String> = (0..=MAX_ASSETS + 1).map(|i| format!("x{i}")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("project.json", b"{}")];
        entries.extend(names.iter().map(|n| (n.as_str(), &b""[..])));
        assert_eq!(
            unpack_bundle(&zipped(&entries), BundleKind::Project),
            Err(BundleError::TooLarge("too many files".to_owned()))
        );
    }

    #[test]
    fn keeps_files_that_are_what_their_asset_says_and_no_others() {
        let honest = [1u8, 2, 3];
        let (a, b, short) = (image_of("a", &honest), image_of("b", &honest), image_of("c", &honest));
        let bytes = pack_bundle(
            BundleKind::Project,
            "{}",
            [(a.file(), &honest[..]), (b.file(), &[4, 5, 6][..]), (short.file(), &honest[1..])],
        )
        .expect("packs");
        let bundle = unpack_bundle(&bytes, BundleKind::Project).expect("unpacks");
        let missing = image_of("d", &honest);
        let verified = verified_files(bundle, &[a, b.clone(), short.clone(), missing]);
        assert_eq!(verified.files.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(verified.altered, [b, short]);
    }
}
