use crate::nodes::registered_nodes;
use crate::testing::TestGraph;
use crate::types::{InputValue, NodeCategory, WgslType};

#[test]
fn an_empty_graph_generates_valid_wgsl() {
    let g = TestGraph::new();
    let wgsl = g.wgsl();
    assert!(wgsl.contains("runtime_uniforms"), "{wgsl}");
}

#[test]
fn uv_into_the_output() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let out = g.output();
    g.link((&uv, "uv"), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(wgsl.contains("vec3f(v_"), "{wgsl}");
}

#[test]
fn unlinked_values_are_uniforms_or_literals() {
    let mut g = TestGraph::new();
    let mul = g.add("mul");
    let sin = g.add("sin");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&mul, "result"), (&sin, "e"));
    g.link((&sin, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    g.set(&declare, "y", 0.5);
    let uniforms = g.program();
    assert!(uniforms.uniforms.iter().any(|(n, v)| n.ends_with("_y") && v.0 == [0.5]));
    assert!(uniforms.program.uniforms.fields.keys().any(|k| k.ends_with("_y")));
    let literal = g.literal_program();
    assert!(literal.uniforms.is_empty());
    assert!(literal.program.code.contains("f32(0.5)"));
    crate::testing::validate_wgsl(&literal.program.code);
    crate::testing::validate_wgsl(&uniforms.program.code);
}

/// Every registered node generates valid WGSL with its defaults, whatever
/// the type it's forced to.
#[test]
fn every_registered_node_generates_valid_wgsl() {
    for descriptor in registered_nodes() {
        if descriptor.category == NodeCategory::Layout || descriptor.type_id == "frag-output" || descriptor.dev {
            continue;
        }
        let types: Vec<Option<WgslType>> = match descriptor.pickable_types() {
            Some(p) => p.types.iter().map(|t| Some(*t)).collect(),
            None => vec![None],
        };
        for ty in types {
            let mut g = TestGraph::new();
            let node = g.add(&descriptor.type_id);
            if let Some(ty) = ty {
                g.force_type(&node, ty);
            }
            let Some(out_def) = descriptor.outputs.first() else { continue };
            let resolved_out = g.model.resolve_node_outputs(&node);
            let handle = resolved_out.first().map_or(out_def.id.clone(), |o| o.id.clone());
            let ty = g.model.resolve_output_handle_type(&node, &handle);
            // Convert whatever it gives to something the output takes.
            let to_color = g.add("declare");
            g.force_type(&to_color, WgslType::Vec4f);
            if ty.components() == 1 {
                g.link((&node, &handle), (&to_color, "x"));
            } else {
                // A vector can only be linked whole to a vector input.
                let out = g.output();
                if ty.components() <= 4 {
                    g.link((&node, &handle), (&out, "color"));
                }
            }
            let out = g.output();
            if !g.model.root().edges.iter().any(|e| e.target == out) {
                g.link((&to_color, "value"), (&out, "color"));
            }
            let code = g.program().program.code;
            let result = std::panic::catch_unwind(|| crate::testing::validate_wgsl(&code));
            assert!(result.is_ok(), "{} as {ty:?} generated invalid WGSL", descriptor.type_id);
        }
    }
}

#[test]
fn variadic_add_joins_its_operands() {
    let mut g = TestGraph::new();
    let a = g.add("time");
    let b = g.add("delta");
    let add = g.add("add");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&a, "seconds"), (&add, "operands"));
    g.link((&b, "delta"), (&add, "operands"));
    g.link((&add, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(wgsl.contains(" + "), "{wgsl}");
}

#[test]
fn integers_promote_to_floats_and_widen_the_family() {
    let mut g = TestGraph::new();
    let time = g.add("time");
    let add = g.add("add");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    // The frame counter is a u32: the sum is a u32.
    g.link((&time, "frame"), (&add, "operands"));
    assert_eq!(g.model.resolve_node_output_type(&add), WgslType::U32);
    g.link((&time, "seconds"), (&add, "operands"));
    assert_eq!(g.model.resolve_node_output_type(&add), WgslType::F32);
    g.link((&add, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    g.wgsl();
}

#[test]
fn division_uses_a_guarded_helper() {
    let mut g = TestGraph::new();
    let div = g.add("div");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.set(&div, "a", 1.0);
    g.link((&div, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    assert!(g.wgsl().contains("fn safeDivF32"));
}

#[test]
fn switch_selects_by_a_bool() {
    let mut g = TestGraph::new();
    let switch = g.add("switch");
    g.set(&switch, "condition", true);
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&switch, "result"), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    assert!(g.wgsl().contains("select("));
    let _ = InputValue::from(true);
}

#[test]
fn custom_wgsl_declares_a_function() {
    let mut g = TestGraph::new();
    let custom = g.add("custom-wgsl");
    let out = g.output();
    let uv = g.add("uv");
    g.link((&uv, "uv"), (&custom, &g.model.resolve_node_inputs(&custom)[0].id.clone()));
    let color_out = g.model.resolve_node_outputs(&custom)[0].id.clone();
    let declare = g.add("declare");
    g.force_type(&declare, WgslType::Vec4f);
    // vec3f into a vec4f declare isn't a component: widen through the output directly.
    g.link((&custom, &color_out), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(wgsl.contains("struct custom_"), "{wgsl}");
    assert!(wgsl.contains("vec4f(") && wgsl.contains(", 1.0)"), "vec3f output gets alpha 1: {wgsl}");
}

#[test]
fn repeat_zone_generates_a_loop() {
    let mut g = TestGraph::new();
    let input = g.model.create_zone(crate::zones::ZoneKind::Repeat, Default::default()).unwrap();
    let item = g.model.zone_items_of(&input)[0].id.clone();
    let output_id = g.model.zone_nodes(&input).unwrap().output.id;
    g.set(&input, "iterations", 4.0);
    let add = g.add("add");
    let declare = g.add("declare");
    let out = g.output();
    g.force_type(&declare, WgslType::Vec4f);
    g.set(&add, "operands", 1.0);
    g.link((&input, &item), (&add, "operands"));
    g.link((&add, "result"), (&output_id, &item));
    g.link((&output_id, &item), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    let wgsl = g.wgsl();
    assert!(wgsl.contains("for (var z_"), "{wgsl}");
}

/// A simulation counting frames: its item plus one, shown as the red channel.
fn counting_simulation(g: &mut TestGraph) -> (String, String) {
    let input = g.model.create_zone(crate::zones::ZoneKind::Simulation, Default::default()).unwrap();
    let item = g.model.zone_items_of(&input)[0].id.clone();
    let output_id = g.model.zone_nodes(&input).unwrap().output.id;
    let add = g.add("add");
    let declare = g.add("declare");
    let out = g.output();
    let one = g.add("declare");
    g.force_type(&one, WgslType::F32);
    g.set(&one, "x", 1.0);
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&input, &item), (&add, "operands"));
    g.link((&one, "value"), (&add, "operands"));
    g.link((&add, "result"), (&output_id, &item));
    g.link((&output_id, &item), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    (input, item)
}

#[test]
fn simulation_zones_are_drawn_by_start_and_step_passes() {
    use crate::codegen::{ResourceDecl, SimulationStage};
    let mut g = TestGraph::new();
    counting_simulation(&mut g);
    let program = g.program();
    let plan = &program.plan;
    let stages: Vec<_> = plan.passes.iter().map(|p| p.simulation.as_ref().and_then(|s| s.stage)).collect();
    assert_eq!(stages, [Some(SimulationStage::Start), Some(SimulationStage::Step), None]);
    for pass in &plan.passes {
        crate::testing::validate_wgsl(&pass.program.code);
    }
    let step = &plan.passes[1].program;
    assert!(step.code.contains("simulation_delta") || step.targets.is_some(), "{}", step.code);
    assert_eq!(step.targets.as_deref().map(<[_]>::len), Some(1));
    assert!(plan.simulates());
    let states = plan.resources.iter().filter(|r| matches!(r, ResourceDecl::State { .. })).count();
    assert_eq!(states, 2, "the previous and current frame's textures");
}

#[test]
fn a_simulation_graph_reads_its_state_in_the_frame_pass() {
    let mut g = TestGraph::new();
    counting_simulation(&mut g);
    let frame = &g.program().plan.passes[2].program;
    assert!(frame.code.contains("textureLoad(runtime_texture_0"), "{}", frame.code);
    assert_eq!(frame.textures.len(), 1);
}

#[test]
fn curve_nodes_generate_valid_wgsl_for_every_float_type() {
    use crate::curve::{CurvePoint, CurvePointKind, Vec2, linear_curve};
    let mut three = linear_curve();
    three.insert(
        1,
        CurvePoint {
            x: 0.4,
            y: 0.8,
            kind: CurvePointKind::Mirrored,
            left: Vec2::new(-0.1, 0.0),
            right: Vec2::new(0.1, 0.0),
        },
    );
    let flat = vec![CurvePoint { x: 0.5, y: 0.3, kind: CurvePointKind::Vector, left: Vec2::default(), right: Vec2::default() }];
    for points in [linear_curve(), three, flat, Vec::new()] {
        for ty in [WgslType::F32, WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f] {
            let mut g = TestGraph::new();
            let curve = g.add("curve");
            g.force_type(&curve, ty);
            let curve_points = points.clone();
            g.model.update_node_data(&curve, |d| d.curve = Some(curve_points));
            let to_color = g.add("declare");
            g.force_type(&to_color, WgslType::Vec4f);
            let float = g.add("to-float");
            g.link((&curve, "result"), (&float, "e"));
            let sum = g.add("add");
            g.force_type(&sum, WgslType::Vec4f);
            g.link((&float, "result"), (&sum, "operands"));
            let out = g.output();
            g.link((&sum, "result"), (&out, "color"));
            assert_eq!(g.wgsl().contains("fn curveSegment"), points.len() >= 2);
            crate::testing::validate_wgsl(&g.literal_program().program.code);
        }
    }
}

#[test]
fn a_curve_round_trips_through_saved_node_data() {
    let data = crate::nodes::default_data_for_type("curve");
    let json = serde_json::to_value(&data).expect("serialises");
    assert_eq!(json["curve"][0]["kind"], "aligned");
    let loaded: crate::types::NodeData = serde_json::from_value(json).expect("loads");
    assert_eq!(loaded, data);
}
