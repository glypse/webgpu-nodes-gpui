//! Nodes reading textures: Sample Texture, Texture Size and Load Texel.

use super::builders::{enum_input, known_enum};
use crate::codegen::{FRAGMENT_VAR, ReadOptions, read_texture};
use crate::passes::texel_type;
use crate::types::{
    EnumVariant, NodeCategory, NodeData, NodeDescriptor, NodeInputDef, NodeOutputDef,
    SamplerAddress, SamplerFilter, TypeFamily, WgslCtx, WgslType, convert_var,
};

pub const SAMPLE_TEXTURE: &str = "sample-texture";
pub const TEXTURE_SIZE: &str = "texture-size";
pub const LOAD_TEXEL: &str = "load-texel";

/// Sample Texture's input taking the coordinates to sample at.
pub const SAMPLE_TEXTURE_UV: &str = "uv";
/// Sample Texture's input taking the mip level to read.
pub const SAMPLE_TEXTURE_LEVEL: &str = "level";
/// Texture Size's input taking the mip level it gives the size of.
pub const TEXTURE_SIZE_LEVEL: &str = "level";

pub fn interpolations() -> Vec<EnumVariant> {
    vec![EnumVariant::new(0, "Linear"), EnumVariant::new(1, "Closest")]
}

/// What a texture reads outside of its [0, 1] coordinates, by variant value.
pub fn extensions() -> Vec<EnumVariant> {
    vec![
        EnumVariant::new(0, "Clamp"),
        EnumVariant::new(1, "Repeat"),
        EnumVariant::new(2, "Mirror"),
        EnumVariant::new(3, "Transparent"),
    ]
}

const EXTENSION_REPEAT: i64 = 1;
const EXTENSION_MIRROR: i64 = 2;
const EXTENSION_TRANSPARENT: i64 = 3;

/// Texture coordinates (origin top-left) of a UV (origin bottom-left, y up).
pub fn texture_coordinates(uv: &str) -> String {
    format!("vec2f(({uv}).x, 1.0 - ({uv}).y)")
}

const EXTEND_FN: &str = "texture_extend";
const EXTEND_SOURCE: &str = "fn texture_extend(coords: vec2f, extension: u32) -> vec2f {
	let repeated = fract(coords);
	let mirrored = 1.0 - abs(fract(coords * 0.5) * 2.0 - 1.0);
	return select(select(coords, mirrored, extension == 2u), repeated, extension == 1u);
}";

const OUTSIDE_FN: &str = "texture_outside";
const OUTSIDE_SOURCE: &str = "fn texture_outside(coords: vec2f) -> bool {
	return any(coords < vec2f(0.0)) || any(coords > vec2f(1.0));
}";

fn sampler_filter(interpolation: i64) -> SamplerFilter {
    if interpolation == 1 { SamplerFilter::Nearest } else { SamplerFilter::Linear }
}

fn sampler_address(extension: i64) -> SamplerAddress {
    match extension {
        EXTENSION_REPEAT => SamplerAddress::Repeat,
        EXTENSION_MIRROR => SamplerAddress::Mirror,
        _ => SamplerAddress::Clamp,
    }
}

/// A texture's colour at some coordinates (the pixel's by default), its
/// interpolation and extension being enum inputs so groups can pass theirs
/// on: picked while compiling they cost nothing, per pixel they cost a
/// sample per interpolation.
pub(super) fn sample_texture_node() -> NodeDescriptor {
    let mut uv = NodeInputDef::new(SAMPLE_TEXTURE_UV, "uv", &[WgslType::Vec2f]);
    uv.fallback = true;
    let data = NodeData::with_values([("interpolation", 0.0), ("extension", 0.0), (SAMPLE_TEXTURE_LEVEL, 0.0)]);
    NodeDescriptor::new(SAMPLE_TEXTURE, "Sample Texture", NodeCategory::Texture)
        .aliases(&["Texture Sample", "Image Texture", "texture2D", "Lookup", "Texture Fetch"])
        .inputs(vec![
            NodeInputDef::new("texture", "texture", &[WgslType::Texture]),
            uv,
            enum_input("interpolation", "interpolation", interpolations()),
            enum_input("extension", "extension", extensions()),
            NodeInputDef::new(SAMPLE_TEXTURE_LEVEL, "level", &[WgslType::F32]),
        ])
        .outputs(vec![
            NodeOutputDef::new("color", "color", WgslType::Vec4f),
            NodeOutputDef::new("alpha", "alpha", WgslType::F32),
        ])
        .defaults(data)
        .static_outputs()
        .wgsl(sample_texture_wgsl)
}

fn sample_texture_wgsl(ctx: &mut WgslCtx<'_>) -> Vec<String> {
    let color = ctx.output_var("color");
    let texture = ctx.texture("texture");
    let uv = if ctx.is_present(SAMPLE_TEXTURE_UV) {
        ctx.promote_input(SAMPLE_TEXTURE_UV)
    } else {
        format!("{FRAGMENT_VAR}.uv")
    };
    let interpolation = known_enum(ctx, "interpolation", &interpolations());
    let extension = known_enum(ctx, "extension", &extensions());
    let coords = format!("{color}_coords");
    let mut lines = vec![format!("let {coords} = {};", texture_coordinates(&uv))];

    // Where the texture is sampled by hardware, a known extension is the
    // sampler's addressing; otherwise the coordinates are wrapped before a
    // clamping read.
    let format = ctx.texture_format("texture");
    let hardware = format.is_filterable();
    let wraps = matches!(extension, Some(EXTENSION_REPEAT | EXTENSION_MIRROR));
    let wrapped = extension.is_none() || (!hardware && wraps);
    let address = match extension {
        Some(e) if hardware => sampler_address(e),
        _ => SamplerAddress::Clamp,
    };
    let sampled = if wrapped {
        let extend = ctx.define_fn(EXTEND_FN, EXTEND_SOURCE);
        let mode = match extension {
            Some(e) => format!("{e}u"),
            None => ctx.promote_input("extension"),
        };
        format!("{extend}({coords}, {mode})")
    } else {
        coords.clone()
    };
    let level = ctx.promote_input(SAMPLE_TEXTURE_LEVEL);
    let sample = |ctx: &mut WgslCtx<'_>, filter| {
        let options = ReadOptions { filter, address, level: Some(&level) };
        read_texture(ctx, &texture, format, &sampled, options)
    };
    let raw = match interpolation {
        None => {
            let linear = sample(ctx, SamplerFilter::Linear);
            let nearest = sample(ctx, SamplerFilter::Nearest);
            format!("select({linear}, {nearest}, {} == 1u)", ctx.promote_input("interpolation"))
        }
        Some(i) => sample(ctx, sampler_filter(i)),
    };

    let outside = format!("{}({coords})", ctx.define_fn(OUTSIDE_FN, OUTSIDE_SOURCE));
    let transparent = match extension {
        None => Some(format!(
            "{} == {EXTENSION_TRANSPARENT}u && {outside}",
            ctx.promote_input("extension")
        )),
        Some(EXTENSION_TRANSPARENT) => Some(outside),
        Some(_) => None,
    };
    lines.push(match transparent {
        None => format!("let {color} = {raw};"),
        Some(t) => format!("let {color} = select({raw}, vec4f(0.0), {t});"),
    });
    let alpha = ctx.output_var("alpha");
    lines.push(format!("let {alpha} = {color}.a;"));
    lines
}

/// A texture's size in pixels at a mip level, the step from one of its pixels
/// to the next in UV, and how many levels it has.
pub(super) fn texture_size_node() -> NodeDescriptor {
    let mut level = NodeInputDef::new(TEXTURE_SIZE_LEVEL, "level", &[WgslType::I32]);
    level.frame = true;
    NodeDescriptor::new(TEXTURE_SIZE, "Texture Size", NodeCategory::Texture)
        .aliases(&["Image Size", "Texture Dimensions", "Texel Size", "Resolution"])
        .inputs(vec![NodeInputDef::new("texture", "texture", &[WgslType::Texture]), level])
        .outputs(vec![
            NodeOutputDef::new("size", "size", WgslType::Vec2f),
            NodeOutputDef::new("texel", "texel", WgslType::Vec2f),
            NodeOutputDef::new("levels", "levels", WgslType::I32),
        ])
        .defaults(NodeData::with_values([(TEXTURE_SIZE_LEVEL, 0.0)]))
        .static_outputs()
        .wgsl(|ctx| {
            let size = ctx.output_var("size");
            let texture = ctx.texture("texture");
            let levels = format!("i32(textureNumLevels({texture}))");
            let level = format!("clamp({}, 0, {levels} - 1)", ctx.promote_input(TEXTURE_SIZE_LEVEL));
            vec![
                format!("let {size} = vec2f(textureDimensions({texture}, {level}));"),
                format!("let {} = 1.0 / {size};", ctx.output_var("texel")),
                format!("let {} = {levels};", ctx.output_var("levels")),
            ]
        })
}

/// A texture's exact texel at integer coordinates (rows down, as texture
/// memory is) and mip level, both clamped into the texture: its value as
/// the type picked on the node, converted from what the texture holds.
pub(super) fn load_texel_node() -> NodeDescriptor {
    let mut data = NodeData::default();
    data.values.insert("texel".to_owned(), crate::types::InputValue::vector(&[0.0, 0.0]));
    data.values.insert("level".to_owned(), 0.0.into());
    let mut d = NodeDescriptor::new(LOAD_TEXEL, "Load Texel", NodeCategory::Texture)
        .aliases(&["texelFetch", "textureLoad", "Texel Fetch", "Pixel Read", "Image Load", "Read Pixel"])
        .inputs(vec![
            NodeInputDef::new("texture", "texture", &[WgslType::Texture]),
            NodeInputDef::new("texel", "texel", &[WgslType::Vec2i]),
            NodeInputDef::new("level", "level", &[WgslType::I32]),
        ])
        .outputs(vec![NodeOutputDef::new("value", "value", WgslType::Vec4f)])
        .defaults(data)
        .wgsl(|ctx| {
            let texture = ctx.texture("texture");
            let levels = format!("i32(textureNumLevels({texture}))");
            let level = format!("{}_level", ctx.var_name);
            let coords = format!("{}_texel", ctx.var_name);
            let load = format!("textureLoad({texture}, {coords}, {level})");
            let held = texel_type(ctx.texture_format("texture"));
            let converted = convert_var(&load, held, ctx.output_type);
            vec![
                format!("let {level} = clamp({}, 0, {levels} - 1);", ctx.promote_input("level")),
                format!(
                    "let {coords} = clamp({}, vec2i(0), vec2i(textureDimensions({texture}, {level})) - 1);",
                    ctx.promote_input("texel")
                ),
                format!("let {} = {converted};", ctx.var_name),
            ]
        });
    d.type_family =
        Some(TypeFamily { key: LOAD_TEXEL.to_owned(), types: vec![WgslType::Vec4f, WgslType::Vec4i, WgslType::Vec4u] });
    d
}

pub(super) fn texture_nodes() -> Vec<NodeDescriptor> {
    vec![sample_texture_node(), texture_size_node(), load_texel_node()]
}
