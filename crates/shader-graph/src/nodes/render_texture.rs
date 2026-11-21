//! The Render Texture node: values or textures drawn into textures of a
//! chosen size and format, several at once.

use crate::passes::{DEFAULT_SIZE_PIXELS, TextureFormat};
use crate::types::{
    HandleType, InputValue, NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef,
    NodeSetting, NodeWidget, WgslType, ZoneItem,
};

pub const RENDER_TEXTURE: &str = "render-texture";

/// The input of the scale a texture is relative to its base.
pub const RENDER_SCALE: &str = "scale";
/// The input of the size in pixels.
pub const RENDER_PIXELS: &str = "pixels";
/// The input of the texture a scale is relative to.
pub const RENDER_LIKE: &str = "like";

/// The format an item has unless it's given one.
pub const DEFAULT_RENDER_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

const MODE_KEY: &str = "mode";
const MIPS_KEY: &str = "mips";

pub fn render_mode_setting() -> NodeSetting {
    NodeSetting::new(MODE_KEY, "Size", &[("scale", "Scale"), ("pixels", "Pixels")])
}

pub fn render_mips_setting() -> NodeSetting {
    NodeSetting::new(MIPS_KEY, "Mips", &[("off", "No mips"), ("on", "Mips")])
}

fn default_items() -> Vec<ZoneItem> {
    let mut color = ZoneItem::new("color", "Color", HandleType::Fixed(WgslType::Vec4f));
    color.format = Some(DEFAULT_RENDER_FORMAT.id().to_owned());
    vec![color]
}

pub fn render_texture_items(data: &NodeData) -> Vec<ZoneItem> {
    data.items.clone().unwrap_or_else(default_items)
}

/// What an item's input takes: a texture item's values are colours.
pub fn render_item_type(item: &ZoneItem) -> WgslType {
    item.ty.fixed().map_or(WgslType::Vec4f, WgslType::value_type)
}

pub fn render_item_format(item: &ZoneItem) -> TextureFormat {
    item.format.as_deref().and_then(TextureFormat::from_id).unwrap_or(DEFAULT_RENDER_FORMAT)
}

/// Whether a Render Texture draws its size from pixels rather than a scale.
pub fn renders_in_pixels(data: &NodeData) -> bool {
    render_mode_setting().value(data) == "pixels"
}

/// Whether a Render Texture keeps all the mip levels of its textures.
pub fn renders_mips(data: &NodeData) -> bool {
    render_mips_setting().value(data) == "on"
}

/// Why a Render Texture can't have the mips it asks for, if it can't.
pub fn mips_problem(data: &NodeData) -> Option<String> {
    if !renders_mips(data) {
        return None;
    }
    let item = render_texture_items(data).into_iter().find(|i| !render_item_format(i).can_mip())?;
    Some(format!("\"{}\" is kept in integers, which can't be averaged into mip levels.", item.label))
}

/// A node drawing values or textures into textures of a chosen size and
/// format, several at once. It emits no code: its outputs are drawn by passes
/// of their own.
pub fn build_render_texture_descriptor(data: &NodeData) -> NodeDescriptor {
    let items = render_texture_items(data);
    let mut size = if renders_in_pixels(data) {
        NodeInputDef::new(RENDER_PIXELS, "pixels", &[WgslType::Vec2u])
    } else {
        NodeInputDef::new(RENDER_SCALE, "scale", &[WgslType::F32])
    };
    size.frame = true;
    let mut inputs = vec![size, NodeInputDef::new(RENDER_LIKE, "like", &[WgslType::Texture])];
    inputs.extend(
        items.iter().map(|i| NodeInputDef::new(i.id.clone(), i.label.clone(), &[render_item_type(i)])),
    );
    let outputs = items
        .iter()
        .map(|i| NodeOutputDef::new(i.id.clone(), i.label.clone(), WgslType::Texture))
        .collect();
    let mut defaults = NodeData::with_values([(RENDER_SCALE, 1.0)]);
    defaults.values.insert(
        RENDER_PIXELS.to_owned(),
        InputValue::vector(&[DEFAULT_SIZE_PIXELS, DEFAULT_SIZE_PIXELS]),
    );
    defaults.items = Some(default_items());
    defaults = defaults.setting(MODE_KEY, "scale").setting(MIPS_KEY, "off");
    let mut descriptor = NodeDescriptor::new(RENDER_TEXTURE, "Render Texture", NodeCategory::Texture)
        .aliases(&[
            "Render Target",
            "Framebuffer",
            "Buffer",
            "Resize",
            "Resolution",
            "Downscale",
            "Pixelate",
            "Render Pass",
            "Feedback",
        ])
        .inputs(inputs)
        .outputs(outputs)
        .settings(vec![render_mode_setting(), render_mips_setting()])
        .defaults(defaults)
        .static_outputs();
    descriptor.widget = Some(NodeWidget::Items);
    descriptor
}
