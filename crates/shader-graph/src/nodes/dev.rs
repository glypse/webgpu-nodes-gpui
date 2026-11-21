//! Developer-tool nodes whose shader fails on purpose, to exercise the error
//! reporting. They are registered always but listed only with developer tools.

use crate::types::{FLOAT_TYPES, NodeCategory, NodeInputDef, NodeOutputDef, WgslType};

use super::builders::*;

type D = crate::types::NodeDescriptor;

pub(super) fn dev_nodes() -> Vec<D> {
    let invalid = D::new("test-invalid-wgsl", "⚠ Test: Bad WGSL", NodeCategory::Misc)
        .dev()
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .wgsl(|ctx| vec![format!("let {} = this_is_not_valid_wgsl;", ctx.var_name)]);
    let runtime_error = D::new("test-runtime-error", "⚠ Test: Runtime Error", NodeCategory::Misc)
        .dev()
        .inputs(vec![NodeInputDef::new("e", "e", FLOAT_TYPES)])
        .outputs(vec![NodeOutputDef::new("result", "result", WgslType::F32)])
        .defaults(float_values([("e", 0.0)]))
        .wgsl(|ctx| {
            let e = ctx.promote_input("e");
            vec![format!("let {} = {e} / f32(0.0);", ctx.var_name)]
        });
    vec![invalid, runtime_error]
}

#[cfg(test)]
mod tests {
    use crate::nodes::registered;
    use crate::testing::{TestGraph, validate_wgsl};
    use crate::types::WgslType;

    /// The program of a graph whose output takes `node`'s result.
    fn program_using(node_type: &str) -> String {
        let mut g = TestGraph::new();
        let node = g.add(node_type);
        let to_color = g.add("declare");
        g.force_type(&to_color, WgslType::Vec4f);
        g.link((&node, "result"), (&to_color, "x"));
        let out = g.output();
        g.link((&to_color, "value"), (&out, "color"));
        g.program().program.code
    }

    #[test]
    fn the_dev_nodes_are_flagged_and_registered() {
        for id in ["test-invalid-wgsl", "test-runtime-error"] {
            let d = registered(id).expect("registered");
            assert!(d.dev, "{id} is a developer-tool node");
            assert!(!d.hidden, "{id} is not hidden, only gated by developer tools");
        }
    }

    #[test]
    fn the_invalid_node_makes_a_shader_naga_rejects() {
        let code = program_using("test-invalid-wgsl");
        assert!(code.contains("this_is_not_valid_wgsl"));
        assert!(std::panic::catch_unwind(|| validate_wgsl(&code)).is_err());
    }

    #[test]
    fn the_runtime_error_node_makes_a_valid_shader_that_divides_by_zero() {
        let code = program_using("test-runtime-error");
        assert!(code.contains("/ f32(0.0)"));
        validate_wgsl(&code);
    }
}
