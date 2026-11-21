//! How the texture keeping a zone item, or a Render Texture's, is sized.

use super::textures::{SizeBase, TextureSize};
use crate::types::{InputValue, NodeData, NodeInputDef, WgslType, ZoneItem};

/// How an item's texture is sized: unset for the canvas' size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemSizeMode {
    Scale,
    Pixels,
}

impl ItemSizeMode {
    /// The mode as items store it.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Pixels => "pixels",
        }
    }

    /// The mode a stored item `size` names.
    pub fn parse(stored: Option<&str>) -> Option<Self> {
        match stored? {
            "scale" => Some(Self::Scale),
            "pixels" => Some(Self::Pixels),
            _ => None,
        }
    }
}

/// The inputs of the node an item is on that give its size: frame values, and a texture to scale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeHandles {
    pub scale: String,
    pub pixels: String,
    pub like: String,
}

/// Sizes read from inputs a Render Texture has once.
pub fn node_size_handles() -> SizeHandles {
    SizeHandles { scale: "scale".into(), pixels: "pixels".into(), like: "like".into() }
}

pub const DEFAULT_SIZE_PIXELS: f64 = 256.0;

/// The inputs giving the size of the item `id` of a zone's input node.
pub fn item_size_handles(id: &str) -> SizeHandles {
    SizeHandles { scale: format!("scale:{id}"), pixels: format!("pixels:{id}"), like: format!("like:{id}") }
}

/// The size inputs an item has for its mode: none for the canvas' size.
pub fn size_inputs(handles: &SizeHandles, mode: Option<ItemSizeMode>, label: Option<&str>) -> Vec<NodeInputDef> {
    let name = |what: &str| label.map_or_else(|| what.to_owned(), |l| format!("{l} {what}"));
    let frame = |def: NodeInputDef| {
        let mut def = def;
        def.frame = true;
        def
    };
    match mode {
        Some(ItemSizeMode::Scale) => vec![
            frame(NodeInputDef::new(handles.scale.clone(), name("scale"), &[WgslType::F32])),
            NodeInputDef::new(handles.like.clone(), name("like"), &[WgslType::Texture]),
        ],
        Some(ItemSizeMode::Pixels) => {
            vec![frame(NodeInputDef::new(handles.pixels.clone(), name("pixels"), &[WgslType::Vec2u]))]
        }
        None => Vec::new(),
    }
}

/// The ids of every size input the zone items have, whatever their mode.
pub fn item_size_input_ids(items: &[ZoneItem]) -> std::collections::HashSet<String> {
    items
        .iter()
        .flat_map(|item| {
            let mode = ItemSizeMode::parse(item.size.as_deref());
            size_inputs(&item_size_handles(&item.id), mode, None)
        })
        .map(|input| input.id)
        .collect()
}

/// The numbers an item's size inputs hold when nothing is linked to them.
pub fn default_size_values(handles: &SizeHandles, mode: ItemSizeMode) -> Vec<(String, InputValue)> {
    match mode {
        ItemSizeMode::Scale => vec![(handles.scale.clone(), InputValue::from(1.0))],
        ItemSizeMode::Pixels => vec![(
            handles.pixels.clone(),
            InputValue::vector(&[DEFAULT_SIZE_PIXELS, DEFAULT_SIZE_PIXELS]),
        )],
    }
}

/// The size the stored numbers of a node's size inputs give, with `base` for a scale to be relative to.
pub fn stored_size(data: &NodeData, handles: &SizeHandles, mode: ItemSizeMode, base: SizeBase) -> TextureSize {
    let number = |id: &str, component: usize, fallback: f64| {
        data.values.get(id).map_or(fallback, |v| v.component(component))
    };
    match mode {
        ItemSizeMode::Pixels => TextureSize::Pixels {
            width: number(&handles.pixels, 0, DEFAULT_SIZE_PIXELS).into(),
            height: number(&handles.pixels, 1, DEFAULT_SIZE_PIXELS).into(),
        },
        ItemSizeMode::Scale => TextureSize::Scale { scale: number(&handles.scale, 0, 1.0).into(), of: base },
    }
}
