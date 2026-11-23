//! Evaluates a graph on the CPU, for the nodes that have a CPU
//! implementation: graphs without a GPU, for tests and tools.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use thiserror::Error;

use crate::graph::{ColorSpace, GROUP_INPUT, GraphDocument, GraphModel, GraphNode, NODE_GROUP};
use crate::nodes::cpu::{coerce, stored_value};
use crate::types::{CpuCtx, CpuInputs, ResolvedNodeTypes, TypeBindings, Value, WgslType};
use crate::zones::{
    ITERATION_HANDLE, ITERATIONS_HANDLE, REPEAT_INPUT, REPEAT_OUTPUT, zone_items,
    zone_max_iterations,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct EvalError(pub String);

/// What a graph is evaluated for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvalOptions {
    /// The pixel's coordinates (origin bottom-left, as the UV node gives them).
    pub uv: [f64; 2],
    /// The gamut of the target drawn to.
    pub color_space: ColorSpace,
    /// The size of the canvas drawn to, in pixels.
    pub resolution: [f64; 2],
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self {
            uv: [0.5, 0.5],
            color_space: ColorSpace::Srgb,
            resolution: [1.0, 1.0],
        }
    }
}

struct ZoneState {
    iteration: usize,
    state: BTreeMap<String, Value>,
}

/// One group instance's document as computed.
struct Frame<'m> {
    doc: &'m GraphDocument,
    bindings: TypeBindings,
    group_inputs: BTreeMap<String, Value>,
    /// The current state of the repeat zones being run, by Repeat Input id.
    zones: BTreeMap<String, ZoneState>,
    /// The final state of the zones run in this frame, by Repeat Output id.
    finals: RefCell<BTreeMap<String, BTreeMap<String, Value>>>,
    computed: RefCell<HashMap<String, Value>>,
    /// Outputs being computed, to stop a cycle.
    visiting: RefCell<HashSet<String>>,
}

impl<'m> Frame<'m> {
    fn new(doc: &'m GraphDocument, bindings: TypeBindings) -> Self {
        Self {
            doc,
            bindings,
            group_inputs: BTreeMap::new(),
            zones: BTreeMap::new(),
            finals: RefCell::default(),
            computed: RefCell::default(),
            visiting: RefCell::default(),
        }
    }
}

struct Evaluator<'m> {
    model: &'m GraphModel,
    options: EvalOptions,
    failure: RefCell<Option<EvalError>>,
}

/// Compute an output of a node of the root document on the CPU, for the node
/// types having a CPU implementation. An error on any other node type.
pub fn evaluate(model: &GraphModel, node: &str, handle: &str) -> Result<Value, EvalError> {
    evaluate_with(model, node, handle, &EvalOptions::default())
}

pub fn evaluate_with(
    model: &GraphModel,
    node: &str,
    handle: &str,
    options: &EvalOptions,
) -> Result<Value, EvalError> {
    let evaluator = Evaluator {
        model,
        options: *options,
        failure: RefCell::default(),
    };
    let frame = Frame::new(model.root(), TypeBindings::new());
    let value = evaluator.output(&frame, node, handle);
    match evaluator.failure.take() {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

struct NodeInputs<'e, 'm> {
    evaluator: &'e Evaluator<'m>,
    frame: &'e Frame<'m>,
    node: &'e GraphNode,
}

impl CpuInputs for NodeInputs<'_, '_> {
    fn input(&self, id: &str) -> Value {
        self.evaluator.input(self.frame, self.node, id)
    }

    fn operands(&self, id: &str) -> Vec<Value> {
        self.evaluator.operands(self.frame, self.node, id)
    }

    fn is_present(&self, id: &str) -> bool {
        self.frame
            .doc
            .edges_into(&self.node.id)
            .any(|e| e.target_handle == id)
    }
}

impl<'m> Evaluator<'m> {
    fn fail(&self, message: impl Into<String>) -> Value {
        self.failure
            .borrow_mut()
            .get_or_insert(EvalError(message.into()));
        vec![0.0]
    }

    fn types_in(&self, frame: &Frame<'m>, node: &GraphNode) -> std::sync::Arc<ResolvedNodeTypes> {
        self.model
            .resolver()
            .resolve(self.model.project(), node, &frame.bindings)
    }

    /// The frame computing the group node `node` of `frame`'s document, its inputs from there.
    fn group_frame(&self, frame: &Frame<'m>, node: &GraphNode) -> Option<Frame<'m>> {
        let project = self.model.project();
        let doc = project.document(node.definition_ref()?)?;
        let descriptor = self.model.resolve_descriptor(node);
        let resolved = self.types_in(frame, node);
        let mut inner = Frame::new(doc, resolved.bindings.clone().unwrap_or_default());
        for input in &descriptor.inputs {
            let ty = resolved.input_type(&input.id).unwrap_or(WgslType::F32);
            if !ty.is_texture() {
                inner
                    .group_inputs
                    .insert(input.id.clone(), self.input(frame, node, &input.id));
            }
        }
        Some(inner)
    }

    fn input(&self, frame: &Frame<'m>, node: &GraphNode, input_id: &str) -> Value {
        let types = self.types_in(frame, node);
        let ty = types
            .input_type(input_id)
            .unwrap_or(WgslType::F32)
            .value_type();
        if let Some(edge) = frame
            .doc
            .edges_into(&node.id)
            .find(|e| e.target_handle == input_id)
        {
            return coerce(&self.output(frame, &edge.source, &edge.source_handle), ty);
        }
        let descriptor = self.model.resolve_descriptor(node);
        let stored = node
            .data
            .values
            .get(input_id)
            .or_else(|| descriptor.default_value(input_id));
        stored_value(stored, ty)
    }

    fn operands(&self, frame: &Frame<'m>, node: &GraphNode, input_id: &str) -> Vec<Value> {
        let types = self.types_in(frame, node);
        let ty = types
            .input_type(input_id)
            .unwrap_or(WgslType::F32)
            .value_type();
        frame
            .doc
            .edges_into(&node.id)
            .filter(|e| e.target_handle == input_id)
            .map(|e| coerce(&self.output(frame, &e.source, &e.source_handle), ty))
            .collect()
    }

    fn output(&self, frame: &Frame<'m>, node_id: &str, handle: &str) -> Value {
        let key = format!("{node_id}:{handle}");
        if let Some(done) = frame.computed.borrow().get(&key) {
            return done.clone();
        }
        if !frame.visiting.borrow_mut().insert(key.clone()) {
            return self.fail(format!("\"{node_id}\" depends on itself"));
        }
        let value = self.compute(frame, node_id, handle);
        frame.visiting.borrow_mut().remove(&key);
        frame.computed.borrow_mut().insert(key, value.clone());
        value
    }

    fn compute(&self, frame: &Frame<'m>, node_id: &str, handle: &str) -> Value {
        let Some(node) = frame.doc.node(node_id) else {
            return self.fail(format!("no node \"{node_id}\""));
        };
        match node.node_type.as_str() {
            GROUP_INPUT => {
                return frame.group_inputs.get(handle).cloned().unwrap_or_else(|| {
                    self.fail(format!("no value for group input \"{handle}\""))
                });
            }
            NODE_GROUP => {
                let Some(inner) = self.group_frame(frame, node) else {
                    return self.fail(format!("no definition for \"{node_id}\""));
                };
                return match inner.doc.output_node_of(handle) {
                    Some(out) => self.input(&inner, out, handle),
                    None => self.fail("no group output"),
                };
            }
            REPEAT_INPUT => {
                let Some(zone) = frame.zones.get(node_id) else {
                    return self.fail(format!("repeat input \"{node_id}\" read outside its zone"));
                };
                if handle == ITERATION_HANDLE {
                    return vec![zone.iteration as f64];
                }
                return zone.state.get(handle).cloned().unwrap_or_else(|| vec![0.0]);
            }
            REPEAT_OUTPUT => {
                return self
                    .run_zone(frame, node)
                    .get(handle)
                    .cloned()
                    .unwrap_or_else(|| vec![0.0]);
            }
            "resolution" => return self.options.resolution.to_vec(),
            "uv" => return self.options.uv.to_vec(),
            "display-gamut-clip" => {
                let c = self.input(frame, node, "color");
                let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
                let space = match self.options.color_space {
                    ColorSpace::Srgb => graph_math::color::ColorSpace::Srgb,
                    ColorSpace::DisplayP3 => graph_math::color::ColorSpace::DisplayP3,
                };
                let clipped = graph_math::color::clip_to_gamut(
                    graph_math::color::Rgb::new(at(0), at(1), at(2)),
                    space,
                );
                return vec![clipped.r, clipped.g, clipped.b];
            }
            _ => {}
        }
        let descriptor = self.model.resolve_descriptor(node);
        let Some(cpu) = descriptor.cpu.clone() else {
            return self.fail(format!("can't evaluate a \"{}\" node", node.node_type));
        };
        let resolved = self.types_in(frame, node);
        let input_types: BTreeMap<String, WgslType> = resolved
            .input_types
            .iter()
            .map(|(id, t)| (id.clone(), t.value_type()))
            .collect();
        let output_types: BTreeMap<String, WgslType> = resolved
            .resolved_outputs
            .iter()
            .map(|o| (o.id.clone(), o.ty.value_type()))
            .collect();
        let inputs = NodeInputs {
            evaluator: self,
            frame,
            node,
        };
        let ctx = CpuCtx {
            inputs: &inputs,
            input_types: &input_types,
            output_types: &output_types,
            output_type: resolved.output_type.value_type(),
            resolved_inputs: &resolved.resolved_inputs,
            data: &node.data,
        };
        let outputs = cpu(&ctx);
        let mut computed = frame.computed.borrow_mut();
        for (id, value) in &outputs {
            computed.insert(format!("{node_id}:{id}"), value.clone());
        }
        outputs.get(handle).cloned().unwrap_or_else(|| vec![0.0])
    }

    /// Run a repeat zone to its end, as its loop does: every next value from the current ones.
    fn run_zone(&self, frame: &Frame<'m>, output: &GraphNode) -> BTreeMap<String, Value> {
        if let Some(done) = frame.finals.borrow().get(&output.id) {
            return done.clone();
        }
        let Some(start) =
            frame.doc.nodes.iter().find(|n| {
                n.is(REPEAT_INPUT) && n.data.paired_output.as_deref() == Some(&output.id)
            })
        else {
            self.fail(format!("no Repeat Input for \"{}\"", output.id));
            return BTreeMap::new();
        };
        let items = zone_items(Some(output));
        let iterations = self
            .input(frame, start, ITERATIONS_HANDLE)
            .first()
            .copied()
            .unwrap_or(0.0);
        let count = iterations
            .trunc()
            .max(0.0)
            .min(f64::from(zone_max_iterations(start))) as usize;
        let mut state: BTreeMap<String, Value> = items
            .iter()
            .map(|i| (i.id.clone(), self.input(frame, start, &i.id)))
            .collect();
        for iteration in 0..count {
            let inner = Frame {
                doc: frame.doc,
                bindings: frame.bindings.clone(),
                group_inputs: frame.group_inputs.clone(),
                zones: frame
                    .zones
                    .iter()
                    .map(|(id, z)| {
                        (
                            id.clone(),
                            ZoneState {
                                iteration: z.iteration,
                                state: z.state.clone(),
                            },
                        )
                    })
                    .chain([(
                        start.id.clone(),
                        ZoneState {
                            iteration,
                            state: state.clone(),
                        },
                    )])
                    .collect(),
                finals: RefCell::default(),
                computed: RefCell::default(),
                visiting: RefCell::default(),
            };
            state = items
                .iter()
                .map(|i| (i.id.clone(), self.input(&inner, output, &i.id)))
                .collect();
        }
        frame
            .finals
            .borrow_mut()
            .insert(output.id.clone(), state.clone());
        state
    }
}
