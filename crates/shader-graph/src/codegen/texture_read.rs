//! WGSL reading a texture as its format allows.

use crate::passes::TextureFormat;
use crate::types::{SamplerAddress, SamplerFilter, ScalarKind, WgslCtx};

pub const BILINEAR_FN: &str = "texture_bilinear";

/// Linear filtering by hand, for 32-bit floats, which hardware filtering can't read.
pub const BILINEAR_SOURCE: &str = "fn texture_bilinear(t: texture_2d<f32>, coords: vec2f, level: i32) -> vec4f {
	let size = vec2f(textureDimensions(t, level));
	let p = coords * size - 0.5;
	let base = vec2i(floor(p));
	let f = p - floor(p);
	let hi = vec2i(size) - 1;
	let i0 = clamp(base, vec2i(0), hi);
	let i1 = clamp(base + 1, vec2i(0), hi);
	let a = textureLoad(t, i0, level);
	let b = textureLoad(t, vec2i(i1.x, i0.y), level);
	let c = textureLoad(t, vec2i(i0.x, i1.y), level);
	let d = textureLoad(t, i1, level);
	return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}";

/// What reading a texture asks of the shader around it.
pub trait ReadEnv {
    /// The module-scope sampler filtering and addressing as asked.
    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String;
    /// Declares a function once however many ask for it by `name`; returns `name`.
    fn define(&mut self, name: &str, source: &str) -> String;
}

impl ReadEnv for WgslCtx<'_> {
    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String {
        WgslCtx::sampler(self, filter, address)
    }

    fn define(&mut self, name: &str, source: &str) -> String {
        self.define_fn(name, source)
    }
}

/// A `textureLoad` of a texture of `format` as a `vec4f`.
pub fn texel_as_color(load: &str, format: TextureFormat) -> String {
    if format.scalar() == ScalarKind::F32 { load.to_owned() } else { format!("vec4f({load})") }
}

#[derive(Clone, Copy, Debug)]
pub struct ReadOptions<'a> {
    pub filter: SamplerFilter,
    pub address: SamplerAddress,
    /// The mip level, an f32 expression: the base level without.
    pub level: Option<&'a str>,
}

impl Default for ReadOptions<'_> {
    fn default() -> Self {
        Self { filter: SamplerFilter::Linear, address: SamplerAddress::Clamp, level: None }
    }
}

/// A texture's value at `coords` (origin top-left, 0 to 1): sampled where
/// the format filters, else by hand: 32-bit floats bilinearly, integers
/// (which can't be interpolated) at the nearest texel. Out of range
/// coordinates are clamped, so a wrapping address is the caller's to apply
/// to the coordinates when the format can't be sampled.
pub fn read_texture(
    env: &mut dyn ReadEnv,
    texture: &str,
    format: TextureFormat,
    coords: &str,
    options: ReadOptions<'_>,
) -> String {
    if format.is_filterable() {
        let sampler = env.sampler(options.filter, options.address);
        return format!(
            "textureSampleLevel({texture}, {sampler}, {coords}, {})",
            options.level.unwrap_or("0.0")
        );
    }
    // Texel reads take whole levels, which have to exist.
    let whole = options.level.map_or_else(
        || "0".to_owned(),
        |level| format!("clamp(i32(round({level})), 0, i32(textureNumLevels({texture})) - 1)"),
    );
    if format.scalar() == ScalarKind::F32 && options.filter == SamplerFilter::Linear {
        let bilinear = env.define(BILINEAR_FN, BILINEAR_SOURCE);
        return format!("{bilinear}({texture}, {coords}, {whole})");
    }
    let size = format!("vec2f(textureDimensions({texture}, {whole}))");
    let load = format!(
        "textureLoad({texture}, vec2i(clamp(({coords}) * {size}, vec2f(0.0), {size} - 1.0)), {whole})"
    );
    texel_as_color(&load, format)
}
