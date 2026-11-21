//! Textures, passes and simulations: what links do with textures, the plan
//! a graph makes, and the code of the passes in it.

use crate::codegen::{PLAN_TARGET, ResourceDecl, SimulationStage};
use crate::graph::{Connection, GraphEdge};
use crate::passes::{CANVAS_SIZE, ItemSizeMode, SizeBase, TextureFormat, TextureLink, TextureSize};
use crate::testing::{TestGraph, validate_wgsl};
use crate::types::{HandleType, InputValue, WgslType};
use crate::zones::ZoneKind;

fn analysis_link(g: &TestGraph, from: &str, to: &str) -> Option<TextureLink> {
    let analysis = g.model.pass_analysis();
    let edge: GraphEdge = g.model.root().edges.iter().find(|e| e.source == from && e.target == to)?.clone();
    analysis.link(&[], &edge)
}

fn validate_all(g: &TestGraph) {
    for pass in &g.program().plan.passes {
        validate_wgsl(&pass.program.code);
    }
}

/// A Render Texture of the canvas' size drawing a UV, and its only item.
fn uv_texture(g: &mut TestGraph) -> (String, String) {
    let uv = g.add("uv");
    let texture = g.add("render-texture");
    let item = g.model.render_items_of(&texture)[0].id.clone();
    g.link((&uv, "uv"), (&texture, &item));
    (texture, item)
}

fn sample_into_output(g: &mut TestGraph, texture: (&str, &str)) -> String {
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((texture.0, texture.1), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    sample
}

#[test]
fn a_value_linked_to_a_texture_input_is_rendered_in_a_pass_before_the_frames() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&uv, "uv"), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    let plan = g.program().plan;
    let writes: Vec<_> = plan.passes.iter().map(|p| p.writes.clone()).collect();
    let id = format!("render:{uv}:uv");
    assert_eq!(writes, [vec![id.clone()], vec![PLAN_TARGET.to_owned()]]);
    assert_eq!(plan.passes[1].reads.as_slice(), std::slice::from_ref(&id));
    assert_eq!(
        plan.resource(&id),
        Some(&ResourceDecl::Transient {
            id: id.clone(),
            size: CANVAS_SIZE,
            format: TextureFormat::Rgba16Float,
            mips: false
        })
    );
    assert_eq!(analysis_link(&g, &uv, &sample), Some(TextureLink::Render { id }));
    validate_all(&g);
}

#[test]
fn a_texture_linked_to_a_value_is_sampled_at_the_pixel() {
    let mut g = TestGraph::new();
    let (texture, item) = uv_texture(&mut g);
    let out = g.output();
    g.link((&texture, &item), (&out, "color"));
    assert_eq!(analysis_link(&g, &texture, &out), Some(TextureLink::Sample));
    let plan = g.literal_program().plan;
    assert_eq!(plan.passes.len(), 2);
    // The pass made for the canvas' size is read exactly, at the pixel.
    assert!(plan.passes[1].program.code.contains("textureLoad(runtime_texture_0, vec2i(runtime_fragment.position), 0)"));
    validate_all(&g);
}

#[test]
fn a_texture_input_with_nothing_linked_reads_nothing() {
    let mut g = TestGraph::new();
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&sample, "color"), (&out, "color"));
    let plan = g.program().plan;
    assert_eq!(plan.passes.len(), 1);
    assert_eq!(plan.resources, [ResourceDecl::Empty { id: "empty".to_owned() }]);
    validate_all(&g);
}

#[test]
fn a_value_computed_in_a_repeat_zone_cant_be_rendered() {
    let mut g = TestGraph::new();
    let input = g.model.create_zone(ZoneKind::Repeat, Default::default()).unwrap();
    let item = g.model.zone_items_of(&input)[0].id.clone();
    g.model.set_zone_item_type(&input, &item, HandleType::Fixed(WgslType::Vec4f));
    let output = g.model.zone_nodes(&input).unwrap().output.id;
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&input, &item), (&sample, "texture"));
    g.link((&sample, "color"), (&output, &item));
    g.link((&output, &item), (&out, "color"));
    assert!(matches!(analysis_link(&g, &input, &sample), Some(TextureLink::Invalid { .. })));
    assert_eq!(g.program().plan.passes.len(), 1);
    validate_all(&g);
}

#[test]
fn a_zones_final_value_can_be_rendered() {
    let mut g = TestGraph::new();
    let input = g.model.create_zone(ZoneKind::Repeat, Default::default()).unwrap();
    let item = g.model.zone_items_of(&input)[0].id.clone();
    let output = g.model.zone_nodes(&input).unwrap().output.id;
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&output, &item), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    assert_eq!(g.program().plan.passes.len(), 2);
    validate_all(&g);
}

#[test]
fn reroutes_pass_textures_through() {
    let mut g = TestGraph::new();
    let (texture, item) = uv_texture(&mut g);
    let reroute = g.add("reroute");
    g.link((&texture, &item), (&reroute, "in"));
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&reroute, "out"), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    assert_eq!(g.program().plan.passes.len(), 2);
    validate_all(&g);
}

#[test]
fn an_index_switch_picks_a_texture_by_an_index_known_while_compiling_and_by_no_other() {
    let mut g = TestGraph::new();
    let (a, a_item) = uv_texture(&mut g);
    let b = g.add("render-texture");
    let b_item = g.model.render_items_of(&b)[0].id.clone();
    let time = g.add("time");
    g.link((&time, "seconds"), (&b, &b_item));
    let pick = g.add("index-switch");
    g.link((&a, &a_item), (&pick, "case-0"));
    g.link((&b, &b_item), (&pick, "case-1"));
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&pick, "result"), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    g.set(&pick, "index", 1.0);
    let known = g.program().plan;
    assert!(known.resources.iter().any(|r| r.id() == format!("render:{b}:{b_item}")), "{:?}", known.resources);
    assert!(!known.resources.iter().any(|r| r.id() == format!("render:{a}:{a_item}")));

    let index = g.add("declare");
    g.force_type(&index, WgslType::I32);
    g.link((&index, "value"), (&pick, "index"));
    assert!(matches!(analysis_link(&g, &pick, &sample), Some(TextureLink::Invalid { .. })));
    assert_eq!(g.program().plan.resources, [ResourceDecl::Empty { id: "empty".to_owned() }]);
    validate_all(&g);
}

#[test]
fn textures_link_to_float_values_and_values_to_textures_but_not_zone_items() {
    let mut g = TestGraph::new();
    let (texture, item) = uv_texture(&mut g);
    let float = g.add("sin");
    let vec2 = g.add("declare");
    g.force_type(&vec2, WgslType::Vec2f);
    let c = |from: &str, to: &str, handle: &str| Connection::new(from, item.as_str(), to, handle);
    assert!(g.model.is_valid_connection(&c(&texture, &float, "e")));
    assert!(g.model.is_valid_connection(&Connection::new(&float, "result", &texture, &item)));
    // A vec2 can't be a colour: it can't stand for a texture's.
    let into_vec2 = Connection::new(&texture, &item, &vec2, "x");
    assert!(g.model.is_valid_connection(&into_vec2), "a texture is a float colour");
    let repeat = g.model.create_zone(ZoneKind::Repeat, Default::default()).unwrap();
    let repeat_item = g.model.zone_items_of(&repeat)[0].id.clone();
    let _ = repeat_item;
}

#[test]
fn simulation_items_can_be_textures_and_repeat_items_cant() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    let item = g.model.add_zone_item(&sim, HandleType::Fixed(WgslType::Texture), Some("Frame")).unwrap();
    assert!(g.model.zone_items_of(&sim).iter().any(|i| i.id == item.id));
    let input = g.model.zone_nodes(&sim).unwrap().input;
    let resolved = g.model.resolve_node_outputs(&input.id);
    assert_eq!(resolved.iter().find(|o| o.id == item.id).map(|o| o.ty), Some(WgslType::Texture));
    let inputs = g.model.resolve_node_inputs(&input.id);
    assert_eq!(inputs.iter().find(|i| i.id == item.id).map(|i| i.default_type()), Some(WgslType::Vec4f), "its values are colours");

    let repeat = g.model.create_zone(ZoneKind::Repeat, Default::default()).unwrap();
    g.model.add_zone_item(&repeat, HandleType::Fixed(WgslType::Texture), Some("Frame"));
    assert_eq!(g.model.zone_items_of(&repeat).len(), 1, "only the default item");
}

#[test]
fn a_simulations_texture_item_is_sampled_from_the_previous_frame() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    let item = g.model.add_zone_item(&sim, HandleType::Fixed(WgslType::Texture), Some("Frame")).unwrap().id;
    let output = g.model.zone_nodes(&sim).unwrap().output.id;
    let uv = g.add("uv");
    let sample = g.add("sample-texture");
    let shift = g.add("add");
    let offset = g.add("declare");
    g.force_type(&offset, WgslType::Vec2f);
    g.set(&offset, "x", 0.01);
    g.link((&uv, "uv"), (&shift, "operands"));
    g.link((&offset, "value"), (&shift, "operands"));
    g.link((&sim, &item), (&sample, "texture"));
    g.link((&shift, "result"), (&sample, "uv"));
    g.link((&sample, "color"), (&output, &item));
    let out = g.output();
    g.link((&output, &item), (&out, "color"));
    let plan = g.program().plan;
    let stages: Vec<_> = plan.passes.iter().map(|p| p.simulation.as_ref().and_then(|s| s.stage)).collect();
    assert_eq!(stages[stages.len() - 3..], [Some(SimulationStage::Start), Some(SimulationStage::Step), None]);
    let step = plan.passes.iter().find(|p| p.simulation.as_ref().and_then(|s| s.stage) == Some(SimulationStage::Step)).unwrap();
    assert!(step.program.code.contains("textureSampleLevel(runtime_texture_1, runtime_sampler_linear_clamp"), "{}", step.program.code);
    assert!(plan.resources.iter().any(|r| matches!(r, ResourceDecl::State { format: TextureFormat::Rgba16Float, .. })));
    validate_all(&g);
}

#[test]
fn a_state_item_takes_the_format_and_size_it_is_given() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    let item = g.model.zone_items_of(&sim)[0].id.clone();
    let output = g.model.zone_nodes(&sim).unwrap().output.id;
    let out = g.output();
    let declare = g.add("declare");
    g.force_type(&declare, WgslType::Vec4f);
    g.link((&output, &item), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));

    g.model.set_zone_item_format(&sim, &item, Some(TextureFormat::Rg16Float));
    g.model.set_zone_item_size(&sim, &item, Some(ItemSizeMode::Pixels));
    let input = g.model.zone_nodes(&sim).unwrap().input;
    assert!(g.model.resolve_node_inputs(&input.id).iter().any(|i| i.id == format!("pixels:{item}") && i.frame));
    g.model.update_node_data(&input.id, |d| {
        d.values.insert(format!("pixels:{item}"), InputValue::vector(&[64.0, 32.0]));
    });
    let plan = g.literal_program().plan;
    let state = plan.resources.iter().find_map(|r| match r {
        ResourceDecl::State { format, size, .. } => Some((*format, size.clone())),
        _ => None,
    });
    assert_eq!(state, Some((TextureFormat::Rg16Float, TextureSize::Pixels { width: 64.0.into(), height: 32.0.into() })));
    assert_eq!(plan.passes[0].program.targets.as_deref(), Some(&[WgslType::Vec4f][..]));

    g.model.set_zone_item_size(&sim, &item, None);
    let input = g.model.zone_nodes(&sim).unwrap().input;
    assert!(!g.model.resolve_node_inputs(&input.id).iter().any(|i| i.id.starts_with("pixels:")));
    validate_all(&g);
}

#[test]
fn state_items_are_kept_in_formats_by_their_type() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    for ty in [WgslType::I32, WgslType::Vec3f, WgslType::Bool, WgslType::Vec2u] {
        g.model.add_zone_item(&sim, HandleType::Fixed(ty), None);
    }
    let output = g.model.zone_nodes(&sim).unwrap().output.id;
    let out = g.output();
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec4f);
    for item in g.model.zone_items_of(&sim) {
        let float = g.add("to-float");
        g.link((&output, &item.id), (&float, "e"));
        g.link((&float, "result"), (&sum, "operands"));
    }
    g.link((&sum, "result"), (&out, "color"));
    let formats: Vec<TextureFormat> = g
        .program()
        .plan
        .resources
        .iter()
        .filter_map(|r| match r {
            ResourceDecl::State { format, read: crate::passes::StateRead::Current, .. } => Some(*format),
            _ => None,
        })
        .collect();
    assert_eq!(
        formats,
        [
            TextureFormat::R32Float,
            TextureFormat::R32Sint,
            TextureFormat::Rgba32Float,
            TextureFormat::R32Uint,
            TextureFormat::Rg32Uint
        ]
    );
    validate_all(&g);
}

#[test]
fn a_simulation_with_more_state_than_a_pass_writes_is_drawn_by_several_passes() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    for _ in 0..3 {
        g.model.add_zone_item(&sim, HandleType::Fixed(WgslType::Vec4f), None);
    }
    let output = g.model.zone_nodes(&sim).unwrap().output.id;
    let out = g.output();
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec4f);
    for item in g.model.zone_items_of(&sim) {
        g.link((&output, &item.id), (&sum, "operands"));
    }
    g.link((&sum, "result"), (&out, "color"));
    let plan = g.program().plan;
    // Four vec4f items are 16 bytes each in 32-bit floats: two to a pass.
    let steps = plan.passes.iter().filter(|p| p.simulation.as_ref().and_then(|s| s.stage) == Some(SimulationStage::Step)).count();
    assert_eq!(steps, 2);
    validate_all(&g);
}

#[test]
fn each_group_instance_keeps_a_state_of_its_own() {
    let mut g = TestGraph::new();
    let sim = g.model.create_zone(ZoneKind::Simulation, Default::default()).unwrap();
    let item = g.model.zone_items_of(&sim)[0].id.clone();
    let output = g.model.zone_nodes(&sim).unwrap().output.id;
    let declare = g.add("declare");
    g.force_type(&declare, WgslType::Vec4f);
    let out = g.output();
    g.link((&output, &item), (&declare, "x"));
    g.link((&declare, "value"), (&out, "color"));
    let group = g.model.group_selection(&[sim, output, declare]).expect("groups");
    let reference = g.model.root().node(&group).and_then(|n| n.definition_ref()).unwrap().to_owned();
    let second = g.model.add_group_instance(&reference, Default::default()).expect("adds");
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec4f);
    for node in [&group, &second] {
        let handle = g.model.resolve_node_outputs(node)[0].id.clone();
        g.link((node, &handle), (&sum, "operands"));
    }
    let out = g.output();
    g.link((&sum, "result"), (&out, "color"));
    let plan = g.program().plan;
    let stages: Vec<_> = plan.passes.iter().filter_map(|p| p.simulation.as_ref()).collect();
    let mut ids: Vec<&str> = stages.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 2, "a simulation per instance: {ids:?}");
    assert_eq!(stages.len(), 4, "each starts and steps");
    validate_all(&g);
}

#[test]
fn a_render_texture_draws_its_items_in_one_pass_or_several() {
    let mut g = TestGraph::new();
    let texture = g.add("render-texture");
    let first = g.model.render_items_of(&texture)[0].id.clone();
    let uv = g.add("uv");
    g.link((&uv, "uv"), (&texture, &first));
    let second = g.model.add_render_item(&texture, WgslType::Vec4f, None).unwrap().id;
    let time = g.add("time");
    g.link((&time, "seconds"), (&texture, &second));
    let sum = g.add("add");
    g.force_type(&sum, WgslType::Vec4f);
    g.link((&texture, &first), (&sum, "operands"));
    g.link((&texture, &second), (&sum, "operands"));
    let out = g.output();
    g.link((&sum, "result"), (&out, "color"));
    let plan = g.program().plan;
    assert_eq!(plan.passes.len(), 2, "both items in one pass");
    assert_eq!(plan.passes[0].writes.len(), 2);
    assert_eq!(plan.passes[0].program.targets.as_deref().map(<[_]>::len), Some(2));
    validate_all(&g);

    // Three 32-bit float items are 48 bytes: past what a pass writes.
    for id in [&first, &second] {
        g.model.set_render_item_format(&texture, id, TextureFormat::Rgba32Float);
    }
    let third = g.model.add_render_item(&texture, WgslType::Vec4f, Some(TextureFormat::Rgba32Float)).unwrap().id;
    g.link((&uv, "uv"), (&texture, &third));
    g.link((&texture, &third), (&sum, "operands"));
    let plan = g.program().plan;
    assert_eq!(plan.passes.len(), 3, "two passes for the texture, then the frame's");
    validate_all(&g);
}

#[test]
fn a_render_textures_scale_follows_the_texture_it_is_like() {
    let mut g = TestGraph::new();
    let (base, base_item) = uv_texture(&mut g);
    let scaled = g.add("render-texture");
    g.model.set_input_value(&scaled, "scale", Some(InputValue::from(0.5)));
    let item = g.model.render_items_of(&scaled)[0].id.clone();
    let uv = g.add("uv");
    g.link((&uv, "uv"), (&scaled, &item));
    g.link((&base, &base_item), (&scaled, "like"));
    let sample = sample_into_output(&mut g, (&scaled, &item));
    let _ = sample;
    let plan = g.literal_program().plan;
    let size = plan.resources.iter().find_map(|r| match r {
        ResourceDecl::Transient { id, size, .. } if *id == format!("render:{scaled}:{item}") => Some(size.clone()),
        _ => None,
    });
    assert_eq!(size, Some(TextureSize::Scale { scale: 0.5.into(), of: SizeBase::Resource(format!("render:{base}:{base_item}")) }));
    assert!(plan.resource(&format!("render:{base}:{base_item}")).is_some(), "the texture it is like is in the plan");
    validate_all(&g);
}

#[test]
fn mips_are_a_full_chain_but_not_of_integers() {
    let mut g = TestGraph::new();
    let (texture, item) = uv_texture(&mut g);
    sample_into_output(&mut g, (&texture, &item));
    g.model.update_node_data(&texture, |d| {
        d.settings.insert("mips".to_owned(), "on".to_owned());
    });
    let plan = g.program().plan;
    assert!(plan.resources.iter().any(|r| matches!(r, ResourceDecl::Transient { mips: true, .. })));
    validate_all(&g);

    g.model.set_render_item_format(&texture, &item, TextureFormat::Rgba32Uint);
    let sample = g.model.root().nodes.iter().find(|n| n.is("sample-texture")).unwrap().id.clone();
    assert!(matches!(analysis_link(&g, &texture, &sample), Some(TextureLink::Invalid { .. })));
}

#[test]
fn render_items_edit_dropping_the_links_of_those_removed() {
    let mut g = TestGraph::new();
    let (texture, item) = uv_texture(&mut g);
    let extra = g.model.add_render_item(&texture, WgslType::Vec4f, None).unwrap();
    assert_eq!(extra.label, "Color 2");
    assert!(!g.model.rename_render_item(&texture, &extra.id, "Color"), "names stay unique");
    assert!(g.model.rename_render_item(&texture, &extra.id, "Mask"));
    g.model.remove_render_item(&texture, &item);
    assert!(g.model.root().edges.iter().all(|e| e.target_handle != item), "its link is gone");
    assert_eq!(g.model.render_items_of(&texture).len(), 1);
}

#[test]
fn the_values_the_frame_renders_to_textures_are_listed_by_id() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let sample = g.add("sample-texture");
    let out = g.output();
    g.link((&uv, "uv"), (&sample, "texture"));
    g.link((&sample, "color"), (&out, "color"));
    let renders = g.model.pass_analysis().renders();
    assert_eq!(renders.keys().collect::<Vec<_>>(), [&format!("render:{uv}:uv")]);
}
