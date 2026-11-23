use std::collections::BTreeMap;

use super::builders::{
    Link, declare, edges, instance, model_for, node, root_project, shader, types, valid_shader,
    values,
};
use crate::enums::case_handle;
use crate::graph::{GraphDocument, GraphModel, GraphNode, GraphProject, GroupSide};
use crate::types::{
    EnumVariant, GroupHandleDef, HandleType, InputControl, InputOwner, NodeData, TypeOwner,
    TypeVariable, WgslType,
};

fn variants() -> Vec<EnumVariant> {
    vec![
        EnumVariant::new(0, "Zero"),
        EnumVariant::new(2, "Two"),
        EnumVariant::new(5, "Five"),
    ]
}

fn input(id: &str, label: &str, ty: impl Into<HandleType>) -> GroupHandleDef {
    GroupHandleDef::new(id, label, ty.into())
}

fn owned(mut h: GroupHandleDef, enum_id: &str, value: i64) -> GroupHandleDef {
    h.owner = Some(InputOwner {
        enum_id: enum_id.to_owned(),
        value,
    });
    h
}

fn type_owned(mut h: GroupHandleDef, variable: &str, types: &[WgslType]) -> GroupHandleDef {
    h.type_owner = Some(TypeOwner {
        variable: variable.to_owned(),
        types: types.to_vec(),
    });
    h
}

fn enum_input(id: &str, label: &str, variants: Vec<EnumVariant>) -> GroupHandleDef {
    let mut h = input(id, label, WgslType::U32);
    h.variants = Some(variants);
    h
}

fn variable(id: &str, types: &[WgslType]) -> TypeVariable {
    TypeVariable {
        id: id.to_owned(),
        name: id.to_owned(),
        types: types.to_vec(),
    }
}

fn two_documents(
    root_nodes: Vec<GraphNode>,
    root_links: &[Link],
    def: GraphDocument,
) -> GraphModel {
    let mut project: GraphProject = root_project(root_nodes, root_links);
    project.definitions.insert("def".to_owned(), def);
    project.reindex();
    GraphModel::new(project)
}

/// A group picking, by its "mode" enum, a constant (Zero), its "x" input
/// (Two) or the time (Five). Its instance "g" feeds the output.
fn match_model(
    instance_data: NodeData,
    root_nodes: Vec<GraphNode>,
    root_links: &[Link],
) -> GraphModel {
    let mut nodes = vec![instance("g", "def", instance_data)];
    nodes.extend(root_nodes);
    let mut links: Vec<Link> = vec![("g", "result", "out", "color")];
    links.extend_from_slice(root_links);
    let mut one = node("one", "declare");
    one.data.values = values(&[("x", 1.0)]);
    let def = GraphDocument {
        inputs: vec![
            enum_input("mode", "Mode", variants()),
            owned(input("x", "X", WgslType::F32), "mode", 2),
        ],
        outputs: vec![input("result", "Result", WgslType::F32)],
        nodes: vec![
            node("in", "group-input"),
            node("go", "group-output"),
            node("m", "match"),
            one,
            node("t", "time"),
        ],
        edges: edges(&[
            ("in", "mode", "m", "selector"),
            ("one", "value", "m", "case-0"),
            ("in", "x", "m", "case-2"),
            ("t", "seconds", "m", "case-5"),
            ("m", "result", "go", "result"),
        ]),
        ..Default::default()
    };
    two_documents(nodes, &links, def)
}

fn with_values(entries: &[(&str, f64)]) -> NodeData {
    NodeData {
        values: values(entries),
        ..Default::default()
    }
}

fn case_ids(model: &GraphModel) -> Vec<String> {
    model
        .resolve_node_inputs("m")
        .iter()
        .map(|i| format!("{}:{}", i.id, i.label))
        .collect()
}

fn def_links(model: &GraphModel) -> Vec<String> {
    model
        .project()
        .document("def")
        .map(|d| {
            d.edges
                .iter()
                .filter(|e| e.target == "m")
                .map(|e| e.target_handle.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn hidden(model: &GraphModel) -> Vec<String> {
    model.hidden_inputs("g").into_iter().collect()
}

#[test]
fn match_has_one_case_per_variant_of_the_enum_linked_to_it() {
    let model = match_model(NodeData::default(), vec![], &[]);
    assert_eq!(
        case_ids(&model),
        ["selector:enum", "case-0:Zero", "case-2:Two", "case-5:Five"]
    );
}

#[test]
fn match_computes_only_the_case_picked_by_an_enum_value_known_while_compiling() {
    let model = match_model(with_values(&[("mode", 2.0)]), vec![], &[]);
    let code = valid_shader(&model);
    assert!(code.contains("let v_m = h_x;"), "{code}");
    assert!(!code.contains("runtime_frame.time"), "{code}");
}

#[test]
fn match_picks_the_first_case_for_a_value_matching_no_variant() {
    let code = shader(&match_model(with_values(&[("mode", 3.0)]), vec![], &[]));
    assert!(code.contains("let v_m = v_one;"), "{code}");
}

#[test]
fn match_selects_among_every_case_when_the_value_is_only_known_in_the_shader() {
    let model = match_model(
        NodeData::default(),
        vec![node("ft", "time")],
        &[("ft", "seconds", "g", "mode")],
    );
    let code = valid_shader(&model);
    assert!(
        code.contains("select(select(v_one, h_x, h_mode == 2u), v_t_seconds, h_mode == 5u)"),
        "{code}"
    );
}

#[test]
fn match_only_lets_an_enum_pick_its_case() {
    let mut model = match_model(NodeData::default(), vec![], &[]);
    model.enter_group("g");
    let link =
        |source: &str, handle: &str| crate::graph::Connection::new(source, handle, "m", "selector");
    assert!(!model.is_valid_connection(&link("t", "seconds")));
    assert!(model.is_valid_connection(&link("in", "mode")));
}

#[test]
fn match_keeps_links_on_their_case_through_renames_and_reorders_and_drops_removed_cases() {
    let mut model = match_model(NodeData::default(), vec![], &[]);
    let [zero, two, _] = <[EnumVariant; 3]>::try_from(variants()).expect("three variants");
    let deux = EnumVariant {
        label: "Deux".to_owned(),
        ..two
    };
    model.set_handle_variants("def", "mode", Some(vec![deux, zero]));
    assert_eq!(
        case_ids(&model),
        ["selector:enum", "case-2:Deux", "case-0:Zero"]
    );
    assert_eq!(def_links(&model), ["selector", "case-0", "case-2"]);
    model.set_handle_variants("def", "mode", None);
    assert_eq!(case_ids(&model), ["selector:enum"]);
    assert_eq!(def_links(&model), ["selector"]);
}

#[test]
fn a_group_node_hides_inputs_under_variants_it_isnt_on() {
    let hidden_with = |data, nodes, links: &[Link]| hidden(&match_model(data, nodes, links));
    assert_eq!(
        hidden_with(with_values(&[("mode", 0.0)]), vec![], &[]),
        ["x"]
    );
    assert!(hidden_with(with_values(&[("mode", 2.0)]), vec![], &[]).is_empty());
    // Unknown while compiling: every variant's inputs.
    let linked = [("ft", "seconds", "g", "mode")];
    assert!(hidden_with(NodeData::default(), vec![node("ft", "time")], &linked).is_empty());
    // Linked or not: its link stays, unused.
    let to_x = [("ft", "seconds", "g", "x")];
    assert_eq!(
        hidden_with(
            with_values(&[("mode", 0.0)]),
            vec![node("ft", "time")],
            &to_x
        ),
        ["x"]
    );
}

#[test]
fn inputs_are_added_to_a_variant_after_its_enums_nested_enums_included() {
    let mut model = match_model(with_values(&[("mode", 2.0)]), vec![], &[]);
    let top = model
        .add_handle("def", GroupSide::Input, WgslType::F32.into(), None, None)
        .expect("added");
    let ab = vec![EnumVariant::new(0, "A"), EnumVariant::new(1, "B")];
    let owner = InputOwner {
        enum_id: "mode".to_owned(),
        value: 2,
    };
    let sub = model
        .add_handle(
            "def",
            GroupSide::Input,
            WgslType::U32.into(),
            Some(ab),
            Some(owner),
        )
        .expect("added");
    let z_owner = InputOwner {
        enum_id: sub.id.clone(),
        value: 1,
    };
    let z = model
        .add_handle(
            "def",
            GroupSide::Input,
            WgslType::F32.into(),
            None,
            Some(z_owner),
        )
        .expect("added");
    let ids = |m: &GraphModel| -> Vec<String> {
        m.project()
            .document("def")
            .map(|d| d.inputs.iter().map(|h| h.id.clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(ids(&model), ["mode", "x", &sub.id, &z.id, &top.id]);
    assert_eq!(hidden(&model), std::slice::from_ref(&z.id));
    model.set_input_value("g", "mode", Some(0.0.into()));
    let mut expected = vec!["x".to_owned(), sub.id.clone(), z.id.clone()];
    expected.sort();
    let mut now = hidden(&model);
    now.sort();
    assert_eq!(now, expected);

    // Removing a variant, or an enum, removes the inputs under it.
    model.set_handle_variants(
        "def",
        &sub.id,
        Some(sub.variants.clone().unwrap_or_default()[..1].to_vec()),
    );
    assert_eq!(ids(&model), ["mode", "x", &sub.id, &top.id]);
    model.remove_handle("def", GroupSide::Input, "mode");
    assert_eq!(ids(&model), [top.id.as_str()]);
}

#[test]
fn inputs_never_shown_together_share_a_label() {
    let mut model = match_model(NodeData::default(), vec![], &[]);
    let owner = InputOwner {
        enum_id: "mode".to_owned(),
        value: 5,
    };
    let y = model
        .add_handle(
            "def",
            GroupSide::Input,
            WgslType::F32.into(),
            None,
            Some(owner),
        )
        .expect("added");
    assert!(model.set_handle_label("def", GroupSide::Input, &y.id, "X"));
    assert!(!model.set_handle_label("def", GroupSide::Input, "mode", "X"));
}

#[test]
fn a_match_has_a_type_picker_with_or_without_an_enum_linked() {
    let model = match_model(NodeData::default(), vec![], &[]);
    assert_eq!(model.type_choice("m").map(|c| c.inferable), Some(true));
    let lone = model_for(vec![node("m", "match")], &[]);
    assert!(lone.type_choice("m").is_some_and(|c| c.types.len() > 1));
}

const CASE_TYPES: [WgslType; 4] = [
    WgslType::F32,
    WgslType::Vec2f,
    WgslType::Vec3f,
    WgslType::Vec4f,
];

/// A group generic over its "v" input's width, picking by it one of four
/// constants (`c1`..`c4`) each as wide as its case. Its instance is fed a
/// `ty`; the group's output takes what reaches it.
fn width_switch_model(ty: WgslType) -> GraphModel {
    let nodes = vec![
        declare("src", ty, &[]),
        instance("g", "def", NodeData::default()),
    ];
    let mut def_nodes = vec![
        node("in", "group-input"),
        node("go", "group-output"),
        node("ws", "width-switch"),
    ];
    let mut links: Vec<Link> = vec![("in", "v", "ws", "value")];
    const IDS: [&str; 4] = ["c1", "c2", "c3", "c4"];
    const CASES: [&str; 4] = ["case-1", "case-2", "case-3", "case-4"];
    for (i, t) in CASE_TYPES.iter().enumerate() {
        def_nodes.push(declare(IDS[i], *t, &[("x", (i + 1) as f64)]));
        links.push((IDS[i], "value", "ws", CASES[i]));
    }
    links.push(("ws", "result", "go", "result"));
    let def = GraphDocument {
        type_variables: vec![variable("t", &CASE_TYPES)],
        inputs: vec![input(
            "v",
            "V",
            HandleType::Variable {
                variable: "t".to_owned(),
            },
        )],
        outputs: vec![input(
            "result",
            "Result",
            HandleType::Inferred { inferred: true },
        )],
        nodes: def_nodes,
        edges: edges(&links),
        ..Default::default()
    };
    two_documents(
        nodes,
        &[("src", "value", "g", "v"), ("g", "result", "out", "color")],
        def,
    )
}

#[test]
fn a_width_switch_takes_the_type_of_the_case_its_values_width_picks() {
    for ty in CASE_TYPES {
        let mut model = width_switch_model(ty);
        assert_eq!(model.resolve_output_handle_type("g", "result"), ty);
        model.enter_group("g");
        assert_eq!(model.resolve_node_output_type("ws"), ty);
    }
}

#[test]
fn a_width_switch_computes_only_the_picked_case() {
    for (n, ty) in CASE_TYPES.iter().enumerate() {
        let picked = n + 1;
        let code = valid_shader(&width_switch_model(*ty));
        assert!(code.contains(&format!("let v_ws = v_c{picked};")), "{code}");
        let other = if picked == 1 { 2 } else { 1 };
        assert!(!code.contains(&format!("v_c{other} =")), "{code}");
    }
}

/// A group generic over "T" (vec2f, vec3f or vec4f), its vector "v" of T; an
/// Orientation enum for vec2f and one for vec3f, the latter with an input
/// "k" under its variant B; "s" shown for vec3f and vec4f. Its instance "g"
/// forces T to `ty`.
fn type_owner_model(ty: WgslType) -> GraphModel {
    let ab = || vec![EnumVariant::new(0, "A"), EnumVariant::new(1, "B")];
    let mut o2 = enum_input("o2", "Orientation", ab());
    o2.type_owner = Some(TypeOwner {
        variable: "T".to_owned(),
        types: vec![WgslType::Vec2f],
    });
    let mut o3 = enum_input("o3", "Orientation", ab());
    o3.type_owner = Some(TypeOwner {
        variable: "T".to_owned(),
        types: vec![WgslType::Vec3f],
    });
    let def = GraphDocument {
        type_variables: vec![variable(
            "T",
            &[WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f],
        )],
        inputs: vec![
            input(
                "v",
                "Vector",
                HandleType::Variable {
                    variable: "T".to_owned(),
                },
            ),
            o2,
            o3,
            owned(input("k", "K", WgslType::F32), "o3", 1),
            type_owned(
                input("s", "S", WgslType::F32),
                "T",
                &[WgslType::Vec3f, WgslType::Vec4f],
            ),
        ],
        outputs: vec![input("result", "Result", WgslType::F32)],
        nodes: vec![node("in", "group-input"), node("go", "group-output")],
        ..Default::default()
    };
    let data = NodeData {
        types: types(&[("T", ty)]),
        values: values(&[("o3", 1.0)]),
        ..Default::default()
    };
    two_documents(
        vec![instance("g", "def", data)],
        &[("g", "result", "out", "color")],
        def,
    )
}

fn sorted_hidden(model: &GraphModel) -> Vec<String> {
    let mut ids = hidden(model);
    ids.sort();
    ids
}

#[test]
fn inputs_owned_by_types_are_shown_only_while_their_variable_has_one_of_their_types() {
    assert_eq!(
        sorted_hidden(&type_owner_model(WgslType::Vec2f)),
        ["k", "o3", "s"]
    );
    assert_eq!(sorted_hidden(&type_owner_model(WgslType::Vec3f)), ["o2"]);
    // Under an enum that's hidden by its type, though on its variant.
    assert_eq!(
        sorted_hidden(&type_owner_model(WgslType::Vec4f)),
        ["k", "o2", "o3"]
    );
}

#[test]
fn inputs_owned_by_types_may_share_a_label_with_inputs_never_shown_for_the_same_types() {
    let mut model = type_owner_model(WgslType::Vec2f);
    assert!(!model.set_handle_label("def", GroupSide::Input, "s", "Orientation"));
    assert!(model.set_handle_label("def", GroupSide::Input, "o2", "S"));
}

fn owners(model: &GraphModel) -> BTreeMap<String, Option<Vec<WgslType>>> {
    model
        .project()
        .document("def")
        .map(|d| {
            d.inputs
                .iter()
                .map(|h| (h.id.clone(), h.type_owner.as_ref().map(|o| o.types.clone())))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn type_owners_follow_their_variables_types_and_are_shown_for_all_without_it() {
    let mut model = type_owner_model(WgslType::Vec3f);
    let (v2, v3) = (WgslType::Vec2f, WgslType::Vec3f);
    model.set_type_variable("def", "T", None, Some(&[v2, v3]));
    let now = owners(&model);
    assert_eq!(
        (&now["o2"], &now["o3"], &now["s"]),
        (&Some(vec![v2]), &Some(vec![v3]), &Some(vec![v3]))
    );
    // Shown for every type it may still take: owned by none.
    model.set_type_variable("def", "T", None, Some(&[v3]));
    let now = owners(&model);
    assert_eq!((&now["o2"], &now["o3"], &now["s"]), (&None, &None, &None));
    model.set_type_variable("def", "T", None, Some(&[v2, v3]));
    model.set_handle_type_owner(
        "def",
        "o2",
        Some(TypeOwner {
            variable: "T".to_owned(),
            types: vec![v2],
        }),
    );
    model.remove_type_variable("def", "T");
    assert_eq!(owners(&model)["o2"], None);
}

#[test]
fn a_type_owner_is_set_within_its_variables_types() {
    let mut model = type_owner_model(WgslType::Vec3f);
    let owner = |m: &GraphModel| {
        m.project()
            .document("def")
            .and_then(|d| d.inputs.iter().find(|h| h.id == "v")?.type_owner.clone())
    };
    let set = |m: &mut GraphModel, types: &[WgslType]| {
        m.set_handle_type_owner(
            "def",
            "v",
            Some(TypeOwner {
                variable: "T".to_owned(),
                types: types.to_vec(),
            }),
        );
    };
    set(&mut model, &[WgslType::Vec4f, WgslType::F32]);
    let vec4 = TypeOwner {
        variable: "T".to_owned(),
        types: vec![WgslType::Vec4f],
    };
    assert_eq!(owner(&model), Some(vec4.clone()));
    // None of its types: nothing changes.
    set(&mut model, &[WgslType::F32]);
    assert_eq!(owner(&model), Some(vec4));
    model.set_handle_type_owner("def", "v", None);
    assert_eq!(owner(&model), None);
}

#[test]
fn type_owners_save_and_load() {
    let model = type_owner_model(WgslType::Vec3f);
    let loaded = GraphProject::from_json(&model.project().to_json()).expect("loads");
    let copy = GraphModel::new(loaded);
    assert_eq!(sorted_hidden(&copy), ["o2"]);
}

/// A group generic over "T" (vec2f or vec3f) whose "o" enum offers A for
/// vec2f, B for both and C for vec3f, "k" being under C. Its instance "g"
/// forces T to `ty` and holds `value`.
fn variant_model(ty: WgslType, value: f64) -> GraphModel {
    let mut o = enum_input("o", "Option", vec![]);
    o.variants_by = Some("T".to_owned());
    o.variants = Some(vec![
        EnumVariant {
            types: Some(vec![WgslType::Vec2f]),
            ..EnumVariant::new(0, "A")
        },
        EnumVariant::new(1, "B"),
        EnumVariant {
            types: Some(vec![WgslType::Vec3f]),
            ..EnumVariant::new(2, "C")
        },
    ]);
    let constants: Vec<GraphNode> = ["a", "b", "c"]
        .iter()
        .enumerate()
        .map(|(i, id)| declare(id, WgslType::F32, &[("x", i as f64)]))
        .collect();
    let mut def_nodes = vec![
        node("in", "group-input"),
        node("go", "group-output"),
        node("m", "match"),
    ];
    def_nodes.extend(constants);
    let def = GraphDocument {
        type_variables: vec![variable("T", &[WgslType::Vec2f, WgslType::Vec3f])],
        inputs: vec![
            input(
                "v",
                "Vector",
                HandleType::Variable {
                    variable: "T".to_owned(),
                },
            ),
            o,
            owned(input("k", "K", WgslType::F32), "o", 2),
        ],
        outputs: vec![input("result", "Result", WgslType::F32)],
        nodes: def_nodes,
        edges: edges(&[
            ("in", "o", "m", "selector"),
            ("a", "value", "m", "case-0"),
            ("b", "value", "m", "case-1"),
            ("c", "value", "m", "case-2"),
            ("m", "result", "go", "result"),
        ]),
        ..Default::default()
    };
    let data = NodeData {
        types: types(&[("T", ty)]),
        values: values(&[("o", value)]),
        ..Default::default()
    };
    two_documents(
        vec![instance("g", "def", data)],
        &[("g", "result", "out", "color")],
        def,
    )
}

fn offered(model: &GraphModel) -> Vec<String> {
    let inputs = model.resolve_node_inputs("g");
    match inputs
        .iter()
        .find(|i| i.id == "o")
        .and_then(|i| i.control.clone())
    {
        Some(InputControl::Enum { variants }) => variants.into_iter().map(|v| v.label).collect(),
        _ => vec![],
    }
}

fn enum_of(model: &GraphModel) -> GroupHandleDef {
    model
        .project()
        .document("def")
        .and_then(|d| d.inputs.iter().find(|h| h.id == "o").cloned())
        .expect("the enum")
}

fn variant_types(model: &GraphModel) -> Vec<Option<Vec<WgslType>>> {
    enum_of(model)
        .variants
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.types)
        .collect()
}

#[test]
fn variants_owned_by_types_are_offered_only_while_their_variable_has_one_of_their_types() {
    assert_eq!(offered(&variant_model(WgslType::Vec2f, 1.0)), ["A", "B"]);
    assert_eq!(offered(&variant_model(WgslType::Vec3f, 1.0)), ["B", "C"]);
}

#[test]
fn variants_not_offered_for_a_type_are_not_picked_the_first_offered_is() {
    // C, under which "k" is, isn't offered for vec2f: A is picked.
    assert_eq!(hidden(&variant_model(WgslType::Vec2f, 2.0)), ["k"]);
    assert!(hidden(&variant_model(WgslType::Vec3f, 2.0)).is_empty());
}

#[test]
fn variants_compile_as_the_first_offered_with_other_types() {
    let code = |ty| shader(&variant_model(ty, 2.0));
    assert!(code(WgslType::Vec2f).contains("let v_m = v_a;"));
    assert!(code(WgslType::Vec3f).contains("let v_m = v_c;"));
}

#[test]
fn variants_follow_their_variables_types() {
    let (v2, v3) = (WgslType::Vec2f, WgslType::Vec3f);
    let mut model = variant_model(v2, 1.0);
    model.set_variant_offered("def", "o", 1, v2, false);
    assert_eq!(
        variant_types(&model),
        [Some(vec![v2]), Some(vec![v3]), Some(vec![v3])]
    );
    model.remove_type_variable("def", "T");
    assert_eq!(variant_types(&model), [None, None, None]);
    assert_eq!(enum_of(&model).variants_by, None);
}

#[test]
fn variants_are_offered_for_all_of_a_type_no_variant_would_have() {
    let (v2, v3, v4) = (WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f);
    let mut model = variant_model(v2, 1.0);
    model.set_variant_offered("def", "o", 1, v2, false);
    model.set_type_variable("def", "T", None, Some(&[v2, v3, v4]));
    assert_eq!(
        variant_types(&model),
        [Some(vec![v2, v4]), Some(vec![v3, v4]), Some(vec![v3, v4])]
    );
}

#[test]
fn variants_keep_a_variant_for_each_type() {
    let (v2, v3) = (WgslType::Vec2f, WgslType::Vec3f);
    let mut model = variant_model(v2, 1.0);
    model.set_variant_offered("def", "o", 1, v3, false);
    model.set_variant_offered("def", "o", 2, v3, false);
    assert_eq!(
        variant_types(&model),
        [Some(vec![v2]), Some(vec![v2]), Some(vec![v3])]
    );
    // Removing the only variant for vec3f leaves all offering it.
    let kept: Vec<EnumVariant> = enum_of(&model)
        .variants
        .unwrap_or_default()
        .into_iter()
        .filter(|v| v.value != 2)
        .collect();
    model.set_handle_variants("def", "o", Some(kept));
    assert_eq!(variant_types(&model), [None, None]);
}

#[test]
fn variants_need_none_for_the_types_their_enum_isnt_shown_for() {
    let (v2, v3, v4) = (WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f);
    let mut model = variant_model(v2, 1.0);
    model.set_type_variable("def", "T", None, Some(&[v2, v3, v4]));
    model.set_handle_type_owner(
        "def",
        "o",
        Some(TypeOwner {
            variable: "T".to_owned(),
            types: vec![v2, v3],
        }),
    );
    model.set_variant_offered("def", "o", 1, v3, false);
    assert_eq!(
        variant_types(&model),
        [Some(vec![v2]), Some(vec![v2]), Some(vec![v3])]
    );
}

#[test]
fn variants_are_all_offered_again_without_the_enums_variable() {
    let mut model = variant_model(WgslType::Vec2f, 1.0);
    model.set_variants_by("def", "o", None);
    assert_eq!(enum_of(&model).variants_by, None);
    assert_eq!(variant_types(&model), [None, None, None]);
    assert_eq!(offered(&model), ["A", "B", "C"]);
    model.set_variants_by("def", "o", Some("T"));
    assert_eq!(enum_of(&model).variants_by.as_deref(), Some("T"));
    assert_eq!(offered(&model), ["A", "B", "C"]);
}

#[test]
fn checkbox_enums_stay_checkboxes_when_their_variants_are_renamed() {
    let mut model = type_owner_model(WgslType::Vec3f);
    let handle = model
        .add_handle(
            "def",
            GroupSide::Input,
            WgslType::Bool.into(),
            Some(GraphModel::toggle_variants()),
            None,
        )
        .expect("added");
    assert_eq!(handle.ty, HandleType::Fixed(WgslType::Bool));
    assert_eq!(handle.label, "Checkbox 1");
    model.set_handle_variants(
        "def",
        &handle.id,
        Some(vec![EnumVariant::new(0, "No"), EnumVariant::new(1, "Yes")]),
    );
    let input = model
        .project()
        .document("def")
        .and_then(|d| d.inputs.iter().find(|h| h.id == handle.id).cloned())
        .expect("kept");
    assert_eq!(input.ty, HandleType::Fixed(WgslType::Bool));
    let labels: Vec<String> = input
        .variants
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.label)
        .collect();
    assert_eq!(labels, ["No", "Yes"]);
}

#[test]
fn compiled_inputs_are_integers() {
    let mut model = type_owner_model(WgslType::Vec3f);
    let id = model
        .add_handle("def", GroupSide::Input, WgslType::I32.into(), None, None)
        .expect("added")
        .id;
    let compiled = |m: &GraphModel| {
        m.project()
            .document("def")
            .and_then(|d| d.inputs.iter().find(|h| h.id == id)?.compiled)
    };
    model.set_handle_compiled("def", &id, true);
    assert_eq!(compiled(&model), Some(true));
    model.set_handle_type("def", GroupSide::Input, &id, WgslType::F32.into());
    assert_eq!(compiled(&model), None);
    model.set_handle_compiled("def", &id, true);
    assert_eq!(compiled(&model), None);
}

#[test]
fn case_handles_name_their_variants_value() {
    assert_eq!(case_handle(5), "case-5");
}

#[test]
fn a_file_with_negative_or_repeated_variant_values_is_rejected() {
    for bad in [
        vec![EnumVariant::new(-1, "Below"), EnumVariant::new(1, "One")],
        vec![EnumVariant::new(1, "One"), EnumVariant::new(1, "Again")],
    ] {
        let def = GraphDocument {
            inputs: vec![enum_input("mode", "Mode", bad)],
            ..GraphDocument::default()
        };
        let model = two_documents(Vec::new(), &[], def);
        assert!(GraphProject::from_json(&model.project().to_json()).is_err());
    }
    let def = GraphDocument {
        inputs: vec![enum_input("mode", "Mode", variants())],
        ..GraphDocument::default()
    };
    let model = two_documents(Vec::new(), &[], def);
    assert!(GraphProject::from_json(&model.project().to_json()).is_ok());
}
