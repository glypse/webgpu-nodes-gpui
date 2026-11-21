use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use super::node_data::{EnumVariant, InputValue, NodeData};
use super::values::{ScalarKind, WgslType};
use crate::passes::TextureFormat;

/// What the add menu groups a node type under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeCategory {
    Input,
    Math,
    Vector,
    Logic,
    Integer,
    Converter,
    Zone,
    Color,
    Texture,
    Misc,
    Layout,
}

impl NodeCategory {
    pub const ALL: [NodeCategory; 11] = [
        NodeCategory::Input,
        NodeCategory::Math,
        NodeCategory::Vector,
        NodeCategory::Logic,
        NodeCategory::Integer,
        NodeCategory::Converter,
        NodeCategory::Zone,
        NodeCategory::Color,
        NodeCategory::Texture,
        NodeCategory::Misc,
        NodeCategory::Layout,
    ];

    pub fn id(self) -> &'static str {
        match self {
            NodeCategory::Input => "input",
            NodeCategory::Math => "math",
            NodeCategory::Vector => "vector",
            NodeCategory::Logic => "logic",
            NodeCategory::Integer => "integer",
            NodeCategory::Converter => "converter",
            NodeCategory::Zone => "zone",
            NodeCategory::Color => "color",
            NodeCategory::Texture => "texture",
            NodeCategory::Misc => "misc",
            NodeCategory::Layout => "layout",
        }
    }
}

/// How the value of an unlinked input is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum InputControl {
    /// A number per component, kept within `min` and `max` while edited.
    Number {
        min: Option<f64>,
        max: Option<f64>,
        soft_min: Option<f64>,
        soft_max: Option<f64>,
        compiled: bool,
    },
    /// An on/off switch per component.
    Toggle { compiled: bool },
    /// A colour swatch opening a picker; vec3f / vec4f only.
    Color,
    /// A colour wheel and value ramp drawn on the node; vec3f / vec4f only.
    Wheel,
    /// A choice among an enum's variants, stored as the variant's value.
    Enum { variants: Vec<EnumVariant> },
}

impl InputControl {
    pub fn number() -> Self {
        InputControl::Number { min: None, max: None, soft_min: None, soft_max: None, compiled: false }
    }

    /// Whether values edited by it are passed to the shader as uniforms, so
    /// editing them doesn't recompile it. An enum's value, or a `compiled`
    /// one, is compiled in instead.
    pub fn is_uniform(&self) -> bool {
        match self {
            InputControl::Number { compiled, .. } | InputControl::Toggle { compiled } => !compiled,
            InputControl::Color | InputControl::Wheel => true,
            InputControl::Enum { .. } => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeInputDef {
    /// Stable: what edges' target handles reference.
    pub id: String,
    pub label: String,
    /// The concrete types the input accepts; the first is the default type.
    /// Inputs sharing a family resolve to one type across the node.
    pub types: Vec<WgslType>,
    /// Puts the input in a family of its own choosing.
    pub family: Option<String>,
    /// Every link into it is collected, in link order.
    pub variadic: bool,
    pub control: Option<InputControl>,
    /// Needs a value that's the same for the whole frame.
    pub frame: bool,
    /// The node computes its own value when this socket is unlinked.
    pub fallback: bool,
}

impl NodeInputDef {
    pub fn new(id: impl Into<String>, label: impl Into<String>, types: &[WgslType]) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            types: types.to_vec(),
            family: None,
            variadic: false,
            control: None,
            frame: false,
            fallback: false,
        }
    }

    pub fn family(mut self, family: impl Into<String>) -> Self {
        self.family = Some(family.into());
        self
    }

    pub fn variadic(mut self) -> Self {
        self.variadic = true;
        self
    }

    pub fn control(mut self, control: InputControl) -> Self {
        self.control = Some(control);
        self
    }

    /// The type used when nothing decides: the first of `types`.
    pub fn default_type(&self) -> WgslType {
        self.types.first().copied().unwrap_or(WgslType::F32)
    }
}

/// The key of an input's family among a node's `inputs`, under which a type
/// forced on it is stored: its own `family`, or else the id of the first input
/// accepting the same types without one.
pub fn input_family_key(input: &NodeInputDef, inputs: &[NodeInputDef]) -> String {
    if let Some(family) = &input.family {
        return family.clone();
    }
    inputs
        .iter()
        .find(|i| i.family.is_none() && i.types == input.types)
        .map_or_else(|| input.id.clone(), |i| i.id.clone())
}

/// The control editing an input of resolved type `ty`.
pub fn input_control(input: &NodeInputDef, ty: WgslType) -> InputControl {
    let color = matches!(input.control, Some(InputControl::Color | InputControl::Wheel));
    if color && !matches!(ty, WgslType::Vec3f | WgslType::Vec4f) {
        return InputControl::number();
    }
    let boolean = ty.kind() == ScalarKind::Bool;
    match &input.control {
        Some(control) if !(boolean && matches!(control, InputControl::Number { .. })) => {
            control.clone()
        }
        _ if boolean => InputControl::Toggle { compiled: false },
        _ => InputControl::number(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeOutputDef {
    /// Stable: what edges' source handles reference.
    pub id: String,
    pub label: String,
    pub ty: WgslType,
}

impl NodeOutputDef {
    pub fn new(id: impl Into<String>, label: impl Into<String>, ty: WgslType) -> Self {
        Self { id: id.into(), label: label.into(), ty }
    }
}

/// A user-facing enum setting shown on the node (e.g. a comparison operator).
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSetting {
    /// Key of the selected value in the node's `settings`.
    pub key: &'static str,
    pub label: &'static str,
    pub options: Vec<(&'static str, &'static str)>,
}

impl NodeSetting {
    pub fn new(
        key: &'static str,
        label: &'static str,
        options: &[(&'static str, &'static str)],
    ) -> Self {
        Self { key, label, options: options.to_vec() }
    }

    /// The selected option's value: the stored one, or the first option.
    pub fn value<'a>(&'a self, data: &NodeData) -> &'a str {
        let stored = data.settings.get(self.key).map(String::as_str);
        self.options
            .iter()
            .find(|(v, _)| Some(*v) == stored)
            .or(self.options.first())
            .map_or("", |(v, _)| v)
    }
}

/// A larger editor a front end draws in a node's body.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeWidget {
    /// A code editor for the node's WGSL and its sockets.
    Code,
    /// How many cases the node has, added and removed at the end.
    Cases,
    /// A graph editing the node's curve.
    Curve,
    /// A gradient editing the node's colour ramp stops and their colours.
    Ramp,
    /// A list of the textures the node draws, edited by item.
    Items,
    /// A colour wheel and value ramp editing the values of a colour group's inputs.
    Color(crate::graph::GroupWidget),
    /// A choice of the project's assets of a kind, which the node reads.
    Asset(crate::assets::AssetKind),
}

/// Shader helper functions nodes may request through `WgslCtx::helper`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HelperName {
    SafeDiv,
    SafeMod,
    SafePow,
}

/// What a node's `wgsl` reads while generating code.
pub struct WgslCtx<'a> {
    /// The variable the statements define; single-output nodes' only output.
    pub var_name: String,
    /// The variable of each active output, by output id.
    pub output_vars: BTreeMap<String, String>,
    pub data: NodeData,
    /// The expressions of the upstream node(s) of each input: raw for
    /// non-variadic inputs, converted to the resolved type for variadic ones.
    pub inputs: BTreeMap<String, Vec<String>>,
    /// The node-wide resolved type of each input.
    pub input_types: BTreeMap<String, WgslType>,
    pub output_type: WgslType,
    pub resolved_outputs: Vec<NodeOutputDef>,
    pub resolved_inputs: Vec<NodeInputDef>,
    pub(crate) env: &'a mut dyn CtxEnv,
}

/// How a texture is filtered when sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SamplerFilter {
    Linear,
    Nearest,
}

impl SamplerFilter {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Nearest => "nearest",
        }
    }
}

/// What a sampler reads outside a texture's [0, 1] coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SamplerAddress {
    Clamp,
    Repeat,
    Mirror,
}

impl SamplerAddress {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Clamp => "clamp",
            Self::Repeat => "repeat",
            Self::Mirror => "mirror",
        }
    }
}

/// What a `WgslCtx` asks of the code generator.
pub(crate) trait CtxEnv {
    fn promote_input(&mut self, id: &str) -> String;
    fn is_present(&self, id: &str) -> bool;
    fn helper(&mut self, name: HelperName, ty: WgslType) -> String;
    fn constant(&self, id: &str) -> Option<i64>;
    fn param(&mut self, key: &str, value: &[f64]) -> String;
    fn define_fn(&mut self, name: &str, source: &str) -> String;
    fn texture(&mut self, id: &str) -> String;
    fn texture_format(&self, id: &str) -> TextureFormat;
    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String;
    fn asset(&self) -> Option<crate::assets::Asset>;
}

impl WgslCtx<'_> {
    /// An expression for the input, converted to its resolved type. For
    /// variadic inputs, the operands joined with ", ".
    pub fn promote_input(&mut self, id: &str) -> String {
        self.env.promote_input(id)
    }

    /// Whether an input carries a value, known while compiling: false when
    /// nothing feeds it, or when it comes from a group input whose socket the
    /// calling group node left unlinked.
    pub fn is_present(&self, id: &str) -> bool {
        self.env.is_present(id)
    }

    /// Request a shared helper function overload; returns its name.
    pub fn helper(&mut self, name: HelperName, ty: WgslType) -> String {
        self.env.helper(name, ty)
    }

    /// The value an input has while compiling, when known: it comes from an
    /// enum the calling group node set.
    pub fn constant(&self, id: &str) -> Option<i64> {
        self.env.constant(id)
    }

    /// An f32 / vecNf expression for a number of the node's own data: a
    /// uniform where input values are, a literal elsewhere.
    pub fn param(&mut self, key: &str, value: &[f64]) -> String {
        self.env.param(key, value)
    }

    /// Declare a function at the shader's top level, once however many nodes
    /// ask for it by `name`; returns `name`.
    pub fn define_fn(&mut self, name: &str, source: &str) -> String {
        self.env.define_fn(name, source)
    }

    /// The texture a Texture socket of the node reads (an input) or gives
    /// (an output), as a module-scope `texture_2d` of the shader.
    pub fn texture(&mut self, id: &str) -> String {
        self.env.texture(id)
    }

    /// The format of the texels of the texture `texture(id)` names.
    pub fn texture_format(&self, id: &str) -> TextureFormat {
        self.env.texture_format(id)
    }

    /// A sampler of the shader filtering and addressing as asked.
    pub fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> String {
        self.env.sampler(filter, address)
    }

    /// The project asset the node reads (`NodeData::asset`), if it exists.
    pub fn asset(&self) -> Option<crate::assets::Asset> {
        self.env.asset()
    }

    pub fn input_type(&self, id: &str) -> WgslType {
        self.input_types.get(id).copied().unwrap_or(WgslType::F32)
    }

    pub fn output_var(&self, id: &str) -> String {
        self.output_vars.get(id).cloned().unwrap_or_else(|| self.var_name.clone())
    }

    /// The expressions linked into an input, or none.
    pub fn operands(&self, id: &str) -> &[String] {
        self.inputs.get(id).map_or(&[], Vec::as_slice)
    }
}

/// A value as its components.
pub type Value = Vec<f64>;

/// Where a node's CPU implementation reads its inputs from. Inputs are
/// converted to their resolved types and read lazily: a Switch computes
/// only the case it picks.
pub trait CpuInputs {
    fn input(&self, id: &str) -> Value;
    /// The values of a variadic input's operands, each converted to its resolved type.
    fn operands(&self, id: &str) -> Vec<Value>;
    /// Whether something feeds the input (see `WgslCtx::is_present`).
    fn is_present(&self, id: &str) -> bool;
}

/// What a node's CPU implementation reads.
pub struct CpuCtx<'a> {
    pub inputs: &'a dyn CpuInputs,
    pub input_types: &'a BTreeMap<String, WgslType>,
    /// The resolved type of each active output.
    pub output_types: &'a BTreeMap<String, WgslType>,
    pub output_type: WgslType,
    pub resolved_inputs: &'a [NodeInputDef],
    pub data: &'a NodeData,
}

impl CpuCtx<'_> {
    pub fn input(&self, id: &str) -> Value {
        self.inputs.input(id)
    }

    pub fn operands(&self, id: &str) -> Vec<Value> {
        self.inputs.operands(id)
    }

    pub fn is_present(&self, id: &str) -> bool {
        self.inputs.is_present(id)
    }

    pub fn input_type(&self, id: &str) -> WgslType {
        self.input_types.get(id).copied().unwrap_or(WgslType::F32)
    }
}

pub type CpuFn = Arc<dyn Fn(&CpuCtx<'_>) -> BTreeMap<String, Value> + Send + Sync>;

pub type WgslFn = Arc<dyn Fn(&mut WgslCtx<'_>) -> Vec<String> + Send + Sync>;
pub type ResolveOutputType =
    Arc<dyn Fn(WgslType, &BTreeMap<String, WgslType>) -> WgslType + Send + Sync>;
pub type ResolveOutputs = Arc<dyn Fn(WgslType) -> Vec<NodeOutputDef> + Send + Sync>;
pub type ResolveInputs = Arc<dyn Fn(WgslType, &NodeData) -> Vec<NodeInputDef> + Send + Sync>;

/// The types a node's type picker offers, and the key its output type is stored under.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeFamily {
    pub key: String,
    pub types: Vec<WgslType>,
}

/// A node type's behaviour.
#[derive(Clone)]
pub struct NodeDescriptor {
    pub type_id: String,
    pub label: String,
    /// Other names the add menu's search finds it by.
    pub aliases: Vec<String>,
    pub category: NodeCategory,
    /// Left out of the add menu.
    pub hidden: bool,
    /// Listed in the add menu only while developer tools are on.
    pub dev: bool,
    pub inputs: Vec<NodeInputDef>,
    pub outputs: Vec<NodeOutputDef>,
    pub default_data: NodeData,
    /// Enum settings rendered on the node; their values live in node data.
    pub settings: Vec<NodeSetting>,
    /// The input whose family drives the node's representative type; the first by default.
    pub primary_input: Option<String>,
    /// Each output keeps the type declared in its `NodeOutputDef`; otherwise
    /// every output carries the node-wide resolved `outputType`.
    pub static_output_types: bool,
    pub resolve_output_type: Option<ResolveOutputType>,
    pub resolve_outputs: Option<ResolveOutputs>,
    pub resolve_inputs: Option<ResolveInputs>,
    pub type_family: Option<TypeFamily>,
    pub widget: Option<NodeWidget>,
    /// The node's outputs computed on the CPU, one value per output id.
    /// f32 results are rounded and integers wrap as in WGSL, so both agree.
    /// Nodes with no meaning on the CPU (they read a pixel, a texture or
    /// custom code) have none.
    pub cpu: Option<CpuFn>,
    /// Emits the statements defining the node's output variables.
    pub wgsl: WgslFn,
}

impl NodeDescriptor {
    pub fn new(type_id: &str, label: &str, category: NodeCategory) -> Self {
        Self {
            type_id: type_id.to_owned(),
            label: label.to_owned(),
            aliases: Vec::new(),
            category,
            hidden: false,
            dev: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            default_data: NodeData::default(),
            settings: Vec::new(),
            primary_input: None,
            static_output_types: false,
            resolve_output_type: None,
            resolve_outputs: None,
            resolve_inputs: None,
            type_family: None,
            widget: None,
            cpu: None,
            wgsl: Arc::new(|_| Vec::new()),
        }
    }

    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|a| (*a).to_owned()).collect();
        self
    }

    pub fn inputs(mut self, inputs: Vec<NodeInputDef>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn outputs(mut self, outputs: Vec<NodeOutputDef>) -> Self {
        self.outputs = outputs;
        self
    }

    pub fn defaults(mut self, data: NodeData) -> Self {
        self.default_data = data;
        self
    }

    pub fn settings(mut self, settings: Vec<NodeSetting>) -> Self {
        self.settings = settings;
        self
    }

    pub fn output_type(
        mut self,
        f: impl Fn(WgslType, &BTreeMap<String, WgslType>) -> WgslType + Send + Sync + 'static,
    ) -> Self {
        self.resolve_output_type = Some(Arc::new(f));
        self
    }

    pub fn wgsl(
        mut self,
        f: impl Fn(&mut WgslCtx<'_>) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        self.wgsl = Arc::new(f);
        self
    }

    pub fn cpu(
        mut self,
        f: impl Fn(&CpuCtx<'_>) -> BTreeMap<String, Value> + Send + Sync + 'static,
    ) -> Self {
        self.cpu = Some(Arc::new(f));
        self
    }

    pub fn cpu_fn(mut self, f: CpuFn) -> Self {
        self.cpu = Some(f);
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub fn dev(mut self) -> Self {
        self.dev = true;
        self
    }

    pub fn primary(mut self, id: &str) -> Self {
        self.primary_input = Some(id.to_owned());
        self
    }

    pub fn static_outputs(mut self) -> Self {
        self.static_output_types = true;
        self
    }

    pub fn input(&self, id: &str) -> Option<&NodeInputDef> {
        self.inputs.iter().find(|i| i.id == id)
    }

    pub fn output(&self, id: &str) -> Option<&NodeOutputDef> {
        self.outputs.iter().find(|o| o.id == id)
    }

    pub fn setting(&self, key: &str) -> Option<&NodeSetting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// The default value of an input, if the descriptor gives one.
    pub fn default_value(&self, input: &str) -> Option<&InputValue> {
        self.default_data.values.get(input)
    }

    /// The types the node's type picker offers and the key its pick is stored
    /// under: its `type_family`, or else its primary input's types, inferred
    /// from links until one is picked. `None` without a choice.
    pub fn pickable_types(&self) -> Option<PickableTypes> {
        if let Some(family) = &self.type_family {
            return (family.types.len() >= 2).then(|| PickableTypes {
                key: family.key.clone(),
                types: family.types.clone(),
                inferable: false,
            });
        }
        let primary = self
            .primary_input
            .as_deref()
            .and_then(|id| self.input(id))
            .or(self.inputs.first())?;
        (primary.types.len() >= 2).then(|| PickableTypes {
            key: input_family_key(primary, &self.inputs),
            types: primary.types.clone(),
            inferable: true,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickableTypes {
    pub key: String,
    pub types: Vec<WgslType>,
    pub inferable: bool,
}

impl fmt::Debug for NodeDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeDescriptor")
            .field("type_id", &self.type_id)
            .field("label", &self.label)
            .field("category", &self.category)
            .field("inputs", &self.inputs)
            .field("outputs", &self.outputs)
            .finish_non_exhaustive()
    }
}
