//! A simulation zone's state: the textures keeping its items between frames.

use super::sources::TextureSource;
use super::textures::{TextureFormat, TextureSize, same_size};
use crate::types::{WgslType, ZoneItem, convert_var, make_type};

/// A simulation zone as computed at `path` (the group nodes it's computed
/// through, from the root), by its input's id: each group instance keeps a
/// state of its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SimulationRef {
    pub path: Vec<String>,
    pub zone: String,
}

impl SimulationRef {
    pub fn id(&self) -> String {
        self.path.iter().chain([&self.zone]).cloned().collect::<Vec<_>>().join("/")
    }
}

/// What a texture item keeps its colours in.
pub const TEXTURE_STATE_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// The format keeping the values of an item of `ty`; a bool's as unsigned integers.
pub fn state_format(ty: WgslType) -> TextureFormat {
    if ty.is_texture() {
        return TEXTURE_STATE_FORMAT;
    }
    TextureFormat::of_scalar(ty.kind(), ty.components())
}

/// The type of a texel of `format`, as WGSL reads and writes it.
pub fn texel_type(format: TextureFormat) -> WgslType {
    make_type(format.scalar(), 4)
}

/// A value of `ty` as the texel of `format` keeping it.
pub fn pack_state(expr: &str, ty: WgslType, format: TextureFormat) -> String {
    convert_var(expr, ty, texel_type(format))
}

/// A texel of `format` as the value of `ty` it keeps.
pub fn unpack_state(texel: &str, ty: WgslType, format: TextureFormat) -> String {
    convert_var(texel, texel_type(format), ty)
}

/// What one pass can write, by WebGPU's default limits.
pub const MAX_COLOR_ATTACHMENTS: usize = 8;
pub const MAX_COLOR_ATTACHMENT_BYTES: u32 = 32;

/// A state item of a simulation, as computed at its instance.
#[derive(Clone, Debug, PartialEq)]
pub struct StateItem {
    pub item: ZoneItem,
    /// Its type there: a texture item's is a texture, its values colours.
    pub ty: WgslType,
    pub format: TextureFormat,
    pub size: TextureSize,
    /// The texture its size is relative to, if it has one: it exists before the item's passes.
    pub like: Option<Box<TextureSource>>,
    /// The id of the texture keeping it (see `state_texture_id`).
    pub texture: String,
}

impl StateItem {
    /// The values of the item: a texture item's are colours.
    pub fn value_type(&self) -> WgslType {
        self.ty.value_type()
    }
}

/// A simulation's state, as computed at its instance.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationState {
    pub reference: SimulationRef,
    pub id: String,
    pub items: Vec<StateItem>,
}

/// The id of the texture keeping a state item of a simulation instance.
pub fn state_texture_id(simulation: &SimulationRef, item: &str) -> String {
    format!("state:{}:{item}", simulation.id())
}

/// Which of a state texture's two frames is read: the previous frame's, or this one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateRead {
    Previous,
    Current,
}

impl StateRead {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Previous => "previous",
            Self::Current => "current",
        }
    }
}

/// The id of the texture resource a state read is: one per state texture and frame.
pub fn state_read_id(texture: &str, read: StateRead) -> String {
    format!("{texture}:{}", read.id())
}

/// Something with a format and, maybe, a size, to be drawn as a colour attachment.
pub trait Attachment {
    fn format(&self) -> TextureFormat;
    fn size(&self) -> Option<&TextureSize>;
}

impl Attachment for StateItem {
    fn format(&self) -> TextureFormat {
        self.format
    }

    fn size(&self) -> Option<&TextureSize> {
        Some(&self.size)
    }
}

/// `items` in order, split into runs each one pass can write as colour
/// attachments: a simulation whose state takes more is computed by several.
/// A pass draws at one size, so items of other sizes are in runs of their own.
pub fn attachment_groups<T: Attachment + Clone>(items: &[T]) -> Vec<Vec<T>> {
    let mut groups: Vec<Vec<T>> = Vec::new();
    let mut current: Vec<T> = Vec::new();
    let mut bytes = 0;
    for item in items {
        let size = item.format().texel_bytes();
        let differs = match (current.first().and_then(Attachment::size), item.size()) {
            (Some(a), Some(b)) => !same_size(a, b),
            _ => false,
        };
        if !current.is_empty()
            && (current.len() == MAX_COLOR_ATTACHMENTS || bytes + size > MAX_COLOR_ATTACHMENT_BYTES || differs)
        {
            groups.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(item.clone());
        bytes += size;
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Item(TextureFormat);

    impl Attachment for Item {
        fn format(&self) -> TextureFormat {
            self.0
        }

        fn size(&self) -> Option<&TextureSize> {
            None
        }
    }

    #[test]
    fn state_formats_follow_the_value() {
        assert_eq!(state_format(WgslType::F32), TextureFormat::R32Float);
        assert_eq!(state_format(WgslType::Vec3f), TextureFormat::Rgba32Float);
        assert_eq!(state_format(WgslType::Vec2i), TextureFormat::Rg32Sint);
        assert_eq!(state_format(WgslType::Bool), TextureFormat::R32Uint);
        assert_eq!(state_format(WgslType::Texture), TextureFormat::Rgba16Float);
    }

    #[test]
    fn states_pack_into_texels() {
        assert_eq!(pack_state("x", WgslType::F32, TextureFormat::R32Float), "vec4f(x)");
        assert_eq!(unpack_state("t", WgslType::Vec2f, TextureFormat::Rg32Float), "t.xy");
        assert_eq!(pack_state("b", WgslType::Bool, TextureFormat::R32Uint), "vec4u(u32(b))");
    }

    #[test]
    fn runs_stay_within_what_a_pass_writes() {
        let items = vec![Item(TextureFormat::Rgba32Float); 3];
        let runs = attachment_groups(&items);
        assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1], "16 bytes each, 32 at most");
        let many = vec![Item(TextureFormat::R32Float); 9];
        assert_eq!(attachment_groups(&many).iter().map(Vec::len).collect::<Vec<_>>(), [8, 1]);
    }
}
