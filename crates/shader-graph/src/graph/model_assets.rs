//! The files a project uses: only their metadata is in the model (and its history).

use std::collections::HashMap;

use super::model::GraphModel;
use crate::assets::{Asset, DEFAULT_PLAYBACK, ImportError, ImportFile, VideoEnd, VideoPlayback, MAX_ASSETS, describe_asset};
use crate::library::is_library_ref;
use crate::nodes::{MEDIA_NODE_TYPES, VIDEO, media_node_of};

/// The asset an imported file is. `reused` when the project already had one
/// with the file's hash: its file is then the project's already.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub asset: Asset,
    pub reused: bool,
}

/// What of a Video node's playback is changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackChange {
    pub start: Option<f64>,
    pub speed: Option<f64>,
    pub end: Option<VideoEnd>,
}

impl GraphModel {
    pub fn assets(&self) -> &[Asset] {
        &self.project.assets
    }

    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.project.assets.iter().find(|a| a.id() == id)
    }

    /// The asset whose file has `hash`, if one does.
    pub fn asset_by_hash(&self, hash: &str) -> Option<&Asset> {
        self.project.assets.iter().find(|a| a.hash() == hash)
    }

    /// Adds an asset, unless its id is taken, it isn't what an import makes
    /// or the project has too many. Returns whether it was added.
    pub fn add_asset(&mut self, asset: Asset) -> bool {
        if self.asset(asset.id()).is_some() || !asset.is_valid() || self.project.assets.len() >= MAX_ASSETS {
            return false;
        }
        self.project.assets.push(asset);
        self.changed();
        true
    }

    /// Adds a file as an asset with a new id, or finds the asset that has its
    /// hash, so importing the same file twice keeps one asset.
    pub fn import_file(&mut self, file: ImportFile) -> Result<Imported, ImportError> {
        let asset = describe_asset(&uuid::Uuid::new_v4().to_string(), file)?;
        if let Some(existing) = self.asset_by_hash(asset.hash()) {
            return Ok(Imported { asset: existing.clone(), reused: true });
        }
        if !self.add_asset(asset.clone()) {
            return Err(ImportError::TooManyAssets);
        }
        Ok(Imported { asset, reused: false })
    }

    pub fn rename_asset(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(asset) = self.project.assets.iter_mut().find(|a| a.id() == id) {
            asset.file_info_mut().name = name.to_owned();
            self.changed();
        }
    }

    /// Give an asset another file of its kind, keeping its id (so what uses it keeps it).
    pub fn replace_asset(&mut self, asset: Asset) {
        if !asset.is_valid() {
            return;
        }
        if let Some(existing) = self.project.assets.iter_mut().find(|a| a.id() == asset.id()) {
            *existing = asset;
            self.changed();
        }
    }

    pub fn remove_asset(&mut self, id: &str) {
        let before = self.project.assets.len();
        self.project.assets.retain(|a| a.id() != id);
        if self.project.assets.len() != before {
            self.changed();
        }
    }

    /// How many nodes of the user's documents read each asset, by asset id
    /// (absent when none).
    pub fn asset_usages(&self) -> HashMap<String, usize> {
        let mut usages = HashMap::new();
        let user_documents = self.project.definitions.iter().filter(|(key, _)| !is_library_ref(key));
        for node in user_documents.flat_map(|(_, doc)| &doc.nodes) {
            if let Some(asset) = &node.data.asset {
                *usages.entry(asset.clone()).or_insert(0) += 1;
            }
        }
        usages
    }

    /// Have an Image or Video node read an asset of its kind (or none, with `None`).
    pub fn set_node_asset(&mut self, node_id: &str, asset_id: Option<&str>) {
        let Some(node) = self.project.resolve_node(node_id) else { return };
        if !MEDIA_NODE_TYPES.contains(node.node_type.as_str()) {
            return;
        }
        let kind_fits = match asset_id {
            None => true,
            Some(id) => self.asset(id).is_some_and(|a| media_node_of(a.kind()) == node.node_type),
        };
        if kind_fits {
            let asset = asset_id.map(str::to_owned);
            self.update_node_data(node_id, |data| data.asset = asset);
        }
    }

    /// Change how a Video node plays its asset.
    pub fn set_video_playback(&mut self, node_id: &str, change: PlaybackChange) {
        let Some(node) = self.project.resolve_node(node_id).filter(|n| n.is(VIDEO)) else { return };
        let current = node.data.playback.unwrap_or(DEFAULT_PLAYBACK);
        let playback = VideoPlayback {
            start: change.start.filter(|n| n.is_finite()).unwrap_or(current.start),
            speed: change.speed.filter(|n| n.is_finite()).unwrap_or(current.speed),
            end: change.end.unwrap_or(current.end),
        };
        self.update_node_data(node_id, |data| data.playback = Some(playback));
    }

    /// Whether a node reads an asset whose file isn't there (`has_file` tells
    /// which are): it reads a transparent image instead.
    pub fn missing_asset(&self, node_id: &str, has_file: impl Fn(&str) -> bool) -> bool {
        let Some(asset) = self.project.resolve_node(node_id).and_then(|n| n.data.asset.as_deref()) else {
            return false;
        };
        self.asset(asset).is_none() || !has_file(asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{ImportError, ImportFile};

    fn png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    #[test]
    fn importing_the_same_bytes_again_reuses_the_asset() {
        let mut model = GraphModel::default();
        let bytes = png();
        let file = ImportFile { name: "Photo.png", mime: "image/png", bytes: &bytes };
        let first = model.import_file(file).expect("imports");
        assert!(!first.reused);
        let second = model.import_file(file).expect("imports");
        assert!(second.reused);
        assert_eq!(second.asset.id(), first.asset.id());
        assert_eq!(model.assets().len(), 1);
    }

    #[test]
    fn a_file_that_cant_be_read_adds_no_asset() {
        let mut model = GraphModel::default();
        let file = ImportFile { name: "notes.txt", mime: "text/plain", bytes: b"hello" };
        assert_eq!(model.import_file(file), Err(ImportError::Unreadable));
        assert!(model.assets().is_empty());
    }
}
