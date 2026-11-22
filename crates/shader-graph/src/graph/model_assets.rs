//! The files a project uses: only their metadata is in the model (and its history).

use std::collections::HashMap;

use super::model::GraphModel;
use super::Point;
use super::limits::MAX_ASSETS;
use crate::assets::{Asset, DEFAULT_PLAYBACK, ImportError, ImportFile, VideoEnd, VideoPlayback, VideoProbe, describe_asset};
use crate::library::is_library_ref;
use crate::nodes::{MEDIA_NODE_TYPES, VIDEO, media_node_of};
use crate::types::NodeData;

/// Vertical distance between the nodes of assets dropped together.
const DROPPED_NODE_SPACING: f32 = 260.0;

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
    pub fn import_file(&mut self, file: ImportFile, probe: &dyn VideoProbe) -> Result<Imported, ImportError> {
        let asset = describe_asset(&uuid::Uuid::new_v4().to_string(), file, probe)?;
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
        self.set_video_playback_step(node_id, change, true);
    }

    /// One step of a scrub: `first` opens its undo step, later steps of the
    /// same gesture fold into it.
    pub fn set_video_playback_step(&mut self, node_id: &str, change: PlaybackChange, first: bool) {
        if self.is_locked() {
            return;
        }
        let Some(node) = self.project.resolve_node(node_id).filter(|n| n.is(VIDEO)) else { return };
        let current = node.data.playback.unwrap_or(DEFAULT_PLAYBACK);
        let playback = VideoPlayback {
            start: change.start.filter(|n| n.is_finite()).unwrap_or(current.start),
            speed: change.speed.filter(|n| n.is_finite()).unwrap_or(current.speed),
            end: change.end.unwrap_or(current.end),
        };
        self.edit_gesture(first, |project| {
            if let Some(key) = project.definition_ref_of(node_id).map(str::to_owned)
                && let Some(n) = project.definitions.get_mut(&key).and_then(|d| d.node_mut(node_id))
            {
                n.data.playback = Some(playback);
            }
        });
    }

    /// Adds an Image or Video node reading an asset, at `position`.
    pub fn add_asset_node(&mut self, asset_id: &str, position: Point) -> Option<String> {
        let asset = self.asset(asset_id)?;
        let data = NodeData { asset: Some(asset_id.to_owned()), ..NodeData::default() };
        self.add_node_with(media_node_of(asset.kind()), position, Some(data))
    }

    /// Assets dropped at `position`: onto an Image or Video node (`onto`), the
    /// first of their kind becomes what it reads; elsewhere each gets a node of
    /// its own, one under the other. Returns the ids of the nodes changed or added.
    pub fn drop_assets(&mut self, asset_ids: &[String], position: Point, onto: Option<&str>) -> Vec<String> {
        let target = onto.and_then(|id| self.active_document().node(id)).map(|n| (n.id.clone(), n.node_type.clone()));
        if let Some((node, node_type)) = target {
            let fitting = asset_ids.iter().find(|id| {
                self.asset(id).is_some_and(|a| media_node_of(a.kind()) == node_type)
            });
            if let Some(asset) = fitting {
                let asset = asset.clone();
                self.set_node_asset(&node, Some(&asset));
                return vec![node];
            }
        }
        self.batched(|model| {
            asset_ids
                .iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    let at = Point::new(position.x, position.y + i as f32 * DROPPED_NODE_SPACING);
                    model.add_asset_node(id, at)
                })
                .collect()
        })
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
    use crate::assets::{ImportError, ImportFile, NoVideos};
    use crate::nodes::IMAGE;

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
        let first = model.import_file(file, &NoVideos).expect("imports");
        assert!(!first.reused);
        let second = model.import_file(file, &NoVideos).expect("imports");
        assert!(second.reused);
        assert_eq!(second.asset.id(), first.asset.id());
        assert_eq!(model.assets().len(), 1);
    }

    #[test]
    fn a_file_that_cant_be_read_adds_no_asset() {
        let mut model = GraphModel::default();
        let file = ImportFile { name: "notes.txt", mime: "text/plain", bytes: b"hello" };
        assert_eq!(model.import_file(file, &NoVideos), Err(ImportError::Unreadable));
        assert!(model.assets().is_empty());
    }

    fn png_of_width(width: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, 1, image::Rgba([9, 9, 9, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn import_width(model: &mut GraphModel, width: u32) -> String {
        let bytes = png_of_width(width);
        let file = ImportFile { name: "Photo.png", mime: "image/png", bytes: &bytes };
        model.import_file(file, &NoVideos).expect("imports").asset.id().to_owned()
    }

    #[test]
    fn dropping_an_asset_on_empty_space_adds_an_image_node_reading_it_there() {
        let mut model = GraphModel::default();
        let asset = import_width(&mut model, 2);
        let added = model.drop_assets(std::slice::from_ref(&asset), Point::new(40.0, 80.0), None);
        let node = model.node(&added[0]).expect("added");
        assert_eq!(added.len(), 1);
        assert_eq!(node.node_type, IMAGE);
        assert_eq!(node.data.asset.as_deref(), Some(asset.as_str()));
        assert_eq!((node.position.x, node.position.y), (40.0, 80.0));
    }

    #[test]
    fn dropped_assets_stack_their_nodes_and_one_undo_takes_them_all_back() {
        let mut model = GraphModel::default();
        let first = import_width(&mut model, 2);
        let second = import_width(&mut model, 3);
        let added = model.drop_assets(&[first, second], Point::new(0.0, 0.0), None);
        let ys: Vec<f32> = added.iter().map(|id| model.node(id).expect("added").position.y).collect();
        assert_eq!(ys, [0.0, DROPPED_NODE_SPACING]);
        assert!(model.undo());
        assert!(model.active_document().nodes.iter().all(|n| n.node_type != IMAGE));
    }

    #[test]
    fn dropping_an_asset_onto_an_image_node_makes_it_what_the_node_reads() {
        let mut model = GraphModel::default();
        let asset = import_width(&mut model, 2);
        let node = model.add_node(IMAGE, Point::new(0.0, 0.0)).expect("adds");
        let before = model.active_document().nodes.len();
        let changed = model.drop_assets(std::slice::from_ref(&asset), Point::new(500.0, 500.0), Some(&node));
        assert_eq!(changed, [node.as_str()]);
        assert_eq!(model.active_document().nodes.len(), before);
        assert_eq!(model.node(&node).expect("kept").data.asset.as_deref(), Some(asset.as_str()));
    }

    #[test]
    fn a_video_playback_scrub_folds_its_steps_into_one_undo_step() {
        let mut model = GraphModel::default();
        let node = model.add_node(VIDEO, Point::new(0.0, 0.0)).expect("adds");
        model.set_video_playback_step(&node, PlaybackChange { speed: Some(0.5), ..Default::default() }, true);
        model.set_video_playback_step(&node, PlaybackChange { speed: Some(0.75), ..Default::default() }, false);
        model.set_video_playback_step(&node, PlaybackChange { speed: Some(2.0), ..Default::default() }, false);
        assert_eq!(model.node(&node).expect("kept").data.playback.map(|p| p.speed), Some(2.0));
        assert!(model.undo());
        assert!(model.node(&node).expect("kept").data.playback.is_none());
    }
}
