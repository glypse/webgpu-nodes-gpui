use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::Rc;

use super::analysis::FrameValueAnalysis;
use crate::assets::frame_interval;
use crate::codegen::{CodegenGraph, UniformValue, ident_suffix, uniform_name, uniform_value};
use crate::graph::{GROUP_INPUT, NODE_GROUP, REROUTE, ROOT_ID};
use crate::nodes::cpu::{coerce, stored_value};
use crate::nodes::{
    TEXTURE_SIZE, TEXTURE_SIZE_LEVEL, VIDEO, VIDEO_DURATION, VIDEO_INTERVAL, is_video_metadata,
    resolve_descriptor,
};
use crate::passes::{FrameArg, FrameSizing, Size, TextureSource, TextureSources};
use crate::types::{
    CpuCtx, CpuFn, CpuInputs, NodeData, NodeInputDef, Value, WgslType, input_control,
};
use crate::zones::{
    ITERATION_HANDLE, ITERATIONS_HANDLE, REPEAT_INPUT, ZoneKind, zone_items, zone_kind_of,
    zone_max_iterations,
};

/// Where a frame program reads what only the runtime knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameSource {
    Time,
    Delta,
    Frame,
    Canvas,
    PassSize,
    /// The iteration of a loop of the plan.
    Iteration(String),
    /// The size of a resource, and how many mip levels it has.
    TextureSize(String),
    /// An input value that's a uniform, so editing it doesn't recompile.
    Uniform(String),
}

/// An output of a step of a frame program, or one component of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRef {
    pub step: usize,
    pub output: String,
    pub component: Option<usize>,
}

/// What a step reads: a literal, an earlier step's output, a source, or a repeat zone's state.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameInput {
    Literal(Value),
    Ref(FrameRef),
    Source(FrameSource),
    Loop { id: String, item: String },
}

/// A node's CPU implementation, kept by the step computed with it.
#[derive(Clone)]
pub struct CpuHandle(pub CpuFn);

impl fmt::Debug for CpuHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CpuFn")
    }
}

impl PartialEq for CpuHandle {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A node computed by its descriptor's `cpu`.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStep {
    pub node_type: String,
    pub cpu: CpuHandle,
    pub data: NodeData,
    pub input_types: BTreeMap<String, WgslType>,
    pub output_types: BTreeMap<String, WgslType>,
    pub output_type: WgslType,
    pub resolved_inputs: Vec<NodeInputDef>,
    pub inputs: BTreeMap<String, FrameInput>,
    pub operands: BTreeMap<String, Vec<FrameInput>>,
    /// The inputs something feeds.
    pub present: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatItem {
    pub id: String,
    pub ty: WgslType,
    pub initial: FrameInput,
    pub next: FrameInput,
}

/// A repeat zone run on the CPU: its body is the steps naming `id` as their loop.
#[derive(Clone, Debug, PartialEq)]
pub struct RepeatStep {
    pub id: String,
    pub iterations: FrameInput,
    pub max: u32,
    pub items: Vec<RepeatItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepKind {
    Node(Box<NodeStep>),
    Repeat(RepeatStep),
    /// A source or loop value given as an output, `value`.
    Read(FrameInput),
    /// A texture's size at a mip level (`size`), the step from one texel to the
    /// next (`texel`) and how many levels it has (`levels`).
    Texel {
        size: FrameInput,
        level: Option<FrameInput>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameStep {
    /// The repeat step whose body it's in, which runs it instead of the program.
    pub loop_id: Option<String>,
    pub kind: StepKind,
}

/// The frame values a plan needs, as steps in order each reading only earlier ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameProgram {
    pub steps: Vec<FrameStep>,
}

pub type IterationOf<'a> = dyn Fn(&str) -> Option<f64> + 'a;
pub type TextureSizeOf<'a> = dyn Fn(&str) -> Option<(Size, Option<u32>)> + 'a;

/// What a frame program's sources are while a pass is drawn.
#[derive(Default)]
pub struct FrameEnv<'a> {
    pub time: f64,
    pub delta: f64,
    pub frame: f64,
    pub canvas: Size,
    /// The size of the pass being drawn.
    pub pass_size: Size,
    /// The current iteration of each loop of the plan, by id.
    pub iteration: Option<&'a IterationOf<'a>>,
    /// The size of a resource's texture, by id, and how many mip levels it has (one without).
    pub texture_size: Option<&'a TextureSizeOf<'a>>,
    /// The values of uniform inputs, by name.
    pub uniforms: &'a [(String, UniformValue)],
}

/// The outputs of the steps of a frame program, as evaluated.
#[derive(Clone, Debug, Default)]
pub struct FrameValues {
    pub outputs: Vec<Option<BTreeMap<String, Value>>>,
}

impl FrameValues {
    /// The value of a step's output.
    pub fn value(&self, reference: &FrameRef) -> Value {
        value_of(&self.outputs, reference)
    }

    /// A number of the program: a literal, or one component of a step's output.
    pub fn arg(&self, arg: &FrameArg) -> f64 {
        match arg {
            FrameArg::Literal(n) => *n,
            FrameArg::Frame(reference) => self.value(reference).first().copied().unwrap_or(0.0),
        }
    }
}

fn value_of(outputs: &[Option<BTreeMap<String, Value>>], reference: &FrameRef) -> Value {
    let value = outputs
        .get(reference.step)
        .and_then(Option::as_ref)
        .and_then(|o| o.get(&reference.output))
        .cloned()
        .unwrap_or_else(|| vec![0.0]);
    match reference.component {
        None => value,
        Some(c) => vec![value.get(c).copied().unwrap_or(0.0)],
    }
}

fn source_value(source: &FrameSource, env: &FrameEnv<'_>) -> Value {
    match source {
        FrameSource::Time => vec![env.time],
        FrameSource::Delta => vec![env.delta],
        FrameSource::Frame => vec![env.frame],
        FrameSource::Canvas => vec![f64::from(env.canvas.width), f64::from(env.canvas.height)],
        FrameSource::PassSize => vec![
            f64::from(env.pass_size.width),
            f64::from(env.pass_size.height),
        ],
        FrameSource::Iteration(id) => vec![env.iteration.and_then(|f| f(id)).unwrap_or(0.0)],
        FrameSource::TextureSize(id) => {
            let (size, levels) = env
                .texture_size
                .and_then(|f| f(id))
                .unwrap_or((Size::new(1, 1), None));
            vec![
                f64::from(size.width),
                f64::from(size.height),
                f64::from(levels.unwrap_or(1)),
            ]
        }
        FrameSource::Uniform(name) => env.uniforms.iter().find(|(n, _)| n == name).map_or_else(
            || vec![0.0],
            |(_, v)| v.0.iter().map(|c| f64::from(*c)).collect(),
        ),
    }
}

struct Loop {
    iteration: usize,
    state: BTreeMap<String, Value>,
}

struct Runner<'a, 'e> {
    program: &'a FrameProgram,
    env: &'a FrameEnv<'e>,
    outputs: RefCell<Vec<Option<BTreeMap<String, Value>>>>,
    loops: RefCell<HashMap<String, Loop>>,
    /// Indices of the steps run by each loop (`None` for the program), in order.
    bodies: HashMap<Option<&'a str>, Vec<usize>>,
}

struct StepInputs<'r, 'a, 'e> {
    runner: &'r Runner<'a, 'e>,
    step: &'r NodeStep,
}

impl CpuInputs for StepInputs<'_, '_, '_> {
    fn input(&self, id: &str) -> Value {
        let ty = self
            .step
            .input_types
            .get(id)
            .copied()
            .unwrap_or(WgslType::F32);
        self.step
            .inputs
            .get(id)
            .map_or_else(|| vec![0.0], |i| coerce(&self.runner.read(i), ty))
    }

    fn operands(&self, id: &str) -> Vec<Value> {
        let ty = self
            .step
            .input_types
            .get(id)
            .copied()
            .unwrap_or(WgslType::F32);
        self.step
            .operands
            .get(id)
            .map(|list| {
                list.iter()
                    .map(|i| coerce(&self.runner.read(i), ty))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_present(&self, id: &str) -> bool {
        self.step.present.iter().any(|p| p == id)
    }
}

impl Runner<'_, '_> {
    fn read(&self, input: &FrameInput) -> Value {
        match input {
            FrameInput::Literal(v) => v.clone(),
            FrameInput::Ref(r) => value_of(&self.outputs.borrow(), r),
            FrameInput::Source(s) => source_value(s, self.env),
            FrameInput::Loop { id, item } => {
                let loops = self.loops.borrow();
                let current = loops.get(id);
                if item == ITERATION_HANDLE {
                    return vec![current.map_or(0.0, |l| l.iteration as f64)];
                }
                current
                    .and_then(|l| l.state.get(item))
                    .cloned()
                    .unwrap_or_else(|| vec![0.0])
            }
        }
    }

    fn run_node(&self, step: &NodeStep) -> BTreeMap<String, Value> {
        let inputs = StepInputs { runner: self, step };
        (step.cpu.0)(&CpuCtx {
            inputs: &inputs,
            input_types: &step.input_types,
            output_types: &step.output_types,
            output_type: step.output_type,
            resolved_inputs: &step.resolved_inputs,
            data: &step.data,
        })
    }

    fn run(&self, index: usize) {
        let Some(step) = self.program.steps.get(index) else {
            return;
        };
        let outputs = match &step.kind {
            StepKind::Node(node) => self.run_node(node),
            StepKind::Read(input) => BTreeMap::from([("value".to_owned(), self.read(input))]),
            StepKind::Texel { size, level } => {
                let size = self.read(size);
                let at = |i: usize| size.get(i).copied().unwrap_or(1.0);
                let (base, tall, levels) = (at(0), at(1), at(2));
                let wanted = level.as_ref().map_or(0.0, |l| {
                    self.read(l).first().copied().unwrap_or(0.0).trunc()
                });
                let level = wanted.max(0.0).min(levels - 1.0);
                let width = (base / 2f64.powf(level)).floor().max(1.0);
                let height = (tall / 2f64.powf(level)).floor().max(1.0);
                BTreeMap::from([
                    ("size".to_owned(), vec![width, height]),
                    (
                        "texel".to_owned(),
                        vec![
                            f64::from((1.0 / width) as f32),
                            f64::from((1.0 / height) as f32),
                        ],
                    ),
                    ("levels".to_owned(), vec![levels]),
                ])
            }
            StepKind::Repeat(repeat) => {
                let wanted = self
                    .read(&repeat.iterations)
                    .first()
                    .copied()
                    .unwrap_or(0.0)
                    .trunc();
                let count = wanted.max(0.0).min(f64::from(repeat.max)) as usize;
                let mut state: BTreeMap<String, Value> = repeat
                    .items
                    .iter()
                    .map(|i| (i.id.clone(), coerce(&self.read(&i.initial), i.ty)))
                    .collect();
                for iteration in 0..count {
                    self.loops.borrow_mut().insert(
                        repeat.id.clone(),
                        Loop {
                            iteration,
                            state: state.clone(),
                        },
                    );
                    self.run_body(Some(&repeat.id));
                    state = repeat
                        .items
                        .iter()
                        .map(|i| (i.id.clone(), coerce(&self.read(&i.next), i.ty)))
                        .collect();
                }
                self.loops.borrow_mut().remove(&repeat.id);
                state
            }
        };
        if let Some(slot) = self.outputs.borrow_mut().get_mut(index) {
            *slot = Some(outputs);
        }
    }

    fn run_body(&self, loop_id: Option<&str>) {
        for &index in self.bodies.get(&loop_id).into_iter().flatten() {
            self.run(index);
        }
    }
}

/// Run a frame program: each step by its node's `cpu`, so the CPU and the GPU agree.
pub fn evaluate_frame(program: &FrameProgram, env: &FrameEnv<'_>) -> FrameValues {
    let mut bodies: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    for (index, step) in program.steps.iter().enumerate() {
        bodies
            .entry(step.loop_id.as_deref())
            .or_default()
            .push(index);
    }
    let runner = Runner {
        bodies,
        program,
        env,
        outputs: RefCell::new(vec![None; program.steps.len()]),
        loops: RefCell::default(),
    };
    runner.run_body(None);
    FrameValues {
        outputs: runner.outputs.into_inner(),
    }
}

/// What compiling a value gives: where to read it, and whether it varies with anything at runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledFrameValue {
    pub input: FrameInput,
    pub dynamic: bool,
}

fn literal(value: Value) -> CompiledFrameValue {
    CompiledFrameValue {
        input: FrameInput::Literal(value),
        dynamic: false,
    }
}

fn source(source: FrameSource) -> CompiledFrameValue {
    CompiledFrameValue {
        input: FrameInput::Source(source),
        dynamic: true,
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrameCompilerOptions {
    /// The definitions (`ROOT_ID` for the root) whose unlinked input values are uniforms.
    pub uniform_scopes: Vec<String>,
    /// Whether Texture Size may be read (a plan has to know the texture it reads).
    pub texture_size: bool,
}

#[derive(Default)]
struct State {
    steps: Vec<FrameStep>,
    dynamics: Vec<bool>,
    memo: HashMap<String, Option<CompiledFrameValue>>,
    zones: HashMap<String, usize>,
    sized: BTreeMap<String, TextureSource>,
    uniforms: Vec<(String, UniformValue)>,
}

/// Compiles the frame values of a graph into one `FrameProgram`, which
/// shaders' hoisted values and passes' sizes read their numbers from. Values
/// made only of literals are folded while compiling.
pub struct FrameCompiler<'g> {
    graph: CodegenGraph<'g>,
    pub analysis: FrameValueAnalysis<'g>,
    sources: Rc<TextureSources<'g>>,
    uniform_scopes: Vec<String>,
    texture_size: bool,
    state: RefCell<State>,
}

fn path_key(path: &[String]) -> String {
    path.join("/")
}

impl<'g> FrameCompiler<'g> {
    /// A compiler of the frame values of `sources`' graph, which sizes there then read.
    pub fn new(
        graph: CodegenGraph<'g>,
        sources: Rc<TextureSources<'g>>,
        options: FrameCompilerOptions,
    ) -> Rc<Self> {
        let compiler = Rc::new(Self {
            graph,
            analysis: FrameValueAnalysis::new(graph, sources.clone()),
            sources: sources.clone(),
            uniform_scopes: options.uniform_scopes,
            texture_size: options.texture_size,
            state: RefCell::default(),
        });
        let sizing: Rc<dyn FrameSizing + 'g> = compiler.clone();
        sources.set_sizing(Rc::downgrade(&sizing));
        compiler
    }

    /// The values of the uniforms its program reads.
    pub fn uniforms(&self) -> Vec<(String, UniformValue)> {
        self.state.borrow().uniforms.clone()
    }

    /// The textures whose sizes the program reads: they must exist for it to be evaluated.
    pub fn sized_textures(&self) -> Vec<TextureSource> {
        self.state.borrow().sized.values().cloned().collect()
    }

    /// The program of everything compiled so far.
    pub fn program(&self) -> FrameProgram {
        FrameProgram {
            steps: self.state.borrow().steps.clone(),
        }
    }

    /// An output of a node of the document computed at `path`, if it's a frame
    /// value this can compile (not one reading a texture's size, when that's off).
    pub fn compile(&self, path: &[String], node: &str, handle: &str) -> Option<CompiledFrameValue> {
        let key = format!("{}|{node}|{handle}", path_key(path));
        if let Some(known) = self.state.borrow().memo.get(&key) {
            return known.clone();
        }
        let result = self
            .analysis
            .is_frame_value(path, node, handle)
            .then(|| self.compile_output(path, node, handle))
            .flatten();
        self.state.borrow_mut().memo.insert(key, result.clone());
        result
    }

    /// The program output giving a node's value, when it varies with what the
    /// runtime knows: one a shader can read from the frame block.
    pub fn hoist(&self, path: &[String], node: &str, handle: &str) -> Option<FrameRef> {
        let value = self.compile(path, node, handle)?;
        if !value.dynamic {
            return None;
        }
        Some(match value.input {
            FrameInput::Ref(r) => r,
            input => self.push(StepKind::Read(input), None, true, "value"),
        })
    }

    /// The same, as a number a plan can carry: a component of the value.
    #[cfg(test)]
    pub(crate) fn arg(
        &self,
        path: &[String],
        node: &str,
        handle: &str,
        component: usize,
    ) -> Option<FrameArg> {
        let value = self.compile(path, node, handle)?;
        Some(self.as_arg(&value, component))
    }

    /// What a node's input reads, as a number a plan can carry: the stored value when unlinked.
    pub fn input_arg(
        &self,
        path: &[String],
        node: &str,
        input: &str,
        component: usize,
    ) -> Option<FrameArg> {
        let value = self.input(path, node, input)?;
        Some(self.as_arg(&value, component))
    }

    fn as_arg(&self, value: &CompiledFrameValue, component: usize) -> FrameArg {
        match &value.input {
            FrameInput::Literal(v) => FrameArg::Literal(v.get(component).copied().unwrap_or(0.0)),
            input => {
                let reference = match input {
                    FrameInput::Ref(r) => r.clone(),
                    other => self.push(StepKind::Read(other.clone()), None, true, "value"),
                };
                FrameArg::Frame(FrameRef {
                    component: Some(component),
                    ..reference
                })
            }
        }
    }

    fn push(
        &self,
        kind: StepKind,
        loop_id: Option<String>,
        dynamic: bool,
        output: &str,
    ) -> FrameRef {
        let mut state = self.state.borrow_mut();
        state.steps.push(FrameStep { loop_id, kind });
        state.dynamics.push(dynamic);
        FrameRef {
            step: state.steps.len() - 1,
            output: output.to_owned(),
            component: None,
        }
    }

    fn is_dynamic(&self, input: &FrameInput) -> bool {
        match input {
            FrameInput::Literal(_) => false,
            FrameInput::Ref(r) => self
                .state
                .borrow()
                .dynamics
                .get(r.step)
                .copied()
                .unwrap_or(true),
            _ => true,
        }
    }

    fn reference(&self, reference: FrameRef) -> CompiledFrameValue {
        let dynamic = self
            .state
            .borrow()
            .dynamics
            .get(reference.step)
            .copied()
            .unwrap_or(true);
        CompiledFrameValue {
            input: FrameInput::Ref(reference),
            dynamic,
        }
    }

    /// The loop a node of the document at `path` runs in, if it's in a repeat zone.
    fn loop_of(&self, path: &[String], node: &str) -> Option<String> {
        let instance = self.sources.instance(path)?;
        let zone = instance.zones.node_zone.get(node)?.as_ref()?;
        (instance.zones.zones.get(zone)?.kind == ZoneKind::Repeat)
            .then(|| format!("{}|{zone}", path_key(path)))
    }

    fn compile_output(
        &self,
        path: &[String],
        node_id: &str,
        handle: &str,
    ) -> Option<CompiledFrameValue> {
        let instance = self.sources.instance(path)?;
        let node = instance.doc.node(node_id)?;
        match node.node_type.as_str() {
            "time" => Some(source(if handle == "frame" {
                FrameSource::Frame
            } else {
                FrameSource::Time
            })),
            "delta" => Some(source(FrameSource::Delta)),
            "resolution" => Some(source(FrameSource::PassSize)),
            TEXTURE_SIZE => {
                if !self.texture_size {
                    return None;
                }
                let texture = self.sources.input(path, node_id, "texture").source();
                if texture != TextureSource::Empty {
                    self.state
                        .borrow_mut()
                        .sized
                        .insert(texture.id(), texture.clone());
                }
                let size = FrameInput::Source(FrameSource::TextureSize(texture.id()));
                let level = self.input(path, node_id, TEXTURE_SIZE_LEVEL)?;
                let reference = self.push(
                    StepKind::Texel {
                        size,
                        level: Some(level.input),
                    },
                    self.loop_of(path, node_id),
                    true,
                    handle,
                );
                Some(self.reference(reference))
            }
            VIDEO => {
                if !is_video_metadata(handle) {
                    return None;
                }
                let asset = node
                    .data
                    .asset
                    .as_deref()
                    .and_then(|id| self.graph.project.assets.iter().find(|a| a.id() == id));
                if handle == VIDEO_INTERVAL {
                    return Some(literal(vec![frame_interval(asset)]));
                }
                debug_assert_eq!(handle, VIDEO_DURATION);
                let duration = asset.and_then(|a| a.video()).map_or(0.0, |v| v.duration);
                Some(literal(vec![f64::from(duration as f32)]))
            }
            GROUP_INPUT => {
                let (parent, outer) = path.split_last()?;
                self.input(outer, parent, handle)
            }
            NODE_GROUP => {
                let definition = self.graph.project.document(node.definition_ref()?)?;
                let out = definition.output_node_of(handle)?;
                let inner: Vec<String> = path.iter().cloned().chain([node_id.to_owned()]).collect();
                self.input(&inner, &out.id, handle)
            }
            REROUTE => self.input(path, node_id, "in"),
            ty => match zone_kind_of(ty) {
                Some(ZoneKind::Repeat) => self.compile_zone_value(path, node_id, handle),
                Some(ZoneKind::Simulation | ZoneKind::Passes) => None,
                None => self.compile_node(path, node_id, handle),
            },
        }
    }

    /// What a node's input reads: its link's value, or its stored one.
    fn input(&self, path: &[String], node_id: &str, input_id: &str) -> Option<CompiledFrameValue> {
        let instance = self.sources.instance(path)?;
        instance.doc.node(node_id)?;
        match instance
            .edges_into(node_id)
            .find(|e| e.target_handle == input_id)
        {
            Some(edge) => self.compile(path, &edge.source, &edge.source_handle),
            None => self.unlinked(path, node_id, input_id),
        }
    }

    /// An unlinked input's value: a uniform where inputs are, else a literal.
    fn unlinked(
        &self,
        path: &[String],
        node_id: &str,
        input_id: &str,
    ) -> Option<CompiledFrameValue> {
        let instance = self.sources.instance(path)?;
        let node = instance.doc.node(node_id)?;
        let descriptor = resolve_descriptor(node, self.graph.project);
        let types = self
            .graph
            .resolver
            .resolve(self.graph.project, node, &instance.bindings);
        let ty = types.input_type(input_id).filter(|t| !t.is_texture())?;
        let stored = node
            .data
            .values
            .get(input_id)
            .or_else(|| descriptor.default_value(input_id));
        let definition = if path.is_empty() {
            Some(ROOT_ID.to_owned())
        } else {
            self.definition_of(path)
        };
        if definition.is_some_and(|d| self.uniform_scopes.contains(&d)) {
            let def = descriptor.inputs.iter().find(|i| i.id == input_id);
            if def.is_none_or(|d| input_control(d, ty).is_uniform()) {
                let name = uniform_name(node_id, input_id);
                let value = uniform_value(ty, stored);
                let mut state = self.state.borrow_mut();
                match state.uniforms.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, existing)) => *existing = value,
                    None => state.uniforms.push((name.clone(), value)),
                }
                return Some(source(FrameSource::Uniform(name)));
            }
        }
        Some(literal(stored_value(stored, ty)))
    }

    fn definition_of(&self, path: &[String]) -> Option<String> {
        let (last, outer) = path.split_last()?;
        let parent = self.sources.instance(outer)?;
        parent.doc.node(last)?.definition_ref().map(str::to_owned)
    }

    fn compile_node(
        &self,
        path: &[String],
        node_id: &str,
        handle: &str,
    ) -> Option<CompiledFrameValue> {
        let instance = self.sources.instance(path)?;
        let node = instance.doc.node(node_id)?;
        let descriptor = resolve_descriptor(node, self.graph.project);
        let cpu = descriptor.cpu.clone()?;
        let types = self
            .graph
            .resolver
            .resolve(self.graph.project, node, &instance.bindings);
        let mut inputs = BTreeMap::new();
        let mut operands = BTreeMap::new();
        for id in self.analysis.inputs_read(path, node_id) {
            let variadic = types
                .resolved_inputs
                .iter()
                .find(|i| i.id == id)
                .is_some_and(|d| d.variadic);
            if variadic {
                let values: Vec<CompiledFrameValue> = instance
                    .edges_into(node_id)
                    .filter(|e| e.target_handle == id)
                    .map(|e| self.compile(path, &e.source, &e.source_handle))
                    .collect::<Option<_>>()?;
                operands.insert(id, values.into_iter().map(|v| v.input).collect());
            } else {
                inputs.insert(id.clone(), self.input(path, node_id, &id)?.input);
            }
        }
        let step = NodeStep {
            node_type: node.node_type.clone(),
            cpu: CpuHandle(cpu),
            data: node.data.clone(),
            input_types: types
                .input_types
                .iter()
                .filter(|(_, t)| !t.is_texture())
                .map(|(id, t)| (id.clone(), t.value_type()))
                .collect(),
            output_types: types
                .resolved_outputs
                .iter()
                .map(|o| (o.id.clone(), o.ty.value_type()))
                .collect(),
            output_type: types.output_type.value_type(),
            resolved_inputs: types.resolved_inputs.clone(),
            inputs,
            operands,
            present: instance
                .doc
                .edges_into(node_id)
                .map(|e| e.target_handle.clone())
                .collect(),
        };
        let loop_id = self.loop_of(path, node_id);
        let dynamic = step
            .inputs
            .values()
            .chain(step.operands.values().flatten())
            .any(|i| self.is_dynamic(i));
        if !dynamic && loop_id.is_none() {
            // Only literals: nothing the runtime knows is read, so it's known now.
            let folded = evaluate_frame(
                &FrameProgram {
                    steps: vec![FrameStep {
                        loop_id: None,
                        kind: StepKind::Node(Box::new(step)),
                    }],
                },
                &FrameEnv {
                    canvas: Size::new(1, 1),
                    pass_size: Size::new(1, 1),
                    ..FrameEnv::default()
                },
            )
            .outputs
            .into_iter()
            .next()
            .flatten()
            .unwrap_or_default();
            let mut state = self.state.borrow_mut();
            for (id, value) in &folded {
                state.memo.insert(
                    format!("{}|{node_id}|{id}", path_key(path)),
                    Some(literal(value.clone())),
                );
            }
            return Some(literal(
                folded.get(handle).cloned().unwrap_or_else(|| vec![0.0]),
            ));
        }
        let reference = self.push(StepKind::Node(Box::new(step)), loop_id, dynamic, handle);
        Some(self.reference(reference))
    }

    /// An output of a repeat zone's boundary: an item's current value, or its final one.
    fn compile_zone_value(
        &self,
        path: &[String],
        node_id: &str,
        handle: &str,
    ) -> Option<CompiledFrameValue> {
        let instance = self.sources.instance(path)?;
        let zone = instance
            .zones
            .zones
            .get(instance.zones.zone_of_boundary.get(node_id)?)?;
        let start = instance.doc.node(&zone.input_id)?;
        let loop_id = format!("{}|{}", path_key(path), zone.id);
        if instance.doc.node(node_id)?.is(REPEAT_INPUT) {
            return Some(CompiledFrameValue {
                input: FrameInput::Loop {
                    id: loop_id,
                    item: handle.to_owned(),
                },
                dynamic: true,
            });
        }
        let known = self.state.borrow().zones.get(&loop_id).copied();
        let index = match known {
            Some(index) => index,
            None => {
                let output = instance.doc.node(&zone.output_id);
                let outputs = output.map_or_else(Vec::new, |o| {
                    self.graph
                        .resolver
                        .resolve(self.graph.project, o, &instance.bindings)
                        .resolved_outputs
                        .clone()
                });
                let iterations = self.input(path, &zone.input_id, ITERATIONS_HANDLE)?;
                let mut items = Vec::new();
                for item in zone_items(output) {
                    let initial = self.input(path, &zone.input_id, &item.id)?;
                    let next = self.input(path, &zone.output_id, &item.id)?;
                    let ty = outputs.iter().find(|o| o.id == item.id)?.ty;
                    if ty.is_texture() {
                        return None;
                    }
                    items.push(RepeatItem {
                        id: item.id,
                        ty,
                        initial: initial.input,
                        next: next.input,
                    });
                }
                let parent = zone
                    .parent
                    .as_ref()
                    .map(|p| format!("{}|{p}", path_key(path)));
                let step = RepeatStep {
                    id: loop_id.clone(),
                    iterations: iterations.input,
                    max: zone_max_iterations(start),
                    items,
                };
                let index = self.push(StepKind::Repeat(step), parent, true, handle).step;
                self.state.borrow_mut().zones.insert(loop_id, index);
                index
            }
        };
        Some(self.reference(FrameRef {
            step: index,
            output: handle.to_owned(),
            component: None,
        }))
    }
}

impl FrameSizing for FrameCompiler<'_> {
    fn input_arg(
        &self,
        path: &[String],
        node: &str,
        input: &str,
        component: usize,
    ) -> Option<FrameArg> {
        FrameCompiler::input_arg(self, path, node, input, component)
    }
}

/// The WGSL identifier of a hoisted frame value's field.
pub fn frame_value_field(node_id: &str, handle: &str) -> String {
    format!("f_{}_{}", ident_suffix(node_id), ident_suffix(handle))
}
