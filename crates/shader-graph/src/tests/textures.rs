//! Reading textures of any format and size: Sample Texture, Texture Size
//! and Load Texel.

use crate::passes::TextureFormat;
use crate::testing::{TestGraph, validate_wgsl};
use crate::types::{InputValue, WgslType};

/// A Render Texture item of `format` drawing a UV, at `scale` of the canvas.
fn texture_of(g: &mut TestGraph, format: TextureFormat, scale: f64) -> (String, String) {
    let uv = g.add("uv");
    let texture = g.add("render-texture");
    let item = g.model.render_items_of(&texture)[0].id.clone();
    g.model.set_render_item_format(&texture, &item, format);
    g.set(&texture, "scale", scale);
    g.link((&uv, "uv"), (&texture, &item));
    (texture, item)
}

fn frame_code(g: &TestGraph) -> String {
    let plan = g.literal_program().plan;
    for pass in &plan.passes {
        validate_wgsl(&pass.program.code);
    }
    plan.passes
        .last()
        .map(|p| p.program.code.clone())
        .unwrap_or_default()
}

fn sampled(format: TextureFormat, scale: f64) -> TestGraph {
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, format, scale);
    let out = g.output();
    g.link((&texture, &item), (&out, "color"));
    g
}

#[test]
fn textures_are_read_exactly_when_made_for_the_passes_size() {
    let code = frame_code(&sampled(TextureFormat::Rgba16Float, 1.0));
    assert!(
        code.contains("textureLoad(runtime_texture_0, vec2i(runtime_fragment.position), 0)"),
        "{code}"
    );
}

#[test]
fn textures_are_sampled_when_made_for_another_size() {
    let code = frame_code(&sampled(TextureFormat::Rgba16Float, 0.5));
    assert!(
        code.contains("textureSampleLevel(runtime_texture_0, runtime_sampler_linear_clamp"),
        "{code}"
    );
}

#[test]
fn thirty_two_bit_floats_are_filtered_by_hand() {
    let code = frame_code(&sampled(TextureFormat::Rgba32Float, 0.5));
    assert!(code.contains("fn texture_bilinear"), "{code}");
    assert!(!code.contains("textureSampleLevel"), "{code}");
}

#[test]
fn integers_are_read_at_the_nearest_texel() {
    for format in [
        TextureFormat::Rgba32Uint,
        TextureFormat::R32Sint,
        TextureFormat::Rg32Uint,
    ] {
        let code = frame_code(&sampled(format, 0.5));
        assert!(code.contains("vec4f(textureLoad("), "{format:?}: {code}");
        assert!(!code.contains("texture_bilinear"), "{code}");
    }
}

#[test]
fn every_format_is_read_by_sample_texture_in_every_mode() {
    for format in TextureFormat::ALL {
        for interpolation in [None, Some(0.0), Some(1.0)] {
            for extension in [None, Some(0.0), Some(1.0), Some(2.0), Some(3.0)] {
                let mut g = TestGraph::new();
                let (texture, item) = texture_of(&mut g, format, 1.0);
                let sample = g.add("sample-texture");
                let out = g.output();
                g.link((&texture, &item), (&sample, "texture"));
                g.link((&sample, "color"), (&out, "color"));
                // A mode left to a linked input is only known per pixel.
                for (input, mode) in [("interpolation", interpolation), ("extension", extension)] {
                    match mode {
                        Some(value) => {
                            g.set(&sample, input, value);
                        }
                        None => {
                            let picked = g.add("declare");
                            g.force_type(&picked, WgslType::U32);
                            g.link((&picked, "value"), (&sample, input));
                        }
                    }
                }
                let result = std::panic::catch_unwind(|| frame_code(&g));
                assert!(
                    result.is_ok(),
                    "{format:?}, interpolation {interpolation:?}, extension {extension:?}"
                );
            }
        }
    }
}

#[test]
fn a_known_extension_is_the_samplers_addressing_where_hardware_samples() {
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, TextureFormat::Rgba16Float, 1.0);
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&texture, &item), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    g.set(&sample, "extension", 1.0);
    let code = frame_code(&g);
    assert!(code.contains("runtime_sampler_linear_repeat"), "{code}");
    assert!(!code.contains("texture_extend"), "{code}");

    // 32-bit floats are read by hand: the coordinates are wrapped before.
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, TextureFormat::Rgba32Float, 1.0);
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&texture, &item), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    g.set(&sample, "extension", 1.0);
    let code = frame_code(&g);
    assert!(
        code.contains("texture_extend") && !code.contains("runtime_sampler"),
        "{code}"
    );
}

#[test]
fn a_transparent_extension_reads_nothing_outside() {
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, TextureFormat::Rgba16Float, 1.0);
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&texture, &item), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    g.set(&sample, "extension", 3.0);
    let code = frame_code(&g);
    assert!(
        code.contains("texture_outside") && code.contains("vec4f(0.0)"),
        "{code}"
    );
}

#[test]
fn texture_size_gives_size_texel_and_levels() {
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, TextureFormat::Rgba16Float, 0.5);
    let size = g.add("texture-size");
    g.link((&texture, &item), (&size, "texture"));
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec2f);
    g.link((&size, "size"), (&sum, "operands"));
    g.link((&size, "texel"), (&sum, "operands"));
    let levels = g.add("to-float");
    g.link((&size, "levels"), (&levels, "e"));
    let total = g.add("add");
    g.force_type(&total, WgslType::Vec3f);
    g.link((&sum, "result"), (&total, "operands"));
    g.link((&levels, "result"), (&total, "operands"));
    let out = g.output();
    g.link((&total, "result"), (&out, "color"));
    let code = frame_code(&g);
    assert!(
        code.contains("textureDimensions(runtime_texture_0, clamp("),
        "{code}"
    );
    assert!(
        code.contains("i32(textureNumLevels(runtime_texture_0))"),
        "{code}"
    );
}

#[test]
fn load_texel_generates_code_that_validates_for_each_type_and_kind_of_texture() {
    for format in TextureFormat::ALL {
        for ty in [WgslType::Vec4f, WgslType::Vec4i, WgslType::Vec4u] {
            let mut g = TestGraph::new();
            let (texture, item) = texture_of(&mut g, format, 1.0);
            let load = g.add("load-texel");
            g.force_type(&load, ty);
            g.link((&texture, &item), (&load, "texture"));
            g.set(&load, "texel", InputValue::vector(&[3.0, 4.0]));
            let to_float = g.add("to-float");
            g.link((&load, "value"), (&to_float, "e"));
            let out = g.output();
            g.link((&to_float, "result"), (&out, "color"));
            let result = std::panic::catch_unwind(|| frame_code(&g));
            assert!(result.is_ok(), "{ty:?} from {format:?}");
        }
    }
}

#[test]
fn load_texel_converts_what_the_texture_holds() {
    let mut g = TestGraph::new();
    let (texture, item) = texture_of(&mut g, TextureFormat::Rg32Sint, 1.0);
    let load = g.add("load-texel");
    g.force_type(&load, WgslType::Vec4f);
    g.link((&texture, &item), (&load, "texture"));
    let out = g.output();
    g.link((&load, "value"), (&out, "color"));
    let code = frame_code(&g);
    assert!(
        code.contains("vec4f(textureLoad(runtime_texture_0"),
        "converted from vec4i: {code}"
    );
}
