use super::builders::{Link, declare, model_for, node, shader, valid_shader};
use crate::codegen::{ShaderOptions, format_wgsl_literal};
use crate::graph::{GraphModel, ROOT_ID};
use crate::nodes::registered_nodes;
use crate::testing::validate_wgsl;
use crate::types::{InputValue, NodeData, WgslType, is_connection_compatible};

/// Every setting combination of a node.
fn setting_variants(
    settings: &[crate::types::NodeSetting],
) -> Vec<Vec<(&'static str, &'static str)>> {
    settings.iter().fold(vec![vec![]], |variants, setting| {
        variants
            .iter()
            .flat_map(|v| {
                setting.options.iter().map(move |(value, _)| {
                    let mut next = v.clone();
                    next.push((setting.key, *value));
                    next
                })
            })
            .collect()
    })
}

#[test]
fn every_node_generates_valid_wgsl_for_every_input_type() {
    let mut failures = Vec::new();
    let mut cases = 0;
    for descriptor in registered_nodes() {
        if descriptor.type_id == "frag-output" || descriptor.dev {
            continue;
        }
        let source_types: Vec<Option<WgslType>> = if descriptor.inputs.is_empty() {
            vec![None]
        } else {
            WgslType::ALL.iter().map(|t| Some(*t)).collect()
        };
        for settings in setting_variants(&descriptor.settings) {
            for source_type in &source_types {
                let mut data: NodeData = descriptor.default_data.clone();
                data.settings = settings
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect();
                let mut n = node("n", &descriptor.type_id);
                n.data = data;
                let mut nodes = vec![n];
                let mut links: Vec<Link> = Vec::new();
                let input_ids: Vec<(String, Vec<WgslType>)> = descriptor
                    .inputs
                    .iter()
                    .map(|i| (i.id.clone(), i.types.clone()))
                    .collect();
                if let Some(ty) = source_type {
                    nodes.push(declare("src", *ty, &[("x", 1.0), ("y", 2.0)]));
                    let mut any = false;
                    for (id, types) in &input_ids {
                        if types.iter().any(|t| is_connection_compatible(*ty, *t)) {
                            links.push((
                                "src",
                                "value",
                                "n",
                                Box::leak(id.clone().into_boxed_str()),
                            ));
                            any = true;
                        }
                    }
                    if !any {
                        continue;
                    }
                }
                if let Some(first) = descriptor.outputs.first() {
                    links.push((
                        "n",
                        Box::leak(first.id.clone().into_boxed_str()),
                        "out",
                        "color",
                    ));
                }
                let model = model_for(nodes, &links);
                let name = format!("{}({settings:?}) <- {source_type:?}", descriptor.type_id);
                cases += 1;
                for options in [
                    ShaderOptions::default(),
                    ShaderOptions {
                        uniform_scopes: vec![ROOT_ID.to_owned()],
                        ..Default::default()
                    },
                ] {
                    match model.generate_with(&options) {
                        Err(e) => failures.push(format!("{name}: {e:?}")),
                        Ok(program) => {
                            let code = program.program.code;
                            if std::panic::catch_unwind(|| validate_wgsl(&code)).is_err() {
                                failures.push(format!("{name}: invalid WGSL"));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cases > 500, "only {cases} cases");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn formats_every_kind_of_literal() {
    let f = |ty, value: Option<InputValue>| format_wgsl_literal(ty, value.as_ref());
    assert_eq!(f(WgslType::F32, Some(1.5.into())), "f32(1.5)");
    assert_eq!(f(WgslType::I32, Some(2.7.into())), "i32(2)");
    assert_eq!(f(WgslType::U32, Some((-3.0).into())), "u32(0)");
    assert_eq!(f(WgslType::Bool, Some(1.0.into())), "true");
    assert_eq!(f(WgslType::Vec3i, Some(4.0.into())), "vec3i(i32(4))");
    assert_eq!(f(WgslType::Vec2f, None), "vec2f()");
}

fn output_model(ty: WgslType) -> GraphModel {
    model_for(
        vec![declare(
            "d",
            ty,
            &[("x", 0.25), ("y", 0.5), ("z", 0.75), ("w", 0.5)],
        )],
        &[("d", "value", "out", "color")],
    )
}

#[test]
fn the_output_makes_a_vec3f_opaque_instead_of_padding_its_alpha_with_0() {
    let code = valid_shader(&output_model(WgslType::Vec3f));
    assert!(code.contains("let v_out = vec4f(v_d, 1.0);"), "{code}");
}

#[test]
fn the_output_keeps_a_vec4fs_alpha() {
    assert!(shader(&output_model(WgslType::Vec4f)).contains("let v_out = v_d;"));
}

#[test]
fn the_output_widens_narrower_values_to_an_opaque_vec3f() {
    let model = output_model(WgslType::Vec2f);
    assert_eq!(model.resolve_input_type("out", "color"), WgslType::Vec3f);
}

#[test]
fn a_fallback_takes_its_default_outside_groups_when_nothing_feeds_the_value() {
    let model = model_for(
        vec![node("f", "fallback"), node("t", "time")],
        &[
            ("t", "seconds", "f", "default"),
            ("f", "result", "out", "color"),
        ],
    );
    assert!(shader(&model).contains("let v_f = v_t_seconds;"));
}

mod aspect_uv {
    use super::*;
    use crate::codegen::ident_suffix;
    use crate::library::{LibraryEntry, entries};
    use crate::types::{GroupHandleDef, InputControl};

    fn entry() -> &'static LibraryEntry {
        entries()
            .iter()
            .find(|e| e.id == "std-aspect-uv")
            .expect("in the library")
    }

    fn handle(side: &[GroupHandleDef], label: &str) -> String {
        side.iter()
            .find(|h| h.label == label)
            .map(|h| h.id.clone())
            .expect("a handle of that label")
    }

    fn var_of(node_type: &str) -> String {
        let nodes: Vec<_> = entry()
            .definition
            .nodes
            .iter()
            .filter(|n| n.node_type == node_type)
            .collect();
        assert_eq!(nodes.len(), 1, "one {node_type}");
        format!("v_{}", ident_suffix(&nodes[0].id))
    }

    struct Ids {
        function: String,
        uv: String,
        mode: String,
        out: String,
    }

    fn ids() -> Ids {
        let e = entry();
        Ids {
            function: format!("group_h_{}", ident_suffix(&e.reference)),
            uv: handle(&e.definition.inputs, "Coordinates"),
            mode: handle(&e.definition.inputs, "Mode"),
            out: handle(&e.definition.outputs, "Coordinates"),
        }
    }

    fn aspect(id: &str, values: &[(&str, f64)]) -> crate::graph::GraphNode {
        let mut n = super::super::builders::instance(id, &entry().reference, NodeData::default());
        n.data.values = super::super::builders::values(values);
        n
    }

    fn leak(s: &str) -> &'static str {
        Box::leak(s.to_owned().into_boxed_str())
    }

    #[test]
    fn falls_back_to_the_fragment_uv_when_unlinked() {
        let i = ids();
        let model = model_for(
            vec![aspect("a", &[(&i.mode, 1.0)])],
            &[("a", leak(&i.out), "out", "color")],
        );
        let code = valid_shader(&model);
        assert!(
            code.contains(&format!("let v_a = {}__d0__e1_1(", i.function)),
            "{code}"
        );
        assert!(
            code.contains(&format!("let {} = {};", var_of("fallback"), var_of("uv"))),
            "{code}"
        );
        assert!(
            !code.contains(&format!("fn {}__e1_1(", i.function)),
            "{code}"
        );
    }

    #[test]
    fn uses_linked_coordinates_one_function_per_linked_and_unlinked_combination() {
        let i = ids();
        let model = model_for(
            vec![
                node("u", "uv"),
                aspect("a", &[]),
                aspect("b", &[]),
                node("sum", "add"),
            ],
            &[
                ("u", "uv", "a", leak(&i.uv)),
                ("a", leak(&i.out), "sum", "operands"),
                ("b", leak(&i.out), "sum", "operands"),
                ("sum", "result", "out", "color"),
            ],
        );
        let code = valid_shader(&model);
        assert!(
            code.contains(&format!("fn {}__e1_0(", i.function)),
            "{code}"
        );
        assert!(
            code.contains(&format!("fn {}__d0__e1_0(", i.function)),
            "{code}"
        );
        let ident = i.function.trim_start_matches("group_");
        assert_eq!(
            code.split(&format!("struct GroupOutputs_{ident} ")).count(),
            2,
            "{code}"
        );
    }

    #[test]
    fn only_computes_the_scale_of_the_picked_mode() {
        let i = ids();
        let at = |mode| {
            model_for(
                vec![aspect("a", &[(&i.mode, mode)])],
                &[("a", leak(&i.out), "out", "color")],
            )
        };
        let stretch = valid_shader(&at(2.0));
        assert!(!stretch.contains("runtime_frame.resolution"), "{stretch}");
        assert!(shader(&at(1.0)).contains("runtime_frame.resolution"));
    }

    #[test]
    fn exposes_the_mode_as_an_enum_and_the_coordinates_as_a_fallback_input() {
        let i = ids();
        let model = model_for(vec![aspect("a", &[])], &[]);
        let inputs = model.resolve_node_inputs("a");
        let mode = inputs.iter().find(|x| x.id == i.mode).expect("mode");
        let Some(InputControl::Enum { variants }) = &mode.control else {
            panic!("an enum")
        };
        let labels: Vec<&str> = variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, ["Contain (Fit)", "Cover (Fill)", "Stretch"]);
        assert!(
            inputs
                .iter()
                .find(|x| x.id == i.uv)
                .is_some_and(|x| x.fallback)
        );
        assert!(!mode.fallback);
    }

    #[test]
    fn lets_any_scalar_drive_an_enum_input() {
        let i = ids();
        let model = model_for(
            vec![node("t", "time"), aspect("a", &[])],
            &[
                ("t", "seconds", "a", leak(&i.mode)),
                ("a", leak(&i.out), "out", "color"),
            ],
        );
        let code = valid_shader(&model);
        assert!(code.contains("u32(v_t_seconds))"), "{code}");
    }
}
