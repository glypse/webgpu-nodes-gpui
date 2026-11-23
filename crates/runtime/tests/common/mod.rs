use std::sync::Arc;

use runtime::MediaSource;
use shader_graph::assets::{AssetKind, asset_hash};

/// An asset's file as the renderer is given it.
pub fn source(kind: AssetKind, bytes: &[u8]) -> MediaSource {
    MediaSource {
        kind,
        hash: asset_hash(bytes),
        bytes: Arc::new(bytes.to_vec()),
    }
}
