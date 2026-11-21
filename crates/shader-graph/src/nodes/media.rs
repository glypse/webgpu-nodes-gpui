//! Nodes reading a project asset: Image and Video.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::assets::{AssetKind, frame_interval};
use crate::codegen::format_wgsl_literal;
use crate::types::{
    NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef, NodeWidget, WgslType,
};

pub const IMAGE: &str = "image";
pub const VIDEO: &str = "video";

/// The input of a Video node taking the time to show.
pub const VIDEO_TIME: &str = "time";
/// The output of a Video node giving the seconds between its file's frames.
pub const VIDEO_INTERVAL: &str = "interval";
/// The output of a Video node giving its file's length in seconds.
pub const VIDEO_DURATION: &str = "duration";

/// The outputs of a Video node that are its file's, known while compiling.
pub fn is_video_metadata(handle: &str) -> bool {
    handle == VIDEO_DURATION || handle == VIDEO_INTERVAL
}

/// The output of an Image or Video node giving its file's frames.
pub const MEDIA_TEXTURE: &str = "texture";

/// Node types reading a project asset (`NodeData.asset`).
pub static MEDIA_NODE_TYPES: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from([IMAGE, VIDEO]));

/// The node type showing an asset of each kind.
pub const fn media_node_of(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Image => IMAGE,
        AssetKind::Video => VIDEO,
    }
}

fn number(n: f64) -> String {
    format_wgsl_literal(WgslType::F32, Some(&n.into()))
}

/// A file's frames, with its size in pixels.
pub(super) fn image_node() -> NodeDescriptor {
    let mut d = NodeDescriptor::new(IMAGE, "Image", NodeCategory::Texture)
        .aliases(&["Image Texture", "Picture", "Photo", "Bitmap", "Movie File In", "Texture Sample"])
        .static_outputs()
        .outputs(vec![
            NodeOutputDef::new(MEDIA_TEXTURE, "texture", WgslType::Texture),
            NodeOutputDef::new("size", "size", WgslType::Vec2f),
        ])
        .wgsl(|ctx| {
            let texture = ctx.texture(MEDIA_TEXTURE);
            vec![format!("let {} = vec2f(textureDimensions({texture}));", ctx.output_var("size"))]
        });
    d.widget = Some(NodeWidget::Asset(AssetKind::Image));
    d
}

/// A video file's frame at a time, played as its node's `playback` says. The
/// time is the project's unless `time` is linked, which gives the project
/// time to show instead.
pub(super) fn video_node() -> NodeDescriptor {
    let mut time = NodeInputDef::new(VIDEO_TIME, "time", &[WgslType::F32]);
    time.frame = true;
    time.fallback = true;
    let mut d = NodeDescriptor::new(VIDEO, "Video", NodeCategory::Texture)
        .aliases(&["Movie", "Movie File In", "Footage", "Clip", "Video Texture"])
        .static_outputs()
        .inputs(vec![time])
        .outputs(vec![
            NodeOutputDef::new(MEDIA_TEXTURE, "texture", WgslType::Texture),
            NodeOutputDef::new("size", "size", WgslType::Vec2f),
            NodeOutputDef::new(VIDEO_DURATION, "duration", WgslType::F32),
            NodeOutputDef::new(VIDEO_INTERVAL, "frame interval", WgslType::F32),
        ])
        .defaults(NodeData::default())
        .wgsl(|ctx| {
            let asset = ctx.asset();
            let duration = asset.as_ref().and_then(|a| a.video()).map_or(0.0, |v| v.duration);
            let texture = ctx.texture(MEDIA_TEXTURE);
            vec![
                format!("let {} = vec2f(textureDimensions({texture}));", ctx.output_var("size")),
                format!("let {} = {};", ctx.output_var(VIDEO_DURATION), number(duration)),
                format!(
                    "let {} = {};",
                    ctx.output_var(VIDEO_INTERVAL),
                    number(frame_interval(asset.as_ref()))
                ),
            ]
        });
    d.widget = Some(NodeWidget::Asset(AssetKind::Video));
    d
}
