//! Helpers for building graphs and checking what they generate.

use crate::codegen::{Program, ShaderOptions};
use crate::graph::{Connection, FRAG_OUTPUT, GraphModel, Point};
use crate::types::{InputValue, WgslType};

/// A model with helpers to build a graph tersely.
pub struct TestGraph {
    pub model: GraphModel,
}

impl Default for TestGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl TestGraph {
    pub fn new() -> Self {
        Self { model: GraphModel::default() }
    }

    pub fn output(&self) -> String {
        self.model
            .root()
            .nodes
            .iter()
            .find(|n| n.is(FRAG_OUTPUT))
            .expect("the root has an output")
            .id
            .clone()
    }

    pub fn add(&mut self, node_type: &str) -> String {
        self.model.add_node(node_type, Point::default()).unwrap_or_else(|| panic!("adds {node_type}"))
    }

    pub fn set(&mut self, node: &str, input: &str, value: impl Into<InputValue>) -> &mut Self {
        self.model.set_input_value(node, input, Some(value.into()));
        self
    }

    pub fn force_type(&mut self, node: &str, ty: WgslType) -> &mut Self {
        self.model.set_node_type(node, Some(ty));
        self
    }

    /// Link `from.output` to `to.input`, which must be valid.
    pub fn link(&mut self, from: (&str, &str), to: (&str, &str)) -> &mut Self {
        let c = Connection::new(from.0, from.1, to.0, to.1);
        assert!(self.model.is_valid_connection(&c), "invalid link {c:?}");
        assert!(self.model.connect(c));
        self
    }

    /// An output of a node, computed on the CPU.
    pub fn eval(&self, node: &str, handle: &str) -> Vec<f64> {
        crate::evaluate::evaluate(&self.model, node, handle).expect("evaluates")
    }

    pub fn program(&self) -> Program {
        self.model.generate().expect("generates")
    }

    pub fn literal_program(&self) -> Program {
        self.model.generate_with(&ShaderOptions::default()).expect("generates")
    }

    /// The program's WGSL, validated by naga.
    pub fn wgsl(&self) -> String {
        let code = self.program().program.code;
        validate_wgsl(&code);
        code
    }
}

/// Panics with the diagnostics if `code` isn't valid WGSL.
pub fn validate_wgsl(code: &str) {
    let module = naga::front::wgsl::parse_str(code)
        .unwrap_or_else(|e| panic!("{}\n{code}", e.emit_to_string(code)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}\n{code}"));
}
