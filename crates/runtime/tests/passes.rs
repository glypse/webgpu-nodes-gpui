//! Multi-pass programs drawn on the real device: textures rendered by passes
//! and simulations kept between frames. Frames are read back sRGB-encoded.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use runtime::{FrameInputs, MediaSource, MediaStatus, PixelFormat, Program, RenderOptions, Renderer, Size};
use shader_graph::assets::{Asset, AssetFile, AssetKind, asset_hash};
use shader_graph::graph::{Connection, FRAG_OUTPUT, GraphModel, Point};
use shader_graph::types::{InputValue, WgslType};
use shader_graph::zones::ZoneKind;

fn srgb(linear: f64) -> u8 {
    let v = if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

struct Scene {
    model: GraphModel,
    renderer: Renderer,
}

impl Scene {
    fn new() -> Self {
        Self { model: GraphModel::default(), renderer: Renderer::new().expect("a GPU") }
    }

    fn add(&mut self, node_type: &str) -> String {
        self.model.add_node(node_type, Point::default()).unwrap_or_else(|| panic!("adds {node_type}"))
    }

    fn constant(&mut self, value: f64) -> String {
        let node = self.add("declare");
        self.model.set_node_type(&node, Some(WgslType::F32));
        self.model.set_input_value(&node, "x", Some(InputValue::from(value)));
        node
    }

    fn link(&mut self, from: (&str, &str), to: (&str, &str)) {
        let c = Connection::new(from.0, from.1, to.0, to.1);
        assert!(self.model.is_valid_connection(&c), "invalid link {c:?}");
        assert!(self.model.connect(c));
    }

    fn output(&self) -> String {
        self.model.root().nodes.iter().find(|n| n.is(FRAG_OUTPUT)).expect("an output").id.clone()
    }

    fn compile(&mut self) -> Program {
        let program = self.model.generate_hoisted().expect("generates");
        let compiled = self
            .renderer
            .compile(&program.plan)
            .unwrap_or_else(|e| panic!("{e}\n{}", program.program.code));
        let values = program
            .uniforms
            .iter()
            .map(|(name, v)| (name.clone(), v.0.iter().map(|c| f64::from(*c)).collect()))
            .collect();
        self.renderer.set_uniforms(&compiled, &values);
        compiled
    }

    fn draw(&mut self, program: &Program, size: Size, frame: u64) -> Vec<u8> {
        let inputs = FrameInputs { time: frame as f64 / 60.0, delta: 1.0 / 60.0, frame };
        let options = RenderOptions { size, format: PixelFormat::Rgba8, premultiply: false };
        self.renderer.render(program, inputs, options).expect("renders").pixels
    }
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// A value drawn into a 4 by 4 Render Texture, sampled back at the pixel.
#[test]
fn a_render_texture_is_drawn_and_sampled() {
    let mut scene = Scene::new();
    let uv = scene.add("uv");
    let texture = scene.add("render-texture");
    scene.model.update_node_data(&texture, |d| {
        d.settings.insert("mode".to_owned(), "pixels".to_owned());
        d.values.insert("pixels".to_owned(), InputValue::vector(&[4.0, 4.0]));
    });
    let color = scene.model.root().node(&texture).unwrap().data.items.as_ref().map(|i| i[0].id.clone());
    let item = color.unwrap_or_else(|| "color".to_owned());
    let sample = scene.add("sample-texture");
    let out = scene.output();
    scene.link((&uv, "uv"), (&texture, &item));
    scene.link((&texture, &item), (&sample, "texture"));
    // Closest: the 4 by 4 texels stay apart.
    scene.model.set_input_value(&sample, "interpolation", Some(InputValue::from(1.0)));
    scene.link((&sample, "color"), (&out, "color"));

    let program = scene.compile();
    assert_eq!(program.plan().passes.len(), 2, "the texture's pass, then the frame's");
    let size = Size::new(8, 8);
    let pixels = scene.draw(&program, size, 0);
    // Left to right, red is u at the centre of each of the four texels.
    let red = |x| pixel(&pixels, 8, x, 4)[0];
    assert_eq!(red(0), red(1), "two pixels per texel");
    assert!(red(0) < red(2) && red(2) < red(4) && red(4) < red(6), "red follows u: {:?}", (0..8).map(red).collect::<Vec<_>>());
    assert_eq!(red(0), srgb(0.125));
    assert_eq!(red(6), srgb(0.875));
    // Top to bottom, green is v, upwards.
    let green = |y| pixel(&pixels, 8, 4, y)[1];
    assert!(green(0) > green(7), "green is v, up");
}

/// A simulation adding one to its state each frame, shown as the red channel.
fn counting_scene() -> Scene {
    let mut scene = Scene::new();
    let input = scene.model.create_zone(ZoneKind::Simulation, Point::default()).expect("zone");
    let item = scene.model.zone_items_of(&input)[0].id.clone();
    let output = scene.model.zone_nodes(&input).expect("nodes").output.id;
    let one = scene.constant(1.0);
    let add = scene.add("add");
    scene.link((&input, &item), (&add, "operands"));
    scene.link((&one, "value"), (&add, "operands"));
    scene.link((&add, "result"), (&output, &item));
    let tenth = scene.constant(0.1);
    let scale = scene.add("mul");
    scene.link((&output, &item), (&scale, "operands"));
    scene.link((&tenth, "value"), (&scale, "operands"));
    let declare = scene.add("declare");
    scene.model.set_node_type(&declare, Some(WgslType::Vec4f));
    scene.link((&scale, "result"), (&declare, "x"));
    let out = scene.output();
    scene.link((&declare, "value"), (&out, "color"));
    scene
}

#[test]
fn a_simulation_steps_a_counter_each_frame() {
    let mut scene = counting_scene();
    let program = scene.compile();
    assert_eq!(program.plan().passes.len(), 3, "start, step, then the frame");
    let size = Size::new(2, 2);
    for frame in 0..5u64 {
        let pixels = scene.draw(&program, size, frame);
        assert_eq!(pixel(&pixels, 2, 1, 1)[0], srgb((frame + 1) as f64 * 0.1), "frame {frame}");
    }
}

#[test]
fn a_simulation_starts_over_when_time_goes_back() {
    let mut scene = counting_scene();
    let program = scene.compile();
    let size = Size::new(2, 2);
    for frame in 0..4u64 {
        scene.draw(&program, size, frame);
    }
    let again = scene.draw(&program, size, 1);
    assert_eq!(pixel(&again, 2, 0, 0)[0], srgb(0.1), "a seek back starts over from the initial value, stepped once");
    let first = scene.draw(&program, size, 0);
    assert_eq!(pixel(&first, 2, 0, 0)[0], srgb(0.1));
}

#[test]
fn drawing_a_frame_again_repeats_its_step() {
    let mut scene = counting_scene();
    let program = scene.compile();
    let size = Size::new(2, 2);
    scene.draw(&program, size, 0);
    let second = scene.draw(&program, size, 1);
    let repeated = scene.draw(&program, size, 1);
    assert_eq!(pixel(&second, 2, 0, 0)[0], pixel(&repeated, 2, 0, 0)[0], "a change while paused doesn't advance the state");
    assert_eq!(pixel(&repeated, 2, 0, 0)[0], srgb(0.2));
}

#[test]
fn each_canvas_size_keeps_its_own_simulation_state() {
    let mut scene = counting_scene();
    let program = scene.compile();
    let (small, large) = (Size::new(2, 2), Size::new(4, 3));
    for frame in 0..4u64 {
        for size in [small, large] {
            let pixels = scene.draw(&program, size, frame);
            assert_eq!(
                pixel(&pixels, size.width, 1, 1)[0],
                srgb((frame + 1) as f64 * 0.1),
                "{size:?}, frame {frame}: drawing at the other size doesn't restart it"
            );
        }
    }
}

#[test]
fn simulation_state_survives_a_recompile_of_the_same_graph() {
    let mut scene = counting_scene();
    let program = scene.compile();
    let size = Size::new(2, 2);
    scene.draw(&program, size, 0);
    scene.draw(&program, size, 1);
    let recompiled = scene.compile();
    let next = scene.draw(&recompiled, size, 2);
    assert_eq!(pixel(&next, 2, 0, 0)[0], srgb(0.3), "the state carries over into the new program");
}

/// A Render Texture of `pixels` square holding the UV of each texel, in `format`.
fn uv_texture(scene: &mut Scene, pixels: f64, format: shader_graph::passes::TextureFormat) -> (String, String) {
    let uv = scene.add("uv");
    let texture = scene.add("render-texture");
    scene.model.update_node_data(&texture, |d| {
        d.settings.insert("mode".to_owned(), "pixels".to_owned());
        d.values.insert("pixels".to_owned(), InputValue::vector(&[pixels, pixels]));
    });
    let item = scene.model.render_items_of(&texture)[0].id.clone();
    scene.model.set_render_item_format(&texture, &item, format);
    scene.link((&uv, "uv"), (&texture, &item));
    (texture, item)
}

#[test]
fn load_texel_reads_the_exact_texel_rows_down() {
    use shader_graph::passes::TextureFormat;
    for format in [TextureFormat::Rgba32Float, TextureFormat::Rgba16Float] {
        let mut scene = Scene::new();
        let (texture, item) = uv_texture(&mut scene, 4.0, format);
        let load = scene.add("load-texel");
        scene.model.set_node_type(&load, Some(WgslType::Vec4f));
        scene.link((&texture, &item), (&load, "texture"));
        scene.model.set_input_value(&load, "texel", Some(InputValue::vector(&[2.0, 1.0])));
        let out = scene.output();
        scene.link((&load, "value"), (&out, "color"));
        let program = scene.compile();
        let pixels = scene.draw(&program, Size::new(2, 2), 0);
        let [red, green, ..] = pixel(&pixels, 2, 0, 0);
        assert_eq!(red, srgb(0.625), "u at the centre of the third texel, {format:?}");
        assert_eq!(green, srgb(0.625), "v counts up from the bottom, the second row from the top is 1 - 1.5 / 4: {format:?}");
    }
}

#[test]
fn integer_textures_keep_their_values_exactly() {
    use shader_graph::passes::TextureFormat;
    let mut scene = Scene::new();
    let texture = scene.add("render-texture");
    let item = scene.model.render_items_of(&texture)[0].id.clone();
    scene.model.set_render_item_type(&texture, &item, WgslType::U32);
    scene.model.set_render_item_format(&texture, &item, TextureFormat::R32Uint);
    let seven = scene.add("declare");
    scene.model.set_node_type(&seven, Some(WgslType::U32));
    scene.model.set_input_value(&seven, "x", Some(InputValue::from(7.0)));
    scene.link((&seven, "value"), (&texture, &item));
    let load = scene.add("load-texel");
    scene.model.set_node_type(&load, Some(WgslType::Vec4u));
    scene.link((&texture, &item), (&load, "texture"));
    let float = scene.add("to-float");
    scene.link((&load, "value"), (&float, "e"));
    let tenth = scene.constant(0.1);
    let scale = scene.add("mul");
    scene.link((&float, "result"), (&scale, "operands"));
    scene.link((&tenth, "value"), (&scale, "operands"));
    let out = scene.output();
    scene.link((&scale, "result"), (&out, "color"));
    let program = scene.compile();
    let pixels = scene.draw(&program, Size::new(2, 2), 0);
    assert_eq!(pixel(&pixels, 2, 1, 1)[0], srgb(0.7), "7 as an integer, a tenth of it as the red");
}

#[test]
fn the_last_mip_level_is_the_mean_of_the_texture() {
    use shader_graph::passes::TextureFormat;
    let mut scene = Scene::new();
    let (texture, item) = uv_texture(&mut scene, 8.0, TextureFormat::Rgba16Float);
    scene.model.update_node_data(&texture, |d| {
        d.settings.insert("mips".to_owned(), "on".to_owned());
    });
    let sample = scene.add("sample-texture");
    scene.link((&texture, &item), (&sample, "texture"));
    scene.model.set_input_value(&sample, "level", Some(InputValue::from(3.0)));
    scene.model.set_input_value(&sample, "interpolation", Some(InputValue::from(1.0)));
    let out = scene.output();
    scene.link((&sample, "color"), (&out, "color"));
    let program = scene.compile();
    let pixels = scene.draw(&program, Size::new(2, 2), 0);
    let [red, green, ..] = pixel(&pixels, 2, 0, 0);
    assert!(red.abs_diff(srgb(0.5)) <= 1 && green.abs_diff(srgb(0.5)) <= 1, "the mean of u and v is a half: {red} {green}");
    let base = {
        scene.model.set_input_value(&sample, "level", Some(InputValue::from(0.0)));
        let program = scene.compile();
        scene.draw(&program, Size::new(8, 8), 0)
    };
    assert!(pixel(&base, 8, 0, 4)[0] < pixel(&base, 8, 7, 4)[0], "level 0 keeps the gradient");
}

#[test]
fn a_simulations_texture_item_feeds_back_frame_to_frame() {
    use shader_graph::types::HandleType;
    let mut scene = Scene::new();
    let sim = scene.model.create_zone(ZoneKind::Simulation, Point::default()).expect("zone");
    let item = scene
        .model
        .add_zone_item(&sim, HandleType::Fixed(WgslType::Texture), Some("Frame"))
        .expect("an item")
        .id;
    let output = scene.model.zone_nodes(&sim).expect("nodes").output.id;
    let previous = scene.add("sample-texture");
    scene.link((&sim, &item), (&previous, "texture"));
    let step = scene.add("declare");
    scene.model.set_node_type(&step, Some(WgslType::Vec4f));
    scene.model.set_input_value(&step, "x", Some(InputValue::from(0.1)));
    let sum = scene.add("add");
    scene.model.set_node_type(&sum, Some(WgslType::Vec4f));
    scene.link((&previous, "color"), (&sum, "operands"));
    scene.link((&step, "value"), (&sum, "operands"));
    scene.link((&sum, "result"), (&output, &item));
    let out = scene.output();
    scene.link((&output, &item), (&out, "color"));
    let program = scene.compile();
    for frame in 0..4u64 {
        let pixels = scene.draw(&program, Size::new(2, 2), frame);
        let red = pixel(&pixels, 2, 1, 0)[0];
        assert!(red.abs_diff(srgb((frame + 1) as f64 * 0.1)) <= 1, "frame {frame}: {red}");
    }
}

const IMPULSE_SIZE: u32 = 9;

fn group_handle(id: &str, label: &str, outputs: bool) -> String {
    let entry = shader_graph::library::entries().iter().find(|e| e.id == id).expect("a library group");
    let handles = if outputs { &entry.definition.outputs } else { &entry.definition.inputs };
    handles.iter().find(|h| h.label == label).map(|h| h.id.clone()).expect("the handle exists")
}

/// The alpha of the image `id` blurs, a white pixel at the centre of a transparent
/// 9 by 9 image, at `radius` pixels, one value per pixel.
fn blurred_alpha(id: &str, radius: f64, values: &[(&str, f64)]) -> (usize, Vec<Vec<f64>>) {
    use shader_graph::types::{CustomSockets, HandleDef};
    let mut scene = Scene::new();
    let uv = scene.add("uv");
    let image = scene.add("custom-wgsl");
    let size = IMPULSE_SIZE;
    scene.model.update_node_data(&image, |d| {
        d.sockets = Some(CustomSockets {
            inputs: vec![HandleDef::new("uv", "uv", WgslType::Vec2f)],
            outputs: vec![HandleDef::new("color", "color", WgslType::Vec4f)],
        });
        d.code = Some(format!(
            "let p = abs(uv - vec2f(0.5));\ncolor = select(vec4f(0.0), vec4f(1.0), all(p < vec2f(0.5 / {size}.0)));"
        ));
    });
    scene.link((&uv, "uv"), (&image, "uv"));
    let texture = scene.add("render-texture");
    scene.model.update_node_data(&texture, |d| {
        d.settings.insert("mode".to_owned(), "pixels".to_owned());
        d.values.insert("pixels".to_owned(), InputValue::vector(&[f64::from(size), f64::from(size)]));
    });
    let item = scene.model.render_items_of(&texture)[0].id.clone();
    scene.link((&image, "color"), (&texture, &item));

    let entry = shader_graph::library::entries().iter().find(|e| e.id == id).expect("a library group");
    let blur = scene.model.add_group_instance(&entry.reference, Point::default()).expect("adds");
    scene.model.set_input_value(&blur, &group_handle(id, "Radius", false), Some(InputValue::from(radius / f64::from(size))));
    for (label, value) in values {
        scene.model.set_input_value(&blur, &group_handle(id, label, false), Some(InputValue::from(*value)));
    }
    scene.link((&texture, &item), (&blur, &group_handle(id, "Texture", false)));
    let out = scene.output();
    scene.link((&blur, &group_handle(id, "Texture", true)), (&out, "color"));
    let program = scene.compile();
    let passes = program.plan().passes.len();
    let pixels = scene.draw(&program, Size::new(size, size), 0);
    let alpha = (0..size)
        .map(|y| (0..size).map(|x| f64::from(pixel(&pixels, size, x, y)[3]) / 255.0).collect())
        .collect();
    (passes, alpha)
}

/// `weight`'s separable kernel around the centre, normalised.
fn kernel(weight: impl Fn(f64) -> f64, reach: i32) -> Vec<Vec<f64>> {
    let centre = (IMPULSE_SIZE as i32 - 1) / 2;
    let sum: f64 = (-reach..=reach).map(|k| weight(f64::from(k))).sum();
    let w = |d: i32| if d.abs() <= reach { weight(f64::from(d)) / sum } else { 0.0 };
    (0..IMPULSE_SIZE as i32).map(|y| (0..IMPULSE_SIZE as i32).map(|x| w(x - centre) * w(y - centre)).collect()).collect()
}

fn assert_close(actual: &[Vec<f64>], expected: &[Vec<f64>]) {
    for (y, row) in actual.iter().enumerate() {
        for (x, value) in row.iter().enumerate() {
            assert!((value - expected[y][x]).abs() < 0.012, "pixel {x}, {y}: {value} against {}", expected[y][x]);
        }
    }
}

#[test]
fn gaussian_blur_weights_taps_by_a_gaussian_of_a_third_of_the_radius() {
    let radius = 3.0_f64;
    let sigma = radius / 3.0;
    let (_, alpha) = blurred_alpha("std-gaussian-blur", radius, &[]);
    let weight = |k: f64| {
        ((-(k * k) / (2.0 * sigma * sigma)).exp() - (-((radius + 1.0).powi(2)) / (2.0 * sigma * sigma)).exp()).max(0.0)
    };
    assert_close(&alpha, &kernel(weight, 3));
}

#[test]
fn box_blur_weights_taps_equally_the_outermost_by_the_radius_fraction() {
    let radius = 1.5_f64;
    let (_, alpha) = blurred_alpha("std-box-blur", radius, &[]);
    assert_close(&alpha, &kernel(|k| (radius + 1.0 - k.abs()).clamp(0.0, 1.0), 2));
}

#[test]
fn a_blur_with_a_direction_blurs_along_the_angle_alone() {
    let centre = (IMPULSE_SIZE as usize - 1) / 2;
    for id in ["std-gaussian-blur", "std-box-blur"] {
        let (_, alpha) = blurred_alpha(id, 2.0, &[("Direction", 1.0), ("Angle", 90.0)]);
        for (y, row) in alpha.iter().enumerate() {
            for (x, value) in row.iter().enumerate() {
                if x != centre {
                    assert!(*value < 0.01, "{id}: pixel {x}, {y} is {value}");
                }
            }
        }
        assert!(alpha[centre - 1][centre] > 0.0, "{id} blurs along the angle");
    }
}

#[test]
fn a_blur_renders_only_the_passes_its_settings_reach() {
    let passes = |id, values: &[(&str, f64)]| blurred_alpha(id, 2.0, values).0;
    // The pass drawing the image, the blur's, and the frame's, which samples the result.
    assert_eq!(passes("std-gaussian-blur", &[]), 4);
    assert_eq!(passes("std-gaussian-blur", &[("Direction", 1.0)]), 3);
    assert_eq!(passes("std-box-blur", &[("Iterations", 3.0)]), 8);
    assert_eq!(passes("std-gaussian-blur", &[("Separable", 0.0)]), 3);
}

/// A simulation adding its step's seconds to its state each frame, scaled to show as red.
fn clock_scene(skip: bool) -> Scene {
    let mut scene = Scene::new();
    let input = scene.model.create_zone(ZoneKind::Simulation, Point::default()).expect("zone");
    let item = scene.model.zone_items_of(&input)[0].id.clone();
    let output = scene.model.zone_nodes(&input).expect("nodes").output.id;
    let sum = scene.add("add");
    scene.link((&input, &item), (&sum, "operands"));
    scene.link((&input, "delta-time"), (&sum, "operands"));
    scene.link((&sum, "result"), (&output, &item));
    scene.model.set_input_value(&output, "skip", Some(InputValue::from(skip)));
    let scale = scene.constant(10.0);
    let scaled = scene.add("mul");
    scene.link((&output, &item), (&scaled, "operands"));
    scene.link((&scale, "value"), (&scaled, "operands"));
    let declare = scene.add("declare");
    scene.model.set_node_type(&declare, Some(WgslType::Vec4f));
    scene.link((&scaled, "result"), (&declare, "x"));
    let out = scene.output();
    scene.link((&declare, "value"), (&out, "color"));
    scene
}

#[test]
fn a_simulation_reads_the_seconds_since_its_previous_step() {
    let mut scene = clock_scene(false);
    let program = scene.compile();
    let size = Size::new(2, 2);
    for frame in 0..4u64 {
        let pixels = scene.draw(&program, size, frame);
        // The first step has no seconds, each other a sixtieth.
        let expected = frame as f64 / 60.0 * 10.0;
        let red = pixel(&pixels, 2, 0, 0)[0];
        assert!(red.abs_diff(srgb(expected)) <= 1, "frame {frame}: {red}");
    }
}

#[test]
fn a_simulation_that_skips_keeps_its_previous_state() {
    let mut scene = clock_scene(true);
    let program = scene.compile();
    let size = Size::new(2, 2);
    for frame in 0..3u64 {
        let pixels = scene.draw(&program, size, frame);
        assert_eq!(pixel(&pixels, 2, 0, 0)[0], 0, "frame {frame} keeps the initial state");
    }
}

#[test]
fn frames_before_the_first_can_be_simulated_without_drawing() {
    let mut scene = counting_scene();
    let program = scene.compile();
    let size = Size::new(2, 2);
    let frames = (0..5u64).map(|frame| FrameInputs { time: frame as f64 / 60.0, delta: 1.0 / 60.0, frame });
    scene.renderer.simulate(&program, frames, size).expect("simulates");
    let pixels = scene.draw(&program, size, 5);
    assert_eq!(pixel(&pixels, 2, 0, 0)[0], srgb(0.6), "the sixth step");
}

/// `time * 0.5` drawn as the red channel: a frame value, written for the pass.
#[test]
fn a_frame_value_is_computed_on_the_cpu_and_read_by_the_shader() {
    let mut scene = Scene::new();
    let time = scene.add("time");
    let half = scene.constant(0.5);
    let scaled = scene.add("mul");
    scene.link((&time, "seconds"), (&scaled, "operands"));
    scene.link((&half, "value"), (&scaled, "operands"));
    let declare = scene.add("declare");
    scene.model.set_node_type(&declare, Some(WgslType::Vec4f));
    scene.link((&scaled, "result"), (&declare, "x"));
    let out = scene.output();
    scene.link((&declare, "value"), (&out, "color"));
    let program = scene.compile();
    assert!(!program.source().code.contains("runtime_frame.time"), "the shader reads the hoisted value");
    for (frame, expected) in [(0, 0.0), (60, 0.5), (90, 0.75)] {
        let pixels = scene.draw(&program, Size::new(2, 2), frame);
        assert_eq!(pixel(&pixels, 2, 0, 0)[0], srgb(expected), "frame {frame}");
    }
}

/// A Render Texture 4 pixels wide plus the frame index, sampled closest: the
/// texels' widths on the canvas show its size.
#[test]
fn a_size_made_of_frame_values_follows_them() {
    let mut scene = Scene::new();
    let uv = scene.add("uv");
    let texture = scene.add("render-texture");
    scene.model.update_node_data(&texture, |d| {
        d.settings.insert("mode".to_owned(), "pixels".to_owned());
    });
    let item = scene.model.render_items_of(&texture)[0].id.clone();
    let time = scene.add("time");
    let four = scene.add("declare");
    scene.model.set_node_type(&four, Some(WgslType::U32));
    scene.model.set_input_value(&four, "x", Some(InputValue::from(4.0)));
    let side = scene.add("add");
    scene.model.set_node_type(&side, Some(WgslType::U32));
    scene.link((&time, "frame"), (&side, "operands"));
    scene.link((&four, "value"), (&side, "operands"));
    scene.link((&side, "result"), (&texture, "pixels"));
    scene.link((&uv, "uv"), (&texture, &item));
    let sample = scene.add("sample-texture");
    scene.model.set_input_value(&sample, "interpolation", Some(InputValue::from(1.0)));
    scene.link((&texture, &item), (&sample, "texture"));
    let out = scene.output();
    scene.link((&sample, "color"), (&out, "color"));
    let program = scene.compile();
    let size = Size::new(12, 12);
    let red = |pixels: &[u8], x| pixel(pixels, 12, x, 6)[0];
    let pixels = scene.draw(&program, size, 0);
    assert_eq!((red(&pixels, 0), red(&pixels, 2)), (red(&pixels, 1), red(&pixels, 1)), "three pixels per texel");
    assert_ne!(red(&pixels, 2), red(&pixels, 3));
    let pixels = scene.draw(&program, size, 2);
    assert_eq!(red(&pixels, 0), red(&pixels, 1), "two pixels per texel");
    assert_ne!(red(&pixels, 1), red(&pixels, 2));
    assert_eq!(red(&pixels, 2), red(&pixels, 3));
}

fn png(width: u32, height: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
    let raw = pixels.iter().flatten().copied().collect();
    let image = image::RgbaImage::from_raw(width, height, raw).expect("pixels fit");
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
    out.into_inner()
}

/// An image asset, read closest at the pixel: left red, right blue.
#[test]
fn an_image_asset_is_decoded_and_sampled() {
    let mut scene = Scene::new();
    let file = png(2, 1, &[[255, 0, 0, 255], [0, 0, 255, 255]]);
    assert!(scene.model.add_asset(Asset::Image(AssetFile {
        id: "pair".into(),
        name: "pair".into(),
        file: "assets/images/pair.png".into(),
        mime: "image/png".into(),
        bytes: file.len() as u64,
        hash: "0".repeat(64),
        width: 2,
        height: 1,
    })));
    let image = scene.add("image");
    scene.model.set_node_asset(&image, Some("pair"));
    let sample = scene.add("sample-texture");
    scene.model.set_input_value(&sample, "interpolation", Some(InputValue::from(1.0)));
    scene.link((&image, "texture"), (&sample, "texture"));
    let out = scene.output();
    scene.link((&sample, "color"), (&out, "color"));
    let program = scene.compile();
    let size = decode_media(&mut scene.renderer, "pair", AssetKind::Image, &file);
    assert_eq!(size, Size::new(2, 1));
    let pixels = scene.draw(&program, Size::new(4, 2), 0);
    assert_eq!(pixel(&pixels, 4, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 4, 3, 1), [0, 0, 255, 255]);
    // Without its file an image reads nothing.
    scene.renderer.sync_media(&BTreeMap::new());
    let pixels = scene.draw(&program, Size::new(4, 2), 0);
    assert_eq!(pixel(&pixels, 4, 0, 0)[3], 0);
}

#[test]
fn a_video_that_cant_be_decoded_fails_when_it_opens() {
    let mut scene = Scene::new();
    let (wake, woken) = mpsc::channel();
    scene.renderer.set_media_waker(move || {
        let _ = wake.send(());
    });
    let sources = BTreeMap::from([("clip".to_owned(), source(AssetKind::Video, b"bytes"))]);
    scene.renderer.sync_media(&sources);
    loop {
        scene.renderer.poll_media();
        match scene.renderer.media_status("clip") {
            MediaStatus::Failed(message) => {
                assert!(message.starts_with("A video can't be used."), "{message}");
                return;
            }
            MediaStatus::Loading => woken.recv_timeout(Duration::from_secs(20)).expect("the video fails"),
            other => panic!("a video that isn't a video: {other:?}"),
        }
    }
}

fn source(kind: AssetKind, bytes: &[u8]) -> MediaSource {
    MediaSource { kind, hash: asset_hash(bytes), bytes: Arc::new(bytes.to_vec()) }
}

/// Gives the renderer an asset's file and waits for its image to decode, returning its size.
fn decode_media(renderer: &mut Renderer, asset: &str, kind: AssetKind, file: &[u8]) -> Size {
    let (wake, woken) = mpsc::channel();
    renderer.set_media_waker(move || {
        let _ = wake.send(());
    });
    renderer.sync_media(&BTreeMap::from([(asset.to_owned(), source(kind, file))]));
    loop {
        renderer.poll_media();
        match renderer.media_status(asset) {
            MediaStatus::Ready(size) => return size,
            MediaStatus::Loading => woken.recv_timeout(Duration::from_secs(20)).expect("the image decodes"),
            other => panic!("the image should decode: {other:?}"),
        }
    }
}
