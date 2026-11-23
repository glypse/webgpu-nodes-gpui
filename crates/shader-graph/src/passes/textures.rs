//! Texture formats and sizes: what a pass draws into and what shaders read.

use crate::frame::FrameRef;
use crate::types::ScalarKind;

/// The formats a texture of a program can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextureFormat {
    Rgba16Float,
    Rg16Float,
    R16Float,
    Rgba32Float,
    Rg32Float,
    R32Float,
    Rgba8Unorm,
    Rgba32Uint,
    Rg32Uint,
    R32Uint,
    Rgba32Sint,
    Rg32Sint,
    R32Sint,
}

impl TextureFormat {
    pub const ALL: [TextureFormat; 13] = [
        Self::Rgba16Float,
        Self::Rg16Float,
        Self::R16Float,
        Self::Rgba32Float,
        Self::Rg32Float,
        Self::R32Float,
        Self::Rgba8Unorm,
        Self::Rgba32Uint,
        Self::Rg32Uint,
        Self::R32Uint,
        Self::Rgba32Sint,
        Self::Rg32Sint,
        Self::R32Sint,
    ];

    /// The format's identifier, as saved files and WebGPU spell it.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Rgba16Float => "rgba16float",
            Self::Rg16Float => "rg16float",
            Self::R16Float => "r16float",
            Self::Rgba32Float => "rgba32float",
            Self::Rg32Float => "rg32float",
            Self::R32Float => "r32float",
            Self::Rgba8Unorm => "rgba8unorm",
            Self::Rgba32Uint => "rgba32uint",
            Self::Rg32Uint => "rg32uint",
            Self::R32Uint => "r32uint",
            Self::Rgba32Sint => "rgba32sint",
            Self::Rg32Sint => "rg32sint",
            Self::R32Sint => "r32sint",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }

    /// The format with `channels` components (1, 2 or more for four) of `scalar`'s kind, 32 bits each.
    pub const fn of_scalar(scalar: ScalarKind, channels: usize) -> Self {
        match (scalar, channels) {
            (ScalarKind::F32, 1) => Self::R32Float,
            (ScalarKind::F32, 2) => Self::Rg32Float,
            (ScalarKind::F32, _) => Self::Rgba32Float,
            (ScalarKind::I32, 1) => Self::R32Sint,
            (ScalarKind::I32, 2) => Self::Rg32Sint,
            (ScalarKind::I32, _) => Self::Rgba32Sint,
            (_, 1) => Self::R32Uint,
            (_, 2) => Self::Rg32Uint,
            (_, _) => Self::Rgba32Uint,
        }
    }

    /// The scalar a format's texels are read and written as.
    pub const fn scalar(self) -> ScalarKind {
        match self {
            Self::Rgba32Sint | Self::Rg32Sint | Self::R32Sint => ScalarKind::I32,
            Self::Rgba32Uint | Self::Rg32Uint | Self::R32Uint => ScalarKind::U32,
            _ => ScalarKind::F32,
        }
    }

    /// How many components its texels have.
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgba16Float
            | Self::Rgba32Float
            | Self::Rgba8Unorm
            | Self::Rgba32Uint
            | Self::Rgba32Sint => 4,
            Self::Rg16Float | Self::Rg32Float | Self::Rg32Uint | Self::Rg32Sint => 2,
            _ => 1,
        }
    }

    /// The bytes a texel takes: among a pass's colour attachments, and in memory.
    pub const fn texel_bytes(self) -> u32 {
        let bytes = match self {
            Self::Rgba16Float | Self::Rg16Float | Self::R16Float => 2,
            Self::Rgba8Unorm => 1,
            _ => 4,
        };
        self.channels() as u32 * bytes
    }

    /// Whether linear filtering reads it. 32-bit floats aren't without the
    /// optional `float32-filterable` feature, which codegen can't count on.
    pub const fn is_filterable(self) -> bool {
        matches!(
            self,
            Self::Rgba16Float | Self::Rg16Float | Self::R16Float | Self::Rgba8Unorm
        )
    }

    pub const fn sample_type(self) -> TextureSampleType {
        match self.scalar() {
            ScalarKind::I32 => TextureSampleType::Sint,
            ScalarKind::U32 => TextureSampleType::Uint,
            _ if self.is_filterable() => TextureSampleType::Float,
            _ => TextureSampleType::UnfilterableFloat,
        }
    }

    /// Whether it can have mip levels: integers can't be averaged.
    pub const fn can_mip(self) -> bool {
        matches!(self.scalar(), ScalarKind::F32)
    }
}

/// What a texture of any format is read as by a shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureSampleType {
    Float,
    UnfilterableFloat,
    Sint,
    Uint,
}

impl TextureSampleType {
    /// The scalar a `texture_2d` of this type is declared over.
    pub const fn texel_scalar(self) -> &'static str {
        match self {
            Self::Sint => "i32",
            Self::Uint => "u32",
            _ => "f32",
        }
    }
}

/// A size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// What a scale is relative to: the canvas or another resource's size.
#[derive(Clone, Debug, PartialEq)]
pub enum SizeBase {
    Canvas,
    Resource(String),
}

/// A frame value as plans carry it: a literal, or a value of the plan's frame program.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameArg {
    Literal(f64),
    Frame(FrameRef),
}

impl From<f64> for FrameArg {
    fn from(n: f64) -> Self {
        Self::Literal(n)
    }
}

/// A size: a scale of the canvas or of another resource, or pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSize {
    Scale { scale: FrameArg, of: SizeBase },
    Pixels { width: FrameArg, height: FrameArg },
}

/// The canvas' own size.
pub const CANVAS_SIZE: TextureSize = TextureSize::Scale {
    scale: FrameArg::Literal(1.0),
    of: SizeBase::Canvas,
};

/// How many levels a full mip chain of a texture of this size has.
pub fn mip_level_count(size: Size) -> u32 {
    size.width.max(size.height).max(1).ilog2() + 1
}

/// Rounds up, ignoring the error of products like `1000 * 0.1`.
fn pixels(value: f64, max: u32) -> u32 {
    let ceiled = (value - 1e-6).ceil();
    if ceiled.is_nan() {
        1
    } else {
        ceiled.clamp(1.0, f64::from(max)) as u32
    }
}

/// What sizes are resolved against.
pub struct SizeEnv<'a> {
    pub canvas: Size,
    /// The current size of a resource, which must already be known.
    pub resource: &'a dyn Fn(&str) -> Option<Size>,
    /// The device's `maxTextureDimension2D`.
    pub max_dimension: u32,
    /// The value of a frame program's output, which sizes made of frame values need.
    pub frame: Option<&'a dyn Fn(&FrameRef) -> f64>,
}

/// The size in pixels: `max(1, ceil(dimension * scale))`, so every texel is
/// covered (a 1001-pixel side has 126 blocks of 8), clamped to the device's
/// limit. A size relative to a resource that isn't known is the canvas'.
pub fn resolve_size(size: &TextureSize, env: &SizeEnv) -> Size {
    let arg = |value: &FrameArg| match (value, env.frame) {
        (FrameArg::Literal(n), _) => *n,
        (FrameArg::Frame(frame), Some(read)) => read(frame),
        // Without the program's values a size is unknown: the smallest.
        (FrameArg::Frame(_), None) => f64::NAN,
    };
    match size {
        TextureSize::Pixels { width, height } => Size::new(
            pixels(arg(width), env.max_dimension),
            pixels(arg(height), env.max_dimension),
        ),
        TextureSize::Scale { scale, of } => {
            let scale = arg(scale);
            let base = match of {
                SizeBase::Canvas => env.canvas,
                SizeBase::Resource(id) => (env.resource)(id).unwrap_or(env.canvas),
            };
            Size::new(
                pixels(f64::from(base.width) * scale, env.max_dimension),
                pixels(f64::from(base.height) * scale, env.max_dimension),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: SizeEnv = SizeEnv {
        canvas: Size::new(1001, 500),
        resource: &|_| None,
        max_dimension: 8192,
        frame: None,
    };

    #[test]
    fn scales_round_up() {
        let half = TextureSize::Scale {
            scale: 0.5.into(),
            of: SizeBase::Canvas,
        };
        assert_eq!(resolve_size(&half, &ENV), Size::new(501, 250));
        let tenth = TextureSize::Scale {
            scale: 0.1.into(),
            of: SizeBase::Canvas,
        };
        assert_eq!(
            resolve_size(
                &tenth,
                &SizeEnv {
                    canvas: Size::new(1000, 10),
                    ..ENV
                }
            ),
            Size::new(100, 1)
        );
    }

    #[test]
    fn sizes_are_clamped_to_the_device() {
        let huge = TextureSize::Pixels {
            width: 100_000.0.into(),
            height: 0.0.into(),
        };
        assert_eq!(resolve_size(&huge, &ENV), Size::new(8192, 1));
    }

    #[test]
    fn scales_follow_resources() {
        let size = TextureSize::Scale {
            scale: 2.0.into(),
            of: SizeBase::Resource("a".into()),
        };
        let env = SizeEnv {
            resource: &|id| (id == "a").then_some(Size::new(10, 20)),
            ..ENV
        };
        assert_eq!(resolve_size(&size, &env), Size::new(20, 40));
    }

    #[test]
    fn mip_chains_reach_one_pixel() {
        assert_eq!(mip_level_count(Size::new(1, 1)), 1);
        assert_eq!(mip_level_count(Size::new(256, 100)), 9);
        assert_eq!(mip_level_count(Size::new(255, 255)), 8);
    }

    #[test]
    fn formats_round_trip_and_know_their_texels() {
        for f in TextureFormat::ALL {
            assert_eq!(TextureFormat::from_id(f.id()), Some(f));
        }
        assert_eq!(TextureFormat::Rgba16Float.texel_bytes(), 8);
        assert_eq!(
            TextureFormat::R32Uint.sample_type(),
            TextureSampleType::Uint
        );
        assert_eq!(
            TextureFormat::Rgba32Float.sample_type(),
            TextureSampleType::UnfilterableFloat
        );
        assert!(!TextureFormat::R32Sint.can_mip());
    }
}
