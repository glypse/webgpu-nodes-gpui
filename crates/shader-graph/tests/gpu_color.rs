//! Colour groups computed on the GPU, against the CPU conversions the colour
//! picker uses. Frames are read back sRGB-encoded in 8 bits.

use runtime::{FrameInputs, PixelFormat, RenderOptions, Renderer, Size};
use shader_graph::codegen::ShaderOptions;
use shader_graph::color::{
    ColorMode, ColorSpace, Lch, Rgb, color_to_linear, oklch_to_linear, srgb_encode,
};
use shader_graph::graph::{Connection, FRAG_OUTPUT, GraphModel, GroupWidget, Point};
use shader_graph::library::entries;
use shader_graph::types::InputValue;

const TOLERANCE: f64 = 2.0 / 255.0;

fn entry(id: &str) -> &'static shader_graph::library::LibraryEntry {
    entries()
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("{id} is in the library"))
}

fn handle(id: &str, label: &str, outputs: bool) -> String {
    let definition = &entry(id).definition;
    let handles = if outputs {
        &definition.outputs
    } else {
        &definition.inputs
    };
    handles
        .iter()
        .find(|h| h.label == label)
        .map(|h| h.id.clone())
        .expect("the handle exists")
}

fn render(model: &mut GraphModel, node: &str, output: &str) -> Rgb {
    let out = model
        .root()
        .nodes
        .iter()
        .find(|n| n.is(FRAG_OUTPUT))
        .expect("an output")
        .id
        .clone();
    assert!(model.connect(Connection::new(node, output, &out, "color")));
    let program = model
        .generate_with(&ShaderOptions::default())
        .expect("generates");
    let mut renderer = Renderer::new().expect("a GPU");
    let compiled = renderer.compile(&program.plan).expect("compiles");
    let options = RenderOptions {
        size: Size::new(1, 1),
        format: PixelFormat::Rgba8,
        premultiply: false,
    };
    let frame = renderer
        .render(
            &compiled,
            FrameInputs {
                time: 0.0,
                delta: 0.0,
                frame: 0,
            },
            options,
        )
        .expect("renders");
    let [r, g, b, _] = [0, 1, 2, 3].map(|i| f64::from(frame.pixels[i]) / 255.0);
    Rgb::new(r, g, b)
}

fn expect_close(actual: Rgb, linear: Rgb, what: &str) {
    for (got, want) in [
        (actual.r, linear.r),
        (actual.g, linear.g),
        (actual.b, linear.b),
    ] {
        let want = srgb_encode(want.clamp(0.0, 1.0));
        assert!(
            (got - want).abs() <= TOLERANCE,
            "{what}: {actual:?} vs encoded {want}"
        );
    }
}

fn color_group(model: &mut GraphModel, mode: ColorMode, values: [f64; 3]) -> String {
    let color = entry("std-color");
    let node = model
        .add_group_instance(&color.reference, Point::default())
        .expect("adds");
    let Some(GroupWidget::Color {
        mode: mode_input,
        modes,
    }) = &color.definition.widget
    else {
        panic!("Color has a widget")
    };
    let index = ColorMode::ALL
        .iter()
        .position(|m| *m == mode)
        .expect("a mode");
    model.set_input_value(&node, mode_input, Some(InputValue::from(index as i32)));
    for (id, value) in modes[&index.to_string()].components.iter().zip(values) {
        model.set_input_value(&node, id, Some(value.into()));
    }
    node
}

#[test]
fn the_color_node_computes_each_mode() {
    let output = handle("std-color", "Color", true);
    let cases = [
        (ColorMode::Rgb, [0.2, 0.4, 0.6]),
        (ColorMode::Hsv, [0.6, 0.5, 0.7]),
        (ColorMode::Hsv, [0.0, 0.0, 0.906]),
        (ColorMode::Oklch, [0.7, 0.1, 200.0]),
    ];
    for (mode, values) in cases {
        let mut model = GraphModel::default();
        let node = color_group(&mut model, mode, values);
        let actual = render(&mut model, &node, &output);
        expect_close(
            actual,
            color_to_linear(mode, values),
            &format!("{mode:?} {values:?}"),
        );
    }
}

#[test]
fn gamut_mapping_matches_the_colour_pickers() {
    let input = handle("std-oklch-gamut-map", "OKLch", false);
    let output = handle("std-oklch-gamut-map", "Linear RGB", true);
    for (l, c, h) in [
        (0.55, 0.32, 144.0),
        (0.75, 0.22, 288.0),
        (0.35, 0.12, 72.0),
        (0.92, 0.05, 0.0),
        (0.5, 0.05, 200.0),
    ] {
        let mut model = GraphModel::default();
        let node = model
            .add_group_instance(&entry("std-oklch-gamut-map").reference, Point::default())
            .expect("adds");
        model.set_input_value(&node, &input, Some(InputValue::vector(&[l, c, h])));
        let actual = render(&mut model, &node, &output);
        expect_close(
            actual,
            oklch_to_linear(Lch { l, c, h }, ColorSpace::Srgb),
            &format!("{l} {c} {h}"),
        );
    }
}

mod ramp {
    use std::collections::BTreeMap;

    use shader_graph::nodes::{COLOR_RAMP, RAMP_FACTOR, RAMP_HUE, RAMP_INTERPOLATION, RAMP_SPACE};
    use shader_graph::ramp::{
        HUE_LONGER, HUE_SHORTER, RAMP_B_SPLINE, RAMP_CARDINAL, RAMP_CONSTANT, RAMP_EASE, RAMP_HSV,
        RAMP_LINEAR, RAMP_OKLCH, RAMP_RGB, RAMP_SMOOTH, RampColorStop, RampModes, RampStop,
        evaluate_ramp,
    };

    use super::*;

    const COLORS: [[f64; 4]; 3] = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    ];
    const POSITIONS: [f64; 3] = [0.0, 0.3, 1.0];

    fn ramp_on_gpu(modes: RampModes, factor: f64) -> Rgb {
        let mut model = GraphModel::default();
        let node = model.add_node(COLOR_RAMP, Point::default()).expect("adds");
        let stops: Vec<RampStop> = POSITIONS
            .iter()
            .enumerate()
            .map(|(i, p)| RampStop {
                id: format!("stop-{i}"),
                position: *p,
            })
            .collect();
        let colors: BTreeMap<String, InputValue> = COLORS
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("stop-{i}"), InputValue::vector(c)))
            .collect();
        model.set_color_ramp(&node, &stops, &colors);
        for (input, value) in [
            (RAMP_FACTOR, factor),
            (RAMP_INTERPOLATION, modes.interpolation as f64),
            (RAMP_SPACE, modes.space as f64),
            (RAMP_HUE, modes.hue as f64),
        ] {
            model.set_input_value(&node, input, Some(value.into()));
        }
        render(&mut model, &node, "color")
    }

    #[test]
    fn the_shader_computes_what_the_cpu_does() {
        let stops: Vec<RampColorStop> = POSITIONS
            .iter()
            .zip(COLORS)
            .map(|(position, color)| RampColorStop {
                position: *position,
                color,
            })
            .collect();
        let modes = [
            RampModes {
                interpolation: RAMP_LINEAR,
                space: RAMP_RGB,
                hue: HUE_SHORTER,
            },
            RampModes {
                interpolation: RAMP_EASE,
                space: RAMP_HSV,
                hue: HUE_SHORTER,
            },
            RampModes {
                interpolation: RAMP_SMOOTH,
                space: RAMP_OKLCH,
                hue: HUE_LONGER,
            },
            RampModes {
                interpolation: RAMP_CARDINAL,
                space: RAMP_RGB,
                hue: HUE_SHORTER,
            },
            RampModes {
                interpolation: RAMP_B_SPLINE,
                space: RAMP_OKLCH,
                hue: HUE_SHORTER,
            },
            RampModes {
                interpolation: RAMP_CONSTANT,
                space: RAMP_HSV,
                hue: HUE_SHORTER,
            },
        ];
        for modes in modes {
            for factor in [0.0, 0.15, 0.3, 0.55, 0.9, 1.0] {
                let [r, g, b, _] = evaluate_ramp(&stops, factor, modes);
                expect_close(
                    ramp_on_gpu(modes, factor),
                    Rgb::new(r, g, b),
                    &format!("{modes:?} at {factor}"),
                );
            }
        }
    }
}
