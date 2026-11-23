//! Frame values: which are the same for a whole frame, the program computing
//! them, and the shaders reading them.

use std::rc::Rc;

use super::builders::{Link, declare, instance, model_for, node, root_project, with_values};
use crate::codegen::{CodegenGraph, FRAME_VALUES_VAR, ShaderOptions, UniformScalar};
use crate::frame::{
    FrameCompiler, FrameCompilerOptions, FrameEnv, FrameInput, FrameLink, FrameProgram,
    FrameValueAnalysis, evaluate_frame,
};
use crate::graph::{GraphDocument, GraphModel, Point, ROOT_ID};
use crate::passes::{FrameArg, Size, TextureSources};
use crate::testing::validate_wgsl;
use crate::types::{FLOAT_TYPES, Handle, HandleType, InputValue, TypeVariable, WgslType};
use crate::zones::ZoneKind;

const AT: Point = Point { x: 0.0, y: 0.0 };

fn env() -> FrameEnv<'static> {
    FrameEnv {
        time: 3.0,
        delta: 0.5,
        frame: 7.0,
        canvas: Size::new(640, 360),
        pass_size: Size::new(320, 180),
        ..FrameEnv::default()
    }
}

fn graph(model: &GraphModel) -> CodegenGraph<'_> {
    CodegenGraph {
        project: model.project(),
        resolver: model.resolver(),
    }
}

fn compiler<'g>(model: &'g GraphModel, scopes: &[&str]) -> Rc<FrameCompiler<'g>> {
    FrameCompiler::new(
        graph(model),
        Rc::new(TextureSources::new(graph(model))),
        FrameCompilerOptions {
            uniform_scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            texture_size: true,
        },
    )
}

fn x(n: f64) -> [(&'static str, InputValue); 1] {
    [("x", n.into())]
}

fn path(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

/// A group adding 1 to its input `a`.
fn increment() -> GraphDocument {
    GraphDocument {
        label: Some("Increment".into()),
        inputs: vec![Handle::new("a", "A", HandleType::Fixed(WgslType::F32))],
        outputs: vec![Handle::new("b", "B", HandleType::Fixed(WgslType::F32))],
        nodes: vec![
            node("gin", "group-input"),
            with_values(node("one", "declare"), &x(1.0)),
            node("sum", "add"),
            node("gout", "group-output"),
        ],
        edges: super::builders::edges(&[
            ("gin", "a", "sum", "operands"),
            ("one", "value", "sum", "operands"),
            ("sum", "result", "gout", "b"),
        ]),
        ..Default::default()
    }
}

fn analysis(model: &GraphModel) -> FrameValueAnalysis<'_> {
    FrameValueAnalysis::of(graph(model))
}

#[test]
fn maths_on_time_unlinked_values_and_canvas_sizes_are_frame_values() {
    let model = model_for(
        vec![
            node("time", "time"),
            with_values(node("two", "declare"), &x(2.0)),
            node("double", "mul"),
            node("size", "resolution"),
            node("width", "separate-vector"),
        ],
        &[
            ("time", "seconds", "double", "operands"),
            ("two", "value", "double", "operands"),
            ("size", "resolution", "width", "v"),
        ],
    );
    let frame = analysis(&model);
    assert!(frame.is_frame_value(&[], "double", "result"));
    assert!(frame.is_frame_value(&[], "two", "value"));
    assert!(frame.is_frame_value(&[], "width", "x"));
}

#[test]
fn what_differs_per_pixel_is_excluded_with_a_reason() {
    let model = model_for(
        vec![
            node("uv", "uv"),
            node("x", "separate-vector"),
            node("time", "time"),
            node("sum", "add"),
        ],
        &[
            ("uv", "uv", "x", "v"),
            ("x", "x", "sum", "operands"),
            ("time", "seconds", "sum", "operands"),
        ],
    );
    let frame = analysis(&model);
    assert!(!frame.is_frame_value(&[], "x", "x"));
    assert!(!frame.is_frame_value(&[], "sum", "result"));
    assert_eq!(
        frame.reason(&[], "sum", "result").as_deref(),
        Some("UV is computed for each pixel.")
    );
    assert!(!frame.is_frame_input(&[], "sum", "operands"));
}

#[test]
fn texture_reads_are_excluded() {
    let model = model_for(
        vec![node("s", "sample-texture"), node("a", "separate-vector")],
        &[("s", "color", "a", "v")],
    );
    let frame = analysis(&model);
    assert!(!frame.is_frame_value(&[], "s", "color"));
    assert!(!frame.is_frame_value(&[], "a", "x"));
}

#[test]
fn groups_pass_through_as_the_inputs_they_are_given() {
    let mut project = root_project(
        vec![
            node("time", "time"),
            node("uv", "uv"),
            node("x", "separate-vector"),
            instance("a", "increment", Default::default()),
            instance("b", "increment", Default::default()),
            instance("c", "increment", Default::default()),
        ],
        &[
            ("time", "seconds", "a", "a"),
            ("uv", "uv", "x", "v"),
            ("x", "x", "b", "a"),
        ],
    );
    project
        .definitions
        .insert("increment".to_owned(), increment());
    let model = GraphModel::new(project);
    let frame = analysis(&model);
    assert!(frame.is_frame_value(&[], "a", "b"));
    assert!(!frame.is_frame_value(&[], "b", "b"));
    // Unlinked, the group's input is an unlinked value.
    assert!(frame.is_frame_value(&[], "c", "b"));
    assert!(frame.is_frame_value(&path(&["a"]), "sum", "result"));
    assert!(!frame.is_frame_value(&path(&["b"]), "sum", "result"));
}

/// A repeat zone with `item` starting from `start` and becoming `op` of itself and its iteration.
fn repeat_zone(model: &mut GraphModel, op: &str) -> (String, String, String, String) {
    let zone = model.create_zone(ZoneKind::Repeat, AT).expect("creates");
    let nodes = model.zone_nodes(&zone).expect("paired");
    let output = nodes.output.id.clone();
    let item = model.zone_items_of(&zone)[0].id.clone();
    let next = model.add_node(op, AT).expect("adds");
    let mut link = |s: &str, sh: &str, t: &str, th: &str| {
        assert!(
            model.connect(crate::graph::Connection::new(s, sh, t, th)),
            "{s}.{sh} -> {t}.{th}"
        );
    };
    link(&zone, &item, &next, "operands");
    link(&zone, "iteration", &next, "operands");
    link(&next, "result", &output, &item);
    (zone, output, item, next)
}

#[test]
fn a_repeat_zone_over_frame_values_and_what_is_in_it_are_frame_values() {
    let mut model = model_for(vec![node("time", "time")], &[]);
    let (zone, output, item, next) = repeat_zone(&mut model, "mul");
    model.connect(crate::graph::Connection::new(
        "time", "seconds", &zone, &item,
    ));
    let frame = analysis(&model);
    assert!(frame.is_frame_value(&[], &output, &item));
    assert!(frame.is_frame_value(&[], &next, "result"));
}

#[test]
fn a_repeat_zone_with_a_value_per_pixel_in_it_is_excluded() {
    let mut model = model_for(
        vec![node("uv", "uv"), node("sep", "separate-vector")],
        &[("uv", "uv", "sep", "v")],
    );
    let zone = model.create_zone(ZoneKind::Repeat, AT).expect("creates");
    let output = model.zone_nodes(&zone).expect("paired").output.id.clone();
    let item = model.zone_items_of(&zone)[0].id.clone();
    let next = model.add_node("mul", AT).expect("adds");
    for (s, sh, t, th) in [
        ("sep", "x", next.as_str(), "operands"),
        (zone.as_str(), "iteration", next.as_str(), "operands"),
        (next.as_str(), "result", output.as_str(), item.as_str()),
    ] {
        assert!(model.connect(crate::graph::Connection::new(s, sh, t, th)));
    }
    let frame = analysis(&model);
    assert!(!frame.is_frame_value(&[], &output, &item));
    assert!(!frame.is_frame_value(&[], &next, "result"));
    assert!(!frame.is_frame_value(&[], &zone, "iteration"));
}

#[test]
fn links_tell_frame_values_from_per_pixel_ones_and_refuse_those_into_inputs_needing_one() {
    let model = model_for(
        vec![
            node("time", "time"),
            node("uv", "uv"),
            node("x", "separate-vector"),
            node("vid", "video"),
        ],
        &[
            ("time", "seconds", "vid", "time"),
            ("uv", "uv", "x", "v"),
            ("x", "x", "vid", "time"),
        ],
    );
    // Two links into one input: the second is what a user could drop on it.
    let frame = analysis(&model);
    let link = |from: &str| {
        let e = model
            .root()
            .edges
            .iter()
            .find(|e| e.source == from && e.target == "vid")
            .expect("linked");
        frame.link(&[], e)
    };
    assert_eq!(link("time"), Some(FrameLink::Frame));
    assert_eq!(
        link("x"),
        Some(FrameLink::Invalid {
            reason:
                "Needs a value that's the same for the whole frame: UV is computed for each pixel."
                    .into()
        })
    );
    // An input needing no frame value takes a per-pixel one as it always did.
    let plain = model_for(
        vec![
            node("uv", "uv"),
            node("x", "separate-vector"),
            node("here", "sin"),
        ],
        &[("uv", "uv", "x", "v"), ("x", "x", "here", "e")],
    );
    let frame = analysis(&plain);
    let e = plain
        .root()
        .edges
        .iter()
        .find(|e| e.target == "here")
        .expect("linked");
    assert_eq!(frame.link(&[], e), None);
}

#[test]
fn programs_evaluate_maths_on_time() {
    let model = model_for(
        vec![
            node("time", "time"),
            with_values(node("two", "declare"), &x(2.0)),
            node("double", "mul"),
        ],
        &[
            ("time", "seconds", "double", "operands"),
            ("two", "value", "double", "operands"),
        ],
    );
    let frame = compiler(&model, &[]);
    let arg = frame.arg(&[], "double", "result", 0).expect("compiles");
    let values = evaluate_frame(&frame.program(), &env());
    assert_eq!(values.arg(&arg), 6.0);
    assert_eq!(
        compiler(&model, &[]).arg(&[], "double", "result", 0),
        Some(arg)
    );
}

#[test]
fn programs_fold_what_reads_only_literals() {
    let model = model_for(
        vec![
            with_values(node("a", "declare"), &x(1.5)),
            with_values(node("b", "declare"), &x(2.0)),
            node("sum", "add"),
        ],
        &[
            ("a", "value", "sum", "operands"),
            ("b", "value", "sum", "operands"),
        ],
    );
    let frame = compiler(&model, &[]);
    assert_eq!(
        frame.arg(&[], "sum", "result", 0),
        Some(FrameArg::Literal(3.5))
    );
    assert!(frame.program().steps.is_empty());
}

#[test]
fn programs_read_unlinked_inputs_as_uniforms_in_the_scopes_asked() {
    let model = model_for(
        vec![
            node("time", "time"),
            with_values(node("d", "declare"), &x(4.0)),
            node("sum", "add"),
        ],
        &[
            ("time", "seconds", "sum", "operands"),
            ("d", "value", "sum", "operands"),
        ],
    );
    let frame = compiler(&model, &[ROOT_ID]);
    let arg = frame.arg(&[], "sum", "result", 0).expect("compiles");
    let uniforms = frame.uniforms();
    assert_eq!(
        uniforms
            .iter()
            .map(|(n, v)| (n.as_str(), v.0.clone()))
            .collect::<Vec<_>>(),
        [("u_d_x", vec![4.0])]
    );
    let ten = [("u_d_x".to_owned(), crate::codegen::UniformValue(vec![10.0]))];
    let values = evaluate_frame(
        &frame.program(),
        &FrameEnv {
            uniforms: &ten,
            ..env()
        },
    );
    assert_eq!(values.arg(&arg), 13.0);
}

#[test]
fn programs_evaluate_a_group_instances_value_with_the_types_it_is_given() {
    let mut project = root_project(
        vec![
            declare("v", WgslType::Vec2f, &[("x", 3.0), ("y", 4.0)]),
            instance("sq", "square", Default::default()),
            node("y", "separate-vector"),
        ],
        &[("v", "value", "sq", "a"), ("sq", "b", "y", "v")],
    );
    let variable = || HandleType::Variable {
        variable: "t".to_owned(),
    };
    project.definitions.insert(
        "square".to_owned(),
        GraphDocument {
            label: Some("Square".into()),
            type_variables: vec![TypeVariable {
                id: "t".into(),
                name: "T".into(),
                types: FLOAT_TYPES.to_vec(),
            }],
            inputs: vec![Handle::new("a", "A", variable())],
            outputs: vec![Handle::new("b", "B", variable())],
            nodes: vec![
                node("gin", "group-input"),
                node("mul", "mul"),
                node("gout", "group-output"),
            ],
            edges: super::builders::edges(&[
                ("gin", "a", "mul", "operands"),
                ("gin", "a", "mul", "operands"),
                ("mul", "result", "gout", "b"),
            ]),
            ..Default::default()
        },
    );
    let model = GraphModel::new(project);
    let frame = compiler(&model, &[]);
    let squared = frame.compile(&[], "sq", "b").expect("compiles");
    assert_eq!(squared.input, FrameInput::Literal(vec![9.0, 16.0]));
    assert_eq!(frame.arg(&[], "y", "y", 0), Some(FrameArg::Literal(16.0)));
}

#[test]
fn programs_run_a_repeat_zone() {
    let mut model = model_for(vec![node("time", "time")], &[]);
    let (zone, output, item, _) = repeat_zone(&mut model, "add");
    model.connect(crate::graph::Connection::new(
        "time", "seconds", &zone, &item,
    ));
    model.connect(crate::graph::Connection::new(
        "time",
        "frame",
        &zone,
        "iterations",
    ));
    let frame = compiler(&model, &[]);
    let arg = frame.arg(&[], &output, &item, 0).expect("compiles");
    // Time 3, then plus the iterations 0, 1, 2, 3, 4, 5, 6.
    assert_eq!(
        evaluate_frame(&frame.program(), &env()).arg(&arg),
        3.0 + 21.0
    );
    let later = evaluate_frame(
        &frame.program(),
        &FrameEnv {
            frame: 2.0,
            time: 0.0,
            ..env()
        },
    );
    assert_eq!(later.arg(&arg), 1.0);
}

#[test]
fn programs_dont_compile_what_differs_per_pixel() {
    let model = model_for(vec![node("uv", "uv")], &[("uv", "uv", "out", "color")]);
    assert_eq!(compiler(&model, &[]).compile(&[], "uv", "uv"), None);
}

// ── Shaders reading frame values ────────────────────────────────────────────

fn pulse() -> Vec<crate::graph::GraphNode> {
    vec![
        node("time", "time"),
        with_values(node("k", "declare"), &x(3.0)),
        node("scaled", "mul"),
        node("wave", "sin"),
    ]
}

const PULSE_LINKS: [Link; 4] = [
    ("time", "seconds", "scaled", "operands"),
    ("k", "value", "scaled", "operands"),
    ("scaled", "result", "wave", "e"),
    ("wave", "result", "out", "color"),
];

fn plan_of(model: &GraphModel, scopes: &[&str]) -> crate::codegen::RenderPlan {
    let options = ShaderOptions {
        uniform_scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        hoist_frame_values: true,
    };
    *model.generate_with(&options).expect("generates").plan
}

#[test]
fn hoisted_frame_values_are_read_from_the_frame_block_instead_of_computed_per_pixel() {
    let plan = plan_of(&model_for(pulse(), &PULSE_LINKS), &[]);
    let pass = &plan.passes[0];
    assert!(!pass.program.code.contains("sin("));
    assert!(!pass.program.code.contains("runtime_frame.time"));
    assert!(
        pass.program
            .code
            .contains(&format!("{FRAME_VALUES_VAR}.f_wave_result"))
    );
    assert_eq!(
        pass.program.frame_values.fields.keys().collect::<Vec<_>>(),
        ["f_wave_result"]
    );
    validate_wgsl(&pass.program.code);
}

#[test]
fn hoisted_values_are_what_the_cpu_computes_for_the_formula_at_any_time() {
    let plan = plan_of(&model_for(pulse(), &PULSE_LINKS), &[]);
    let reference = &plan.passes[0].program.frame_value_refs["f_wave_result"];
    for time in [0.0, 0.4, 1.7, 9.0] {
        let values = evaluate_frame(&plan.frame, &FrameEnv { time, ..env() });
        assert!(
            (values.value(reference)[0] - (time * 3.0).sin()).abs() < 1e-5,
            "{time}"
        );
    }
}

#[test]
fn hoisted_values_read_uniforms_where_inputs_are_so_editing_one_only_changes_them() {
    let mut model = model_for(pulse(), &PULSE_LINKS);
    let before = plan_of(&model, &[ROOT_ID]);
    model.set_input_value("k", "x", Some(5.0.into()));
    let after = plan_of(&model, &[ROOT_ID]);
    assert_eq!(after.passes[0].program.code, before.passes[0].program.code);
    let uniform = |plan: &crate::codegen::RenderPlan| {
        plan.uniforms
            .iter()
            .find(|(n, _)| n == "u_k_x")
            .map(|(_, v)| v.0.clone())
    };
    assert_eq!(uniform(&before), Some(vec![3.0]));
    assert_eq!(uniform(&after), Some(vec![5.0]));
    let reference = &after.passes[0].program.frame_value_refs["f_wave_result"];
    let values = evaluate_frame(
        &after.frame,
        &FrameEnv {
            time: 2.0,
            uniforms: &after.uniforms,
            ..env()
        },
    );
    assert!((values.value(reference)[0] - 10f64.sin()).abs() < 1e-5);
    validate_wgsl(&after.passes[0].program.code);
}

#[test]
fn per_pixel_values_and_what_only_they_read_stay_in_the_shader() {
    let mut nodes = pulse();
    nodes.extend([
        node("uv", "uv"),
        node("x", "separate-vector"),
        node("sum", "add"),
    ]);
    let model = model_for(
        nodes,
        &[
            PULSE_LINKS[0],
            PULSE_LINKS[1],
            PULSE_LINKS[2],
            ("uv", "uv", "x", "v"),
            ("x", "x", "sum", "operands"),
            ("wave", "result", "sum", "operands"),
            ("sum", "result", "out", "color"),
        ],
    );
    let plan = plan_of(&model, &[]);
    let code = &plan.passes[0].program.code;
    assert!(code.contains(&format!("{FRAME_VALUES_VAR}.f_wave_result")));
    assert!(code.contains("runtime_fragment.uv"));
    assert!(!code.contains("sin("));
    validate_wgsl(code);
}

#[test]
fn values_of_literals_only_stay_in_the_shader_for_the_compiler_to_fold() {
    let model = model_for(
        vec![with_values(node("wave", "sin"), &[("e", 2.0.into())])],
        &[("wave", "result", "out", "color")],
    );
    let plan = plan_of(&model, &[]);
    assert!(plan.passes[0].program.code.contains("sin("));
    assert!(
        plan.passes[0]
            .program
            .frame_values
            .fields
            .keys()
            .all(|k| k == "unused")
    );
    assert_eq!(plan.frame, FrameProgram::default());
}

#[test]
fn integers_and_booleans_are_stored_as_integers() {
    let model = model_for(
        vec![
            node("time", "time"),
            node("n", "to-int"),
            with_data_settings(
                with_values(node("big", "compare"), &[("b", 1.0.into())]),
                ">",
            ),
            node("uv", "uv"),
            node("x", "separate-vector"),
            node("pick", "switch"),
            node("scaled", "mul"),
        ],
        &[
            ("time", "seconds", "n", "e"),
            ("n", "result", "big", "a"),
            ("uv", "uv", "x", "v"),
            ("big", "result", "pick", "condition"),
            ("x", "x", "pick", "true"),
            ("n", "result", "scaled", "operands"),
            ("pick", "result", "scaled", "operands"),
            ("scaled", "result", "out", "color"),
        ],
    );
    let plan = plan_of(&model, &[]);
    let pass = &plan.passes[0];
    let field = |name: &str| pass.program.frame_values.fields[name];
    assert_eq!(field("f_n_result").scalar, UniformScalar::I32);
    assert_eq!(field("f_big_result").scalar, UniformScalar::U32);
    let values = evaluate_frame(&plan.frame, &FrameEnv { time: 2.7, ..env() });
    assert_eq!(
        values.value(&pass.program.frame_value_refs["f_n_result"]),
        [2.0]
    );
    assert_eq!(
        values.value(&pass.program.frame_value_refs["f_big_result"]),
        [1.0]
    );
    validate_wgsl(&pass.program.code);
}

fn with_data_settings(mut n: crate::graph::GraphNode, op: &str) -> crate::graph::GraphNode {
    n.data.settings.insert("op".to_owned(), op.to_owned());
    n
}

// ── Assets ──────────────────────────────────────────────────────────────────

use crate::assets::{Asset, AssetFile, VideoFile};

fn image(id: &str, width: u32, height: u32) -> Asset {
    Asset::Image(AssetFile {
        id: id.into(),
        name: id.into(),
        file: format!("assets/images/{id}.png"),
        mime: "image/png".into(),
        bytes: 1,
        hash: id.repeat(64)[..64].into(),
        width,
        height,
    })
}

fn video(id: &str, frame_rate: f64) -> Asset {
    Asset::Video(VideoFile {
        file: AssetFile {
            id: id.into(),
            name: id.into(),
            file: format!("assets/videos/{id}.mp4"),
            mime: "video/mp4".into(),
            bytes: 1,
            hash: id.repeat(64)[..64].into(),
            width: 64,
            height: 32,
        },
        duration: 2.0,
        frame_rate,
    })
}

fn with_assets(
    nodes: Vec<crate::graph::GraphNode>,
    links: &[Link],
    assets: Vec<Asset>,
) -> GraphModel {
    let mut project = root_project(nodes, links);
    project.assets = assets;
    GraphModel::new(project)
}

fn using(mut n: crate::graph::GraphNode, asset: &str) -> crate::graph::GraphNode {
    n.data.asset = Some(asset.to_owned());
    n
}

#[test]
fn programs_evaluate_the_octaves_of_a_texture_1001_pixels_wide() {
    let model = with_assets(
        vec![
            using(node("img", "image"), "w"),
            node("size", "texture-size"),
            node("x", "separate-vector"),
            node("log", "log"),
            with_values(node("two", "declare"), &x(2.0)),
            node("log2", "log"),
            node("div", "div"),
            node("ceil", "ceil"),
        ],
        &[
            ("img", "texture", "size", "texture"),
            ("size", "size", "x", "v"),
            ("x", "x", "log", "e"),
            ("two", "value", "log2", "e"),
            ("log", "result", "div", "a"),
            ("log2", "result", "div", "b"),
            ("div", "result", "ceil", "e"),
        ],
        vec![image("w", 1001, 500)],
    );
    let frame = compiler(&model, &[]);
    let arg = frame.arg(&[], "ceil", "result", 0).expect("compiles");
    let sized = |id: &str| (id == "media:w").then_some((Size::new(1001, 500), None));
    let values = evaluate_frame(
        &frame.program(),
        &FrameEnv {
            texture_size: Some(&sized),
            ..env()
        },
    );
    assert_eq!(values.arg(&arg), 10.0);
    // The same maths for another texture: 1000 wide is 9.97 octaves, 2000 wide 10.97.
    let wider = |_: &str| Some((Size::new(2000, 1), None));
    let values = evaluate_frame(
        &frame.program(),
        &FrameEnv {
            texture_size: Some(&wider),
            ..env()
        },
    );
    assert_eq!(values.arg(&arg), 11.0);
}

#[test]
fn a_video_gives_its_frame_interval_as_a_frame_value() {
    let model = with_assets(
        vec![using(node("vid", "video"), "w")],
        &[("vid", "interval", "out", "color")],
        vec![video("v", 25.0), video("w", 24.0)],
    );
    let frame = analysis(&model);
    assert!(frame.is_frame_value(&[], "vid", "interval"));
    assert!(frame.is_frame_value(&[], "vid", "duration"));
    assert!(!frame.is_frame_value(&[], "vid", "size"));
    let compiled = compiler(&model, &[]);
    let arg = compiled.arg(&[], "vid", "interval", 0).expect("compiles");
    let values = evaluate_frame(&compiled.program(), &env());
    assert_eq!(values.arg(&arg), f64::from((1.0 / 24.0) as f32));
}

#[test]
fn a_video_frame_interval_is_compiled_into_a_shader() {
    let model = with_assets(
        vec![using(node("vid", "video"), "v")],
        &[("vid", "interval", "out", "color")],
        vec![video("v", 25.0)],
    );
    assert!(super::builders::shader(&model).contains("f32(0.0399999"));
}

fn two_frames() -> GraphModel {
    with_assets(
        vec![
            using(node("now", "video"), "v"),
            using(node("before", "video"), "v"),
            node("time", "time"),
            node("back", "sub"),
            node("diff", "sub"),
            node("a", "sample-texture"),
            node("b", "sample-texture"),
        ],
        &[
            ("time", "seconds", "back", "a"),
            ("now", "interval", "back", "b"),
            ("back", "result", "before", "time"),
            ("now", "texture", "a", "texture"),
            ("before", "texture", "b", "texture"),
            ("a", "color", "diff", "a"),
            ("b", "color", "diff", "b"),
            ("diff", "result", "out", "color"),
        ],
        vec![video("v", 25.0)],
    )
}

#[test]
fn every_video_node_gets_a_texture_of_its_own_even_on_one_asset() {
    use crate::codegen::ResourceDecl;
    let plan = plan_of(&two_frames(), &[]);
    let videos: Vec<&ResourceDecl> = plan
        .resources
        .iter()
        .filter(|r| matches!(r, ResourceDecl::Media { .. }))
        .collect();
    assert_eq!(videos.len(), 2);
    assert_ne!(videos[0].id(), videos[1].id());
}

#[test]
fn a_video_shows_the_frame_time_by_default_and_the_time_linked_to_it_otherwise() {
    use crate::codegen::ResourceDecl;
    let plan = plan_of(&two_frames(), &[]);
    let times: Vec<Option<FrameArg>> = plan
        .resources
        .iter()
        .filter_map(|r| match r {
            ResourceDecl::Media { time, .. } => Some(time.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(times.iter().filter(|t| t.is_none()).count(), 1);
    let linked = times.into_iter().flatten().next().expect("one has a time");
    let values = evaluate_frame(&plan.frame, &FrameEnv { time: 3.0, ..env() });
    assert_eq!(values.arg(&linked), f64::from(3.0f32 - 0.04f32));
}

#[test]
fn a_video_refuses_a_time_computed_per_pixel() {
    let model = with_assets(
        vec![
            using(node("vid", "video"), "v"),
            node("uv", "uv"),
            node("x", "separate-vector"),
        ],
        &[("uv", "uv", "x", "v"), ("x", "x", "vid", "time")],
        vec![video("v", 25.0)],
    );
    let edge = model
        .root()
        .edges
        .iter()
        .find(|e| e.target == "vid")
        .expect("linked");
    assert!(matches!(
        analysis(&model).link(&[], edge),
        Some(FrameLink::Invalid { .. })
    ));
}
