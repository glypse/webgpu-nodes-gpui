use crate::testing::TestGraph;
use crate::types::WgslType;

#[test]
fn a_family_resolves_to_the_widest_linked_type() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let time = g.add("time");
    let add = g.add("add");
    g.link((&time, "seconds"), (&add, "operands"));
    assert_eq!(g.model.resolve_node_output_type(&add), WgslType::F32);
    g.link((&uv, "uv"), (&add, "operands"));
    assert_eq!(g.model.resolve_node_output_type(&add), WgslType::Vec2f);
}

#[test]
fn a_forced_type_wins_over_links() {
    let mut g = TestGraph::new();
    let time = g.add("time");
    let sin = g.add("sin");
    g.link((&time, "seconds"), (&sin, "e"));
    g.force_type(&sin, WgslType::Vec3f);
    assert_eq!(g.model.resolve_node_output_type(&sin), WgslType::Vec3f);
    assert_eq!(
        g.model.resolve_inferred_input_type(&sin, "e"),
        WgslType::F32
    );
}

#[test]
fn separate_vector_has_as_many_outputs_as_components() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let sep = g.add("separate-vector");
    g.link((&uv, "uv"), (&sep, "v"));
    assert_eq!(g.model.resolve_node_outputs(&sep).len(), 2);
}

#[test]
fn comparison_gives_bools_of_the_same_width() {
    let mut g = TestGraph::new();
    let uv = g.add("uv");
    let cmp = g.add("compare");
    g.link((&uv, "uv"), (&cmp, "a"));
    assert_eq!(g.model.resolve_node_output_type(&cmp), WgslType::Vec2b);
}

#[test]
fn declare_inputs_follow_its_type() {
    let mut g = TestGraph::new();
    let d = g.add("declare");
    g.force_type(&d, WgslType::Vec3i);
    assert_eq!(g.model.resolve_node_inputs(&d).len(), 3);
    assert_eq!(g.model.resolve_input_type(&d, "x"), WgslType::I32);
}
