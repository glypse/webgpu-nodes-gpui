//! Multi-pass programs drawn on the real device: textures rendered by passes
//! and simulations kept between frames. Frames are read back sRGB-encoded.

use runtime::{FrameInputs, PixelFormat, Program, RenderOptions, Renderer, Size};
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
        let program = self.model.generate().expect("generates");
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
