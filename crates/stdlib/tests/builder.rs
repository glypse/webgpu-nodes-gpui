use shader_graph::graph::FrameTint;
use shader_graph::types::WgslType;
use stdlib::build::{BuiltGroup, build_groups};
use stdlib::builder::{
    CustomWgsl, ErrorKind, Frame, GroupBuilder, GroupSpec, Input, Output, RepeatItem, Variant,
};
use stdlib::{json, layout, validate};

const SINK: &str = "test-sink";
const CALLER: &str = "test-caller";

/// Uses every kind of thing a group may describe.
fn sink() -> GroupSpec {
    GroupSpec::new(
        SINK,
        "Sink",
        "vector",
        &["vector", "mode", "amount", "flag"],
        &["value", "total", "doubled"],
        |g| {
            let t = g.type_variable("t", "T", &[WgslType::Vec3f, WgslType::F32]);
            let vector = g.input("vector", Input::new(&t).label("Vector"));
            let mode = g.enum_input(
                "mode",
                Input::enumeration([Variant::new(0, "Plain"), Variant::new(1, "Scaled")], 0),
            );
            let amount = g.input(
                "amount",
                Input::new(WgslType::F32)
                    .default(0.5)
                    .min(0.0)
                    .max(1.0)
                    .owner(&mode, 1)
                    .type_owner(&t, &[WgslType::F32]),
            );
            let flag = g.enum_input("flag", Input::toggle(true));
            let amount = g.wireless("Amount", amount);
            let picked = g.frame(Frame::new("Outer").note("Both ways."), || {
                g.frame(Frame::new("Inner").tint(FrameTint::Blue), || {
                    g.match_on(&mode)
                        .case(0, vector)
                        .case(1, g.op("mul", (vector, amount)))
                        .add()
                })
            });
            let flagged = g.op("switch", (&flag, picked, g.constant(WgslType::F32, 0.0)));
            let total = g.repeat(
                4,
                vec![RepeatItem::new("Sum", WgslType::F32, 0.0)],
                |body| {
                    let index = g.op("to-float", body.iteration());
                    body.set("Sum", g.op("add", (body.state("Sum"), index)));
                },
            );
            let sum = total.get("Sum");
            let table = g
                .index_switch(g.constant(WgslType::I32, 1), WgslType::F32)
                .case(0, 0.25)
                .case(1, sum)
                .add();
            let custom = g.custom_wgsl(
                CustomWgsl::new("y = x * 2.0;")
                    .input("x", WgslType::F32)
                    .output("y", WgslType::F32),
                [("x", table.into())],
            );
            g.output("value", flagged, Output::new(&t));
            g.output("total", sum, WgslType::F32);
            g.output("doubled", custom.out("y"), WgslType::F32);
        },
    )
}

fn caller() -> GroupSpec {
    GroupSpec::new(CALLER, "Caller", "vector", &["vector"], &["total"], |g| {
        let vector = g.input("vector", WgslType::Vec3f);
        let called = g
            .call(SINK)
            .input("vector", vector)
            .input("amount", 2.0)
            .add();
        g.output("total", called.out("total"), WgslType::F32);
    })
}

fn errors_of(specs: &[GroupSpec]) -> Vec<String> {
    match build_groups(specs) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.0.iter().map(ToString::to_string).collect(),
    }
}

fn single(
    inputs: &'static [&'static str],
    outputs: &'static [&'static str],
    build: fn(&GroupBuilder<'_>),
) -> GroupSpec {
    GroupSpec::new("test-single", "Single", "vector", inputs, outputs, build)
}

fn built() -> Vec<BuiltGroup> {
    build_groups(&[caller(), sink()]).expect("the groups build")
}

#[test]
fn every_kind_of_node_compiles_to_valid_wgsl() {
    let groups = built();
    let problems = validate::validate_groups(&groups, &[SINK, CALLER]);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_called_group_is_built_first_and_depended_on() {
    let groups = built();
    let ids: Vec<&str> = groups.iter().map(|g| g.spec.id).collect();
    assert_eq!(ids, [SINK, CALLER]);
    assert_eq!(groups[1].dependencies, [groups[0].reference.clone()]);
    assert!(groups[0].reference.starts_with("test-sink@"));
}

#[test]
fn building_twice_gives_identical_files() {
    let render = |groups: &[BuiltGroup]| -> Vec<String> {
        groups
            .iter()
            .map(|g| json::render(g).expect("renders"))
            .collect()
    };
    assert_eq!(render(&built()), render(&built()));
}

#[test]
fn the_text_written_reads_back_as_the_value_described() {
    for group in built() {
        let text = json::render(&group).expect("renders");
        let read: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(read, json::file_value(&group).expect("a value"));
        let version = shader_graph::library::version_of(&read["definition"]);
        assert_eq!(group.reference, format!("{}@{version}", group.spec.id));
    }
}

#[test]
fn a_changed_group_changes_its_callers_version() {
    let before = built();
    let changed = GroupSpec::new(SINK, "Sink", "vector", &["vector"], &["total"], |g| {
        let vector = g.input("vector", WgslType::F32);
        g.output("total", vector, WgslType::F32);
    });
    let caller = GroupSpec::new(CALLER, "Caller", "vector", &["vector"], &["total"], |g| {
        let vector = g.input("vector", WgslType::F32);
        let called = g.call(SINK).input("vector", vector).add();
        g.output("total", called.out("total"), WgslType::F32);
    });
    let after = build_groups(&[changed, caller]).expect("builds");
    assert_ne!(before[1].reference, after[1].reference);
}

#[test]
fn the_interface_is_described_as_the_editor_stores_it() {
    let groups = built();
    let sink = &groups[0].definition;
    let mode = sink
        .inputs
        .iter()
        .find(|h| h.label == "mode")
        .expect("mode");
    assert_eq!(mode.variants.as_ref().map(Vec::len), Some(2));
    let amount = sink
        .inputs
        .iter()
        .find(|h| h.label == "amount")
        .expect("amount");
    assert_eq!(
        amount.owner.as_ref().map(|o| (o.enum_id.as_str(), o.value)),
        Some((mode.id.as_str(), 1))
    );
    assert_eq!((amount.min, amount.max), (Some(0.0), Some(1.0)));
    assert!(amount.type_owner.is_some());
    assert_eq!(
        sink.defaults.get(&amount.id).map(|v| v.component(0)),
        Some(0.5)
    );
    assert_eq!(sink.type_variables.len(), 1);
}

#[test]
fn groups_are_laid_out_without_changing_their_nodes() {
    let mut groups = built();
    layout::layout_headless(&mut groups, &[SINK, CALLER]).expect("lays out");
    let positions: Vec<_> = groups[0]
        .definition
        .nodes
        .iter()
        .map(|n| n.position)
        .collect();
    assert!(positions.iter().any(|p| p.x > 0.0));
}

#[test]
fn an_unknown_node_type_is_reported() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.op("no-such-node", 1);
        g.output("out", node, WgslType::F32);
    })]);
    assert_eq!(errors, ["test-single: no node type \"no-such-node\""]);
}

#[test]
fn nodes_with_a_method_of_their_own_are_refused() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.op("match", 1);
        g.output("out", node, WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("`match_on`")),
        "{errors:?}"
    );
}

#[test]
fn operands_beyond_a_nodes_inputs_are_reported() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.op("abs", (1, 2));
        g.output("out", node, WgslType::F32);
    })]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("takes 1 operands, given 2")),
        "{errors:?}"
    );
}

#[test]
fn an_unknown_input_lists_the_known_ones() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.node("compare").input("c", 1).add();
        g.output("out", node, WgslType::Bool);
    })]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("no input \"c\"") && e.contains("a, b")),
        "{errors:?}"
    );
}

#[test]
fn an_unknown_output_is_reported() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.op("separate-vector", g.vec(WgslType::Vec3f, (1, 2, 3)));
        g.output("out", node.out("q"), WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("no output \"q\"")),
        "{errors:?}"
    );
}

#[test]
fn a_setting_is_checked_against_its_options() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.node("compare").args((1, 2)).setting("op", "~~").add();
        g.output("out", node, WgslType::Bool);
        g.node("compare").setting("mode", "x").add();
    })]);
    assert!(
        errors.iter().any(|e| e.contains("no option \"~~\"")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("no setting \"mode\"")),
        "{errors:?}"
    );
}

#[test]
fn a_type_is_forced_only_on_a_family_the_node_has() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let node = g.node("declare").typed("nope", WgslType::F32).add();
        g.output("out", node, WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("no type family \"nope\"")),
        "{errors:?}"
    );
}

#[test]
fn declared_and_added_interfaces_must_match() {
    let errors = errors_of(&[single(&["a"], &["out"], |g| {
        g.output("other", 1, WgslType::F32);
    })]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("declares input \"a\" but adds none")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("declares output \"out\" but adds none")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("adds output \"other\" it doesn't declare")),
        "{errors:?}"
    );
}

#[test]
fn an_input_or_output_added_twice_is_reported() {
    let errors = errors_of(&[single(&["a"], &["out"], |g| {
        g.input("a", WgslType::F32);
        g.input("a", WgslType::F32);
        g.output("out", 1, WgslType::F32);
        g.output("out", 1, WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("input \"a\" added twice")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("output \"out\" added twice")),
        "{errors:?}"
    );
}

#[test]
fn a_call_checks_the_group_and_its_keys() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        g.call("test-nothing").add();
        g.output("out", 1, WgslType::F32);
    })]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("no group described as \"test-nothing\"")),
        "{errors:?}"
    );

    let errors = errors_of(&[
        single(&[], &["out"], |g| {
            let called = g.call(SINK).input("nope", 1).add();
            g.output("out", called.out("total"), WgslType::F32);
        }),
        sink(),
    ]);
    assert!(
        errors.iter().any(|e| e.contains("no input \"nope\"")),
        "{errors:?}"
    );
}

#[test]
fn a_group_calling_itself_is_reported() {
    let errors = errors_of(&[single(&[], &["out"], |g| {
        let me = g.call("test-single").add();
        g.output("out", me.out("out"), WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("calls itself")),
        "{errors:?}"
    );
}

#[test]
fn a_match_needs_a_case_for_every_variant() {
    let errors = errors_of(&[single(&["mode"], &["out"], |g| {
        let mode = g.enum_input("mode", Input::enumeration([(0, "A"), (1, "B")], 0));
        let picked = g.match_on(&mode).case(0, 1.0).add();
        g.output("out", picked, WgslType::F32);
    })]);
    assert!(
        errors.iter().any(|e| e.contains("no case for B")),
        "{errors:?}"
    );
}

#[test]
fn a_partial_match_may_leave_variants_out() {
    let errors = errors_of(&[single(&["mode"], &["out"], |g| {
        let mode = g.enum_input("mode", Input::enumeration([(0, "A"), (1, "B")], 0));
        let picked = g.match_on(&mode).case(0, 1.0).partial().add();
        g.output("out", picked, WgslType::F32);
    })]);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn the_error_kinds_name_what_was_wrong() {
    let errors = build_groups(&[single(&[], &["out"], |g| {
        g.output("out", g.op("nope", 1), WgslType::F32);
    })])
    .expect_err("fails");
    assert!(matches!(errors.0[0].kind, ErrorKind::UnknownNodeType(_)));
}

#[test]
fn wgsl_that_does_not_compile_is_reported() {
    let groups = build_groups(&[single(&[], &["out"], |g| {
        let custom = g.custom_wgsl(
            CustomWgsl::new("y = nonsense;").output("y", WgslType::F32),
            [],
        );
        g.output("out", custom.out("y"), WgslType::F32);
    })])
    .expect("builds");
    assert!(!validate::validate_groups(&groups, &["test-single"]).is_empty());
}

#[test]
fn a_section_is_listed_without_the_helpers_section() {
    let spec =
        GroupSpec::new("test-section", "Section", "texture", &[], &[], |_| {}).section(&["Blur"]);
    assert_eq!(spec.section, vec!["Blur"]);
    assert_eq!(spec.search_section, None);
}

/// A version of a one-input group, `extra` adding a node before the rest.
fn versioned(extra: bool) -> GroupSpec {
    let build: fn(&GroupBuilder<'_>) = if extra {
        |g| {
            let x = g.input("x", Input::new(WgslType::F32));
            let doubled = g.op("mul", (x, 2.0)).first();
            g.output("y", g.op("add", (doubled, 1.0)), Output::new(WgslType::F32));
        }
    } else {
        |g| {
            let x = g.input("x", Input::new(WgslType::F32));
            g.output("y", g.op("add", (x, 1.0)), Output::new(WgslType::F32));
        }
    };
    GroupSpec::new("test-versioned", "Versioned", "math", &["x"], &["y"], build)
}

// A project indexes nodes by id across its documents and may hold two versions
// of one group: ids derived from the order nodes are added alone would repeat.
#[test]
fn two_versions_of_a_group_share_no_node_or_link_id() {
    let build = |extra| {
        build_groups(&[versioned(extra)])
            .expect("the group builds")
            .remove(0)
    };
    let (old, new) = (build(false), build(true));
    let ids = |g: &BuiltGroup| -> std::collections::HashSet<String> {
        let d = &g.definition;
        d.nodes.iter().map(|n| n.id.clone()).chain(d.edges.iter().map(|e| e.id.clone())).collect()
    };
    assert!(ids(&old).is_disjoint(&ids(&new)));
    assert_eq!(old.definition.inputs, new.definition.inputs);
    assert_eq!(old.definition.outputs, new.definition.outputs);
    assert_eq!(build(true).definition, new.definition);
}
