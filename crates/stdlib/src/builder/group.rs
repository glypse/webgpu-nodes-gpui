use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use shader_graph::enums::{
    CASES_FAMILY, MATCH, MATCH_SELECTOR, WIDTH_SWITCH, WIDTH_SWITCH_VALUE, case_handle,
};
use shader_graph::graph::{
    Connection, FrameTint, GROUP_INPUT, GROUP_OUTPUT, GraphDocument, GraphEdge, GraphNode,
    GroupRef, LAYOUT_FRAME, NODE_GROUP, REROUTE, set_frame_note, set_frame_tint,
};
use shader_graph::layout::reroute_branches;
use shader_graph::nodes::{
    CUSTOM_WGSL, INDEX_SWITCH, INDEX_SWITCH_INDEX, build_index_switch_descriptor,
    build_match_descriptor, registered,
};
use shader_graph::types::{
    CustomSockets, GroupHandleDef, HandleDef, HandleType, InputValue, NodeData, NodeDescriptor,
    TypeVariable, WgslType, ZoneItem, input_family_key,
};
use shader_graph::zones::{
    ITERATION_HANDLE, ITERATIONS_HANDLE, REPEAT_INPUT, REPEAT_OUTPUT, is_zone_node_type,
};

use super::error::ErrorKind;
use super::ids::{handle_id, stable_id};
use super::interface::{EnumRef, Input, Output, TypeVar};
use super::operand::{Operand, Operands, Ref};

/// What the builder knows of a group it calls.
#[derive(Clone, Debug)]
pub struct ResolvedGroup {
    /// What group nodes reference: the id and version.
    pub reference: String,
    /// Input keys and the handle ids they have.
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

pub(crate) type Resolver<'r> = &'r dyn Fn(&str) -> Result<ResolvedGroup, ErrorKind>;

#[derive(Clone, Debug)]
struct Socket {
    /// What the description calls it: the handle, or a called group's key.
    name: String,
    handle: String,
}

impl Socket {
    fn plain(handle: &str) -> Self {
        Self {
            name: handle.to_owned(),
            handle: handle.to_owned(),
        }
    }
}

struct NodeRec {
    node: GraphNode,
    /// Takes any handle: the sockets of a group's boundary nodes are its interface.
    open: bool,
    inputs: Vec<Socket>,
    outputs: Vec<Socket>,
    variadic: Vec<String>,
}

impl NodeRec {
    fn input(&self, name: &str) -> Option<&Socket> {
        self.inputs
            .iter()
            .find(|s| s.name == name)
            .or_else(|| self.inputs.iter().find(|s| s.handle == name))
    }

    fn output(&self, name: &str) -> Option<&Socket> {
        self.outputs
            .iter()
            .find(|s| s.name == name)
            .or_else(|| self.outputs.iter().find(|s| s.handle == name))
    }
}

fn names(sockets: &[Socket]) -> String {
    let names: Vec<&str> = sockets.iter().map(|s| s.name.as_str()).collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn sockets_of(descriptor: &NodeDescriptor) -> (Vec<Socket>, Vec<Socket>, Vec<String>) {
    (
        descriptor
            .inputs
            .iter()
            .map(|i| Socket::plain(&i.id))
            .collect(),
        descriptor
            .outputs
            .iter()
            .map(|o| Socket::plain(&o.id))
            .collect(),
        descriptor
            .inputs
            .iter()
            .filter(|i| i.variadic)
            .map(|i| i.id.clone())
            .collect(),
    )
}

/// Node types with a method of their own, or none at all in a group.
fn own_method(node_type: &str) -> Option<&'static str> {
    Some(match node_type {
        GROUP_INPUT | GROUP_OUTPUT => "`input` and `output`",
        NODE_GROUP => "`call`",
        REROUTE => "`reroute` or `wireless`",
        LAYOUT_FRAME => "`frame`",
        MATCH => "`match_on`",
        INDEX_SWITCH => "`index_switch`",
        CUSTOM_WGSL => "`custom_wgsl`",
        "frag-output" => "nothing: a group has no fragment output",
        t if is_zone_node_type(t) => "`repeat`",
        _ => return None,
    })
}

#[derive(Default)]
struct State {
    nodes: Vec<NodeRec>,
    edges: Vec<GraphEdge>,
    /// Handle ids refs point at, by index.
    handles: Vec<String>,
    inputs: Vec<GroupHandleDef>,
    outputs: Vec<GroupHandleDef>,
    input_keys: Vec<(String, String)>,
    output_keys: Vec<(String, String)>,
    type_variables: Vec<TypeVariable>,
    defaults: BTreeMap<String, InputValue>,
    /// The frames being described, outermost first.
    frames: Vec<String>,
    /// Group Input and Group Output nodes, by side and the top-level frame they're in.
    boundaries: Vec<((bool, Option<String>), u32)>,
    dependencies: Vec<String>,
    errors: Vec<ErrorKind>,
}

impl State {
    fn intern(&mut self, handle: &str) -> u32 {
        let index = self
            .handles
            .iter()
            .position(|h| h == handle)
            .unwrap_or_else(|| {
                self.handles.push(handle.to_owned());
                self.handles.len() - 1
            });
        index as u32
    }

    fn handle(&self, r: Ref) -> Option<&str> {
        self.handles.get(r.handle as usize).map(String::as_str)
    }

    fn add_node(
        &mut self,
        group: &str,
        node_type: &str,
        data: NodeData,
        group_ref: Option<GroupRef>,
        frame: Option<String>,
        sockets: (Vec<Socket>, Vec<Socket>, Vec<String>),
    ) -> u32 {
        let id = stable_id(&format!("{group}/node/{}", self.nodes.len()));
        let mut node = GraphNode::new(node_type, shader_graph::graph::Point::default());
        node.id = id;
        node.data = data;
        node.group = group_ref;
        node.frame = frame;
        let (inputs, outputs, variadic) = sockets;
        self.nodes.push(NodeRec {
            node,
            open: false,
            inputs,
            outputs,
            variadic,
        });
        (self.nodes.len() - 1) as u32
    }

    fn add_registered(
        &mut self,
        group: &str,
        node_type: &str,
        data: NodeData,
    ) -> Result<u32, ErrorKind> {
        let descriptor = registered(node_type)
            .ok_or_else(|| ErrorKind::UnknownNodeType(node_type.to_owned()))?;
        let frame = self.frames.last().cloned();
        Ok(self.add_node(group, node_type, data, None, frame, sockets_of(&descriptor)))
    }

    /// The Group Input or Group Output node of the top-level frame being
    /// described (or of none), added the first time it's needed. Each shows
    /// only its linked sockets.
    fn boundary(&mut self, group: &str, input: bool) -> u32 {
        let frame = self.frames.first().cloned();
        let key = (input, frame.clone());
        if let Some((_, node)) = self.boundaries.iter().find(|(k, _)| *k == key) {
            return *node;
        }
        let data = NodeData {
            hide_unlinked: Some(true),
            ..Default::default()
        };
        let node_type = if input { GROUP_INPUT } else { GROUP_OUTPUT };
        let index = self.add_node(
            group,
            node_type,
            data,
            None,
            frame,
            (Vec::new(), Vec::new(), Vec::new()),
        );
        if let Some(rec) = self.nodes.get_mut(index as usize) {
            rec.open = true;
        }
        self.boundaries.push((key, index));
        index
    }

    fn link(&mut self, group: &str, source: Ref, target: u32, target_handle: &str) {
        if source == Ref::INVALID || target == Ref::INVALID.node {
            return;
        }
        let Some(handle) = self.handle(source).map(str::to_owned) else {
            return;
        };
        let Some(target_rec) = self.nodes.get(target as usize) else {
            return;
        };
        let target_id = target_rec.node.id.clone();
        let target_handle = if target_rec.open {
            target_handle.to_owned()
        } else if let Some(socket) = target_rec.input(target_handle) {
            socket.handle.clone()
        } else {
            let error = ErrorKind::NoSocket {
                node: target_rec.node.node_type.clone(),
                side: "input",
                name: target_handle.to_owned(),
                known: names(&target_rec.inputs),
            };
            self.errors.push(error);
            return;
        };
        let source_node = if source.node == Ref::GROUP_INPUT_NODE {
            let boundary = self.boundary(group, true);
            let known = self.inputs.iter().any(|h| h.id == handle);
            if !known {
                self.errors.push(ErrorKind::Invalid(format!(
                    "a link from \"{handle}\", which isn't an input of this group"
                )));
                return;
            }
            boundary
        } else {
            source.node
        };
        let Some(source_rec) = self.nodes.get(source_node as usize) else {
            self.errors.push(ErrorKind::Invalid(
                "a link from a socket of another group".to_owned(),
            ));
            return;
        };
        if source.node != Ref::GROUP_INPUT_NODE && source_rec.output(&handle).is_none() {
            self.errors.push(ErrorKind::NoSocket {
                node: source_rec.node.node_type.clone(),
                side: "output",
                name: handle,
                known: names(&source_rec.outputs),
            });
            return;
        }
        let connection =
            Connection::new(source_rec.node.id.clone(), handle, target_id, target_handle);
        let mut edge = connection.into_edge();
        edge.id = stable_id(&format!("{group}/edge/{}", self.edges.len()));
        self.edges.push(edge);
    }

    /// Link or store `operand` on the node's input `name`. A value for a
    /// variadic input becomes a Declare node (an f32), as links are all it collects.
    fn feed(&mut self, group: &str, node: u32, name: &str, operand: Operand) {
        match operand {
            Operand::Ref(source) => self.link(group, source, node, name),
            Operand::Value(value) => {
                let Some(rec) = self.nodes.get(node as usize) else {
                    return;
                };
                let handle = if rec.open {
                    name.to_owned()
                } else if let Some(socket) = rec.input(name) {
                    socket.handle.clone()
                } else {
                    let error = ErrorKind::NoSocket {
                        node: rec.node.node_type.clone(),
                        side: "input",
                        name: name.to_owned(),
                        known: names(&rec.inputs),
                    };
                    self.errors.push(error);
                    return;
                };
                if rec.variadic.contains(&handle) {
                    let constant = self.constant(group, WgslType::F32, value);
                    self.link(group, constant, node, &handle);
                } else if let Some(rec) = self.nodes.get_mut(node as usize) {
                    rec.node.data.values.insert(handle, value);
                }
            }
        }
    }

    fn constant(&mut self, group: &str, ty: WgslType, value: InputValue) -> Ref {
        let value = match value {
            InputValue::Scalar(s) => shader_graph::types::VectorValue {
                x: Some(s),
                y: Some(s),
                z: Some(s),
                w: Some(s),
            },
            InputValue::Vector(v) => v,
        };
        let mut data = NodeData::default();
        data.types.insert("declare".to_owned(), ty);
        data.values.extend(
            [
                ("x", value.x),
                ("y", value.y),
                ("z", value.z),
                ("w", value.w),
            ]
            .into_iter()
            .filter_map(|(k, v)| Some((k.to_owned(), InputValue::Scalar(v?)))),
        );
        match self.add_registered(group, "declare", data) {
            Ok(node) => self.primary(node),
            Err(e) => {
                self.errors.push(e);
                Ref::INVALID
            }
        }
    }

    /// The socket a node gives when used as a value: its first output.
    fn primary(&mut self, node: u32) -> Ref {
        let Some(rec) = self.nodes.get(node as usize) else {
            return Ref::INVALID;
        };
        let Some(output) = rec.outputs.first().map(|s| s.handle.clone()) else {
            let error = ErrorKind::NoOutput {
                node: rec.node.node_type.clone(),
            };
            self.errors.push(error);
            return Ref::INVALID;
        };
        Ref {
            node,
            handle: self.intern(&output),
        }
    }
}

/// A node of the group being described.
#[derive(Clone, Copy)]
pub struct Node<'g> {
    g: &'g GroupBuilder<'g>,
    index: u32,
}

impl<'g> Node<'g> {
    /// The socket `name` of the node: an output's id, or a called group's output key.
    pub fn out(&self, name: &str) -> Ref {
        if self.index == Ref::INVALID.node {
            return Ref::INVALID;
        }
        let mut state = self.g.state.borrow_mut();
        let Some(rec) = state.nodes.get(self.index as usize) else {
            return Ref::INVALID;
        };
        let Some(socket) = rec.output(name) else {
            let error = ErrorKind::NoSocket {
                node: rec.node.node_type.clone(),
                side: "output",
                name: name.to_owned(),
                known: names(&rec.outputs),
            };
            state.errors.push(error);
            return Ref::INVALID;
        };
        let handle = socket.handle.clone();
        Ref {
            node: self.index,
            handle: state.intern(&handle),
        }
    }

    /// The node's first output.
    pub fn first(&self) -> Ref {
        if self.index == Ref::INVALID.node {
            return Ref::INVALID;
        }
        self.g.state.borrow_mut().primary(self.index)
    }

    pub(super) fn operand(self) -> Operand {
        Operand::Ref(self.first())
    }
}

/// A frame around the nodes a description adds.
#[derive(Clone, Debug)]
pub struct Frame {
    label: String,
    note: Option<String>,
    tint: Option<FrameTint>,
}

impl Frame {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            note: None,
            tint: None,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn tint(mut self, tint: FrameTint) -> Self {
        self.tint = Some(tint);
        self
    }
}

impl From<&str> for Frame {
    fn from(label: &str) -> Self {
        Frame::new(label)
    }
}

/// A node being described: what feeds its inputs, and its settings.
pub struct NodeBuilder<'g> {
    g: &'g GroupBuilder<'g>,
    node_type: String,
    call: bool,
    allow_own: bool,
    positional: Vec<Operand>,
    named: Vec<(String, Vec<Operand>)>,
    settings: Vec<(String, String)>,
    types: Vec<(String, WgslType)>,
    label: Option<String>,
}

impl<'g> NodeBuilder<'g> {
    fn new(g: &'g GroupBuilder<'g>, node_type: &str) -> Self {
        Self {
            g,
            node_type: node_type.to_owned(),
            call: false,
            allow_own: false,
            positional: Vec::new(),
            named: Vec::new(),
            settings: Vec::new(),
            types: Vec::new(),
            label: None,
        }
    }

    /// Feed the inputs in the order the node declares them; a variadic last one takes the rest.
    pub fn args(mut self, operands: impl Operands) -> Self {
        self.positional.extend(operands.into_operands());
        self
    }

    /// Feed the input `name` (a node's input id; a called group's input key).
    /// A variadic input takes several operands.
    pub fn input(mut self, name: &str, operands: impl Operands) -> Self {
        self.named.push((name.to_owned(), operands.into_operands()));
        self
    }

    /// Pick `value` of the enum setting `key`.
    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.push((key.to_owned(), value.to_owned()));
        self
    }

    /// Force the type of the family `family` (an input id or family key).
    pub fn typed(mut self, family: &str, ty: WgslType) -> Self {
        self.types.push((family.to_owned(), ty));
        self
    }

    /// Name the node, shown instead of its type's.
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    pub fn add(self) -> Node<'g> {
        let g = self.g;
        match g.add_described(self) {
            Ok(index) => Node { g, index },
            Err(error) => {
                g.err(error);
                Node {
                    g,
                    index: Ref::INVALID.node,
                }
            }
        }
    }
}

/// A Match, Width Switch or Index Switch node's cases, added in the order given.
pub struct Switch<'g> {
    g: &'g GroupBuilder<'g>,
    kind: SwitchKind,
    cases: Vec<(i64, Operand)>,
    ty: Option<WgslType>,
    partial: bool,
}

enum SwitchKind {
    Match(EnumRef),
    Index(Operand, WgslType),
}

impl<'g> Switch<'g> {
    /// What the case of the enum's variant `value` is. On an Index Switch, `value` is ignored: cases are in order.
    pub fn case(mut self, value: i64, operand: impl Into<Operand>) -> Self {
        self.cases.push((value, operand.into()));
        self
    }

    /// A Match whose cases are only some of its enum's variants: the others
    /// are left unlinked, for variants the group's inputs rule out.
    pub fn partial(mut self) -> Self {
        self.partial = true;
        self
    }

    /// The type of the result when no case decides it.
    pub fn typed(mut self, ty: WgslType) -> Self {
        self.ty = Some(ty);
        self
    }

    pub fn add(self) -> Node<'g> {
        let g = self.g;
        match g.add_switch(self) {
            Ok(index) => Node { g, index },
            Err(error) => {
                g.err(error);
                Node {
                    g,
                    index: Ref::INVALID.node,
                }
            }
        }
    }
}

/// A Custom WGSL node: its code and sockets, named as the code uses them.
pub struct CustomWgsl {
    code: String,
    inputs: Vec<(String, WgslType)>,
    outputs: Vec<(String, WgslType)>,
}

impl CustomWgsl {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn input(mut self, name: &str, ty: WgslType) -> Self {
        self.inputs.push((name.to_owned(), ty));
        self
    }

    pub fn output(mut self, name: &str, ty: WgslType) -> Self {
        self.outputs.push((name.to_owned(), ty));
        self
    }
}

/// A state item of a repeat zone: its label, type and the value it starts as.
pub struct RepeatItem {
    label: String,
    ty: HandleType,
    initial: Operand,
}

impl RepeatItem {
    pub fn new(
        label: impl Into<String>,
        ty: impl Into<HandleType>,
        initial: impl Into<Operand>,
    ) -> Self {
        Self {
            label: label.into(),
            ty: ty.into(),
            initial: initial.into(),
        }
    }
}

/// The body of a repeat zone: the iteration and the items' values, and what
/// each item becomes next.
pub struct RepeatBody<'g> {
    g: &'g GroupBuilder<'g>,
    iteration: Ref,
    items: Vec<(String, Ref)>,
    next: RefCell<Vec<(String, Operand)>>,
}

impl RepeatBody<'_> {
    /// The index of the iteration being run.
    pub fn iteration(&self) -> Ref {
        self.iteration
    }

    /// The value of the item `label` at the start of the iteration.
    pub fn state(&self, label: &str) -> Ref {
        match self.items.iter().find(|(l, _)| l == label) {
            Some((_, r)) => *r,
            None => {
                self.g
                    .err(ErrorKind::Invalid(format!("no repeat item \"{label}\"")));
                Ref::INVALID
            }
        }
    }

    /// The value of the item `label` for the next iteration.
    pub fn set(&self, label: &str, operand: impl Into<Operand>) {
        self.next
            .borrow_mut()
            .push((label.to_owned(), operand.into()));
    }
}

/// The items of a repeat zone as it leaves it.
pub struct Repeated<'g> {
    g: &'g GroupBuilder<'g>,
    items: Vec<(String, Ref)>,
}

impl Repeated<'_> {
    /// The final value of the item `label`.
    pub fn get(&self, label: &str) -> Ref {
        match self.items.iter().find(|(l, _)| l == label) {
            Some((_, r)) => *r,
            None => {
                self.g
                    .err(ErrorKind::Invalid(format!("no repeat item \"{label}\"")));
                Ref::INVALID
            }
        }
    }
}

/// What a described group has become.
pub(crate) struct Finished {
    pub document: GraphDocument,
    pub dependencies: Vec<String>,
    pub input_keys: Vec<(String, String)>,
    pub output_keys: Vec<(String, String)>,
}

/// Describes one group: its interface and nodes. Mistakes are collected and
/// reported together when the group is built, never panicking.
pub struct GroupBuilder<'r> {
    id: String,
    resolver: Resolver<'r>,
    state: RefCell<State>,
}

impl<'r> GroupBuilder<'r> {
    pub(crate) fn new(id: &str, resolver: Resolver<'r>) -> Self {
        Self {
            id: id.to_owned(),
            resolver,
            state: RefCell::new(State::default()),
        }
    }

    fn err(&self, error: ErrorKind) {
        self.state.borrow_mut().errors.push(error);
    }

    // ── Interface ────────────────────────────────────────────────────────

    /// A type the sockets typed with it share, decided per group node among `types`.
    pub fn type_variable(&self, key: &str, name: &str, types: &[WgslType]) -> TypeVar {
        let id = stable_id(&format!("{}/type/{key}", self.id));
        let mut state = self.state.borrow_mut();
        if state.type_variables.iter().any(|v| v.id == id) {
            state.errors.push(ErrorKind::Duplicate {
                what: "type variable",
                key: key.to_owned(),
            });
        } else {
            state.type_variables.push(TypeVariable {
                id: id.clone(),
                name: name.to_owned(),
                types: types.to_vec(),
            });
        }
        TypeVar { id }
    }

    fn add_input(&self, key: &str, input: Input) -> (String, Ref) {
        let id = handle_id(&self.id, true, key);
        let mut state = self.state.borrow_mut();
        if state.input_keys.iter().any(|(k, _)| k == key) {
            state.errors.push(ErrorKind::Duplicate {
                what: "input",
                key: key.to_owned(),
            });
        }
        state.input_keys.push((key.to_owned(), id.clone()));
        let mut handle = GroupHandleDef::new(
            id.clone(),
            input.label.unwrap_or_else(|| key.to_owned()),
            input.ty,
        );
        handle.variants = input.variants;
        handle.compiled = input.compiled.then_some(true);
        handle.owner = input.owner;
        handle.type_owner = input.type_owner;
        handle.variants_by = input.variants_by;
        handle.min = input.min;
        handle.max = input.max;
        handle.display = input.display;
        state.inputs.push(handle);
        if let Some(default) = input.default {
            state.defaults.insert(id.clone(), default);
        }
        let socket = Ref {
            node: Ref::GROUP_INPUT_NODE,
            handle: state.intern(&id),
        };
        (id, socket)
    }

    /// An input of the group, linked from the Group Input node of wherever it's used.
    pub fn input(&self, key: &str, input: impl Into<Input>) -> Ref {
        self.add_input(key, input.into()).1
    }

    /// An enum input: made with `Input::enumeration` or `Input::toggle`.
    pub fn enum_input(&self, key: &str, input: Input) -> EnumRef {
        let variants = input.variants.clone();
        let (handle, socket) = self.add_input(key, input);
        let Some(variants) = variants else {
            self.err(ErrorKind::NotEnum(key.to_owned()));
            return EnumRef {
                socket,
                handle,
                variants: Vec::new(),
            };
        };
        EnumRef {
            socket,
            handle,
            variants,
        }
    }

    /// An output of the group fed by `source`.
    pub fn output(&self, key: &str, source: impl Into<Operand>, output: impl Into<Output>) {
        let (output, source): (Output, Operand) = (output.into(), source.into());
        let id = handle_id(&self.id, false, key);
        let mut state = self.state.borrow_mut();
        if state.output_keys.iter().any(|(k, _)| k == key) {
            state.errors.push(ErrorKind::Duplicate {
                what: "output",
                key: key.to_owned(),
            });
        }
        state.output_keys.push((key.to_owned(), id.clone()));
        state.outputs.push(GroupHandleDef::new(
            id.clone(),
            output.label.unwrap_or_else(|| key.to_owned()),
            output.ty,
        ));
        let boundary = state.boundary(&self.id, false);
        state.feed(&self.id, boundary, &id, source);
    }

    // ── Nodes ────────────────────────────────────────────────────────────

    /// A node of the registered type `node_type`, to feed and configure.
    pub fn node(&self, node_type: &str) -> NodeBuilder<'_> {
        NodeBuilder::new(self, node_type)
    }

    /// A node of `node_type` fed `operands` in the order its inputs are declared.
    pub fn op(&self, node_type: &str, operands: impl Operands) -> Node<'_> {
        self.node(node_type).args(operands).add()
    }

    /// A node of the group `target`, fed by input key; its outputs are by key too.
    pub fn call(&self, target: &str) -> NodeBuilder<'_> {
        let mut builder = NodeBuilder::new(self, target);
        builder.call = true;
        builder
    }

    /// A Declare node holding `value` as a `ty` (a scalar fills every component).
    pub fn constant(&self, ty: WgslType, value: impl Into<InputValue>) -> Node<'_> {
        let r = self.state.borrow_mut().constant(&self.id, ty, value.into());
        let index = r.node;
        Node { g: self, index }
    }

    /// A `ty` vector (or scalar) made of `components`.
    pub fn vec(&self, ty: WgslType, components: impl Operands) -> Node<'_> {
        self.node("declare")
            .typed("declare", ty)
            .args(components)
            .add()
    }

    /// Give a node a name, shown instead of its type's.
    pub fn label<'g>(&'g self, node: Node<'g>, label: &str) -> Node<'g> {
        if let Some(rec) = self.state.borrow_mut().nodes.get_mut(node.index as usize) {
            rec.node.data.label = Some(label.to_owned());
        }
        node
    }

    /// A reroute passing `source` on.
    pub fn reroute(&self, source: impl Into<Operand>) -> Node<'_> {
        self.raw_reroute(None, source.into())
    }

    /// A reroute named `label` passing `source` on, its links drawn as tags:
    /// for a value used far from where it's computed.
    pub fn wireless(&self, label: &str, source: impl Into<Operand>) -> Node<'_> {
        self.raw_reroute(Some(label), source.into())
    }

    fn raw_reroute(&self, label: Option<&str>, source: Operand) -> Node<'_> {
        let mut state = self.state.borrow_mut();
        let data = NodeData {
            label: label.map(str::to_owned),
            hide_links: label.map(|_| true),
            ..Default::default()
        };
        let frame = state.frames.last().cloned();
        let index = state.add_node(
            &self.id,
            REROUTE,
            data,
            None,
            frame,
            (
                vec![Socket::plain("in")],
                vec![Socket::plain("out")],
                Vec::new(),
            ),
        );
        state.feed(&self.id, index, "in", source);
        Node { g: self, index }
    }

    /// The case of `cases` (by component count, one to four) `value`'s type picks.
    pub fn width_switch(&self, value: impl Into<Operand>, cases: impl Operands) -> Node<'_> {
        self.width_switch_with(None, value.into(), cases.into_operands())
    }

    /// Like `width_switch`, converting every case to `ty`.
    pub fn width_switch_as(
        &self,
        ty: WgslType,
        value: impl Into<Operand>,
        cases: impl Operands,
    ) -> Node<'_> {
        self.width_switch_with(Some(ty), value.into(), cases.into_operands())
    }

    fn width_switch_with(
        &self,
        ty: Option<WgslType>,
        value: Operand,
        cases: Vec<Operand>,
    ) -> Node<'_> {
        let mut builder = NodeBuilder::new(self, WIDTH_SWITCH);
        builder.allow_own = true;
        builder = builder.input(WIDTH_SWITCH_VALUE, value);
        for (i, operand) in cases.into_iter().enumerate() {
            let handle = case_handle(i as i64 + 1);
            if let Some(ty) = ty {
                builder = builder.typed(&handle, ty);
            }
            builder = builder.input(&handle, operand);
        }
        builder.add()
    }

    /// A node picking by the enum input `selector`'s variant.
    pub fn match_on(&self, selector: &EnumRef) -> Switch<'_> {
        Switch {
            g: self,
            kind: SwitchKind::Match(selector.clone()),
            cases: Vec::new(),
            ty: None,
            partial: false,
        }
    }

    /// A node picking among cases (added with `case`, in order) by a runtime `index`, all of type `ty`.
    pub fn index_switch(&self, index: impl Into<Operand>, ty: WgslType) -> Switch<'_> {
        Switch {
            g: self,
            kind: SwitchKind::Index(index.into(), ty),
            cases: Vec::new(),
            ty: None,
            partial: false,
        }
    }

    /// A Custom WGSL node, its inputs fed by name.
    pub fn custom_wgsl(
        &self,
        custom: CustomWgsl,
        inputs: impl IntoIterator<Item = (&'static str, Operand)>,
    ) -> Node<'_> {
        let socket_id = |side: &str, name: &str| {
            stable_id(&format!(
                "{}/custom/{}/{side}/{name}",
                self.id,
                self.state.borrow().nodes.len()
            ))
        };
        let sockets = CustomSockets {
            inputs: custom
                .inputs
                .iter()
                .map(|(n, t)| HandleDef::new(socket_id("in", n), n.clone(), *t))
                .collect(),
            outputs: custom
                .outputs
                .iter()
                .map(|(n, t)| HandleDef::new(socket_id("out", n), n.clone(), *t))
                .collect(),
        };
        let data = NodeData {
            code: Some(custom.code),
            sockets: Some(sockets.clone()),
            ..Default::default()
        };
        let named = |defs: &[HandleDef]| -> Vec<Socket> {
            defs.iter()
                .map(|h| Socket {
                    name: h.label.clone(),
                    handle: h.id.clone(),
                })
                .collect()
        };
        let sockets = (named(&sockets.inputs), named(&sockets.outputs), Vec::new());
        let mut state = self.state.borrow_mut();
        let frame = state.frames.last().cloned();
        let index = state.add_node(&self.id, CUSTOM_WGSL, data, None, frame, sockets);
        for (name, operand) in inputs {
            state.feed(&self.id, index, name, operand);
        }
        Node { g: self, index }
    }

    // ── Organisation ─────────────────────────────────────────────────────

    /// A frame around the nodes `body` adds (nested frames included),
    /// returning what `body` does. Group inputs used and outputs given inside
    /// a top-level frame come from Group Input and Group Output nodes of its own.
    pub fn frame<T>(&self, frame: impl Into<Frame>, body: impl FnOnce() -> T) -> T {
        let frame: Frame = frame.into();
        {
            let mut state = self.state.borrow_mut();
            let mut data = NodeData {
                label: Some(frame.label),
                ..Default::default()
            };
            set_frame_note(&mut data, frame.note.as_deref());
            set_frame_tint(&mut data, frame.tint);
            let parent = state.frames.last().cloned();
            let index = state.add_node(
                &self.id,
                LAYOUT_FRAME,
                data,
                None,
                parent,
                (Vec::new(), Vec::new(), Vec::new()),
            );
            let id = state.nodes.get(index as usize).map(|r| r.node.id.clone());
            state.frames.extend(id);
        }
        let result = body();
        self.state.borrow_mut().frames.pop();
        result
    }

    // ── Zones ────────────────────────────────────────────────────────────

    /// A repeat zone running `iterations` times, carrying `items`: `body`
    /// gets the iteration and the items' current values and sets their next
    /// ones. Returns the items' final values.
    pub fn repeat(
        &self,
        iterations: impl Into<Operand>,
        items: Vec<RepeatItem>,
        body: impl FnOnce(&RepeatBody<'_>),
    ) -> Repeated<'_> {
        let iterations: Operand = iterations.into();
        let (input, output, zone_items, state_refs) = {
            let mut state = self.state.borrow_mut();
            let zone = state.nodes.len();
            let zone_items: Vec<ZoneItem> = items
                .iter()
                .map(|item| {
                    ZoneItem::new(
                        stable_id(&format!("{}/zone/{zone}/{}", self.id, item.label)),
                        item.label.clone(),
                        item.ty.clone(),
                    )
                })
                .collect();
            let item_sockets: Vec<Socket> =
                zone_items.iter().map(|i| Socket::plain(&i.id)).collect();
            let frame = state.frames.last().cloned();
            let output = state.add_node(
                &self.id,
                REPEAT_OUTPUT,
                NodeData::default(),
                None,
                frame.clone(),
                (item_sockets.clone(), item_sockets.clone(), Vec::new()),
            );
            let output_id = state.nodes.get(output as usize).map(|r| r.node.id.clone());
            let input_data = NodeData {
                paired_output: output_id,
                ..Default::default()
            };
            let mut input_sockets = vec![Socket::plain(ITERATIONS_HANDLE)];
            input_sockets.extend(item_sockets.clone());
            let mut state_outputs = vec![Socket::plain(ITERATION_HANDLE)];
            state_outputs.extend(item_sockets);
            let input = state.add_node(
                &self.id,
                REPEAT_INPUT,
                input_data,
                None,
                frame,
                (input_sockets, state_outputs, Vec::new()),
            );
            state.feed(&self.id, input, ITERATIONS_HANDLE, iterations);
            for (item, def) in items.iter().zip(&zone_items) {
                state.feed(&self.id, input, &def.id, item.initial);
            }
            if let Some(rec) = state.nodes.get_mut(output as usize) {
                rec.node.data.items = Some(zone_items.clone());
            }
            let refs: Vec<(String, Ref)> = zone_items
                .iter()
                .map(|def| {
                    let handle = state.intern(&def.id);
                    (
                        def.label.clone(),
                        Ref {
                            node: input,
                            handle,
                        },
                    )
                })
                .collect();
            (input, output, zone_items, refs)
        };
        let iteration = {
            let mut state = self.state.borrow_mut();
            Ref {
                node: input,
                handle: state.intern(ITERATION_HANDLE),
            }
        };
        let repeat_body = RepeatBody {
            g: self,
            iteration,
            items: state_refs,
            next: RefCell::new(Vec::new()),
        };
        body(&repeat_body);
        let next = repeat_body.next.into_inner();
        let mut state = self.state.borrow_mut();
        let mut results = Vec::new();
        for def in &zone_items {
            let operands: Vec<Operand> = next
                .iter()
                .filter(|(label, _)| *label == def.label)
                .map(|(_, o)| *o)
                .collect();
            match operands.as_slice() {
                [operand] => state.feed(&self.id, output, &def.id, *operand),
                other => state.errors.push(ErrorKind::Invalid(format!(
                    "repeat item \"{}\" is given {} next values, not one",
                    def.label,
                    other.len()
                ))),
            }
            let handle = state.intern(&def.id);
            results.push((
                def.label.clone(),
                Ref {
                    node: output,
                    handle,
                },
            ));
        }
        Repeated {
            g: self,
            items: results,
        }
    }

    // ── Adding nodes ─────────────────────────────────────────────────────

    fn add_described(&self, b: NodeBuilder<'_>) -> Result<u32, ErrorKind> {
        if b.call {
            return self.add_call(b);
        }
        let node_type = b.node_type.as_str();
        if !b.allow_own
            && let Some(instead) = own_method(node_type)
        {
            return Err(ErrorKind::OwnMethod {
                node: node_type.to_owned(),
                instead,
            });
        }
        let descriptor = registered(node_type)
            .ok_or_else(|| ErrorKind::UnknownNodeType(node_type.to_owned()))?;
        if descriptor.dev {
            return Err(ErrorKind::OwnMethod {
                node: node_type.to_owned(),
                instead: "nothing: it's for developing the editor",
            });
        }
        let mut data = NodeData {
            label: b.label.clone(),
            ..Default::default()
        };
        for (key, value) in &b.settings {
            let setting = descriptor
                .setting(key)
                .ok_or_else(|| ErrorKind::NoSetting {
                    node: node_type.to_owned(),
                    key: key.clone(),
                    known: descriptor
                        .settings
                        .iter()
                        .map(|s| s.key)
                        .collect::<Vec<_>>()
                        .join(", "),
                })?;
            if !setting.options.iter().any(|(v, _)| v == value) {
                return Err(ErrorKind::NoOption {
                    node: node_type.to_owned(),
                    key: key.clone(),
                    value: value.clone(),
                    options: setting
                        .options
                        .iter()
                        .map(|(v, _)| *v)
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
            data.settings.insert(key.clone(), value.clone());
        }
        let mut families: Vec<String> = descriptor
            .inputs
            .iter()
            .flat_map(|i| [i.id.clone(), input_family_key(i, &descriptor.inputs)])
            .collect();
        families.extend(descriptor.type_family.iter().map(|f| f.key.clone()));
        for (family, ty) in &b.types {
            if !families.contains(family) {
                let mut known: Vec<&str> = families.iter().map(String::as_str).collect();
                known.sort_unstable();
                known.dedup();
                return Err(ErrorKind::NoFamily {
                    node: node_type.to_owned(),
                    key: family.clone(),
                    known: known.join(", "),
                });
            }
            data.types.insert(family.clone(), *ty);
        }
        let feeds = Self::feeds(node_type, &descriptor, b.positional, b.named)?;
        let mut state = self.state.borrow_mut();
        let index = state.add_registered(&self.id, node_type, data)?;
        for (name, operand) in feeds {
            state.feed(&self.id, index, &name, operand);
        }
        Ok(index)
    }

    /// Operands by input: the positional ones in the descriptor's order, then the named.
    fn feeds(
        node_type: &str,
        descriptor: &NodeDescriptor,
        positional: Vec<Operand>,
        named: Vec<(String, Vec<Operand>)>,
    ) -> Result<Vec<(String, Operand)>, ErrorKind> {
        let mut feeds = Vec::new();
        let mut rest = positional.into_iter();
        for input in &descriptor.inputs {
            if input.variadic {
                feeds.extend(rest.by_ref().map(|o| (input.id.clone(), o)));
            } else if let Some(operand) = rest.next() {
                feeds.push((input.id.clone(), operand));
            }
        }
        let left = rest.count();
        if left > 0 {
            return Err(ErrorKind::TooManyOperands {
                node: node_type.to_owned(),
                max: descriptor.inputs.len(),
                given: descriptor.inputs.len() + left,
            });
        }
        for (name, operands) in named {
            let input = descriptor
                .inputs
                .iter()
                .find(|i| i.id == name)
                .ok_or_else(|| ErrorKind::NoSocket {
                    node: node_type.to_owned(),
                    side: "input",
                    name: name.clone(),
                    known: descriptor
                        .inputs
                        .iter()
                        .map(|i| i.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                })?;
            if !input.variadic && operands.len() != 1 {
                return Err(ErrorKind::NotVariadic {
                    node: node_type.to_owned(),
                    name,
                    given: operands.len(),
                });
            }
            feeds.extend(operands.into_iter().map(|o| (name.clone(), o)));
        }
        Ok(feeds)
    }

    fn add_call(&self, b: NodeBuilder<'_>) -> Result<u32, ErrorKind> {
        let resolved = (self.resolver)(&b.node_type)?;
        if !b.positional.is_empty() {
            return Err(ErrorKind::Invalid(format!(
                "\"{}\" takes its inputs by key (`input`), not in order",
                b.node_type
            )));
        }
        let socket = |(key, handle): &(String, String)| Socket {
            name: key.clone(),
            handle: handle.clone(),
        };
        let inputs: Vec<Socket> = resolved.inputs.iter().map(socket).collect();
        let outputs: Vec<Socket> = resolved.outputs.iter().map(socket).collect();
        let mut state = self.state.borrow_mut();
        if !state.dependencies.contains(&resolved.reference) {
            state.dependencies.push(resolved.reference.clone());
        }
        let frame = state.frames.last().cloned();
        let data = NodeData {
            label: b.label,
            ..Default::default()
        };
        let index = state.add_node(
            &self.id,
            NODE_GROUP,
            data,
            Some(GroupRef {
                definition_ref: resolved.reference,
                locked: true,
            }),
            frame,
            (inputs, outputs, Vec::new()),
        );
        for (name, operands) in b.named {
            if let [operand] = operands.as_slice() {
                state.feed(&self.id, index, &name, *operand);
            } else {
                state.errors.push(ErrorKind::NotVariadic {
                    node: b.node_type.clone(),
                    name,
                    given: operands.len(),
                });
            }
        }
        Ok(index)
    }

    fn add_switch(&self, switch: Switch<'_>) -> Result<u32, ErrorKind> {
        let (node_type, descriptor, mut data, selector, cases): (
            _,
            _,
            _,
            (String, Operand),
            Vec<(String, Operand)>,
        ) = match switch.kind {
            SwitchKind::Match(selector) => {
                let descriptor = build_match_descriptor(&selector.variants);
                let mut cases = Vec::new();
                for (value, operand) in &switch.cases {
                    if !selector.variants.iter().any(|v| v.value == *value) {
                        return Err(ErrorKind::Invalid(format!(
                            "Match has a case for {value}, which no variant of its enum has"
                        )));
                    }
                    cases.push((case_handle(*value), *operand));
                }
                let missing: Vec<String> = selector
                    .variants
                    .iter()
                    .filter(|v| !switch.cases.iter().any(|(value, _)| *value == v.value))
                    .map(|v| v.label.clone())
                    .collect();
                if !missing.is_empty() && !switch.partial {
                    return Err(ErrorKind::Invalid(format!(
                        "Match has no case for {}",
                        missing.join(", ")
                    )));
                }
                (
                    MATCH,
                    descriptor,
                    NodeData::default(),
                    (MATCH_SELECTOR.to_owned(), Operand::Ref(selector.socket)),
                    cases,
                )
            }
            SwitchKind::Index(index, ty) => {
                let count = switch.cases.len() as u32;
                let data = NodeData {
                    cases: Some(count),
                    types: BTreeMap::from([(CASES_FAMILY.to_owned(), ty)]),
                    ..Default::default()
                };
                let descriptor = build_index_switch_descriptor(&data);
                let cases = switch
                    .cases
                    .iter()
                    .enumerate()
                    .map(|(i, (_, o))| (case_handle(i as i64), *o))
                    .collect();
                (
                    INDEX_SWITCH,
                    descriptor,
                    data,
                    (INDEX_SWITCH_INDEX.to_owned(), index),
                    cases,
                )
            }
        };
        if let Some(ty) = switch.ty {
            data.types.insert(CASES_FAMILY.to_owned(), ty);
        }
        let mut state = self.state.borrow_mut();
        let frame = state.frames.last().cloned();
        let index = state.add_node(
            &self.id,
            node_type,
            data,
            None,
            frame,
            sockets_of(&descriptor),
        );
        state.feed(&self.id, index, &selector.0, selector.1);
        for (handle, operand) in cases {
            state.feed(&self.id, index, &handle, operand);
        }
        Ok(index)
    }

    // ── Result ───────────────────────────────────────────────────────────

    /// The group described, each output linked to several inputs split at a reroute.
    pub(crate) fn finish(
        self,
        declared_inputs: &[&str],
        declared_outputs: &[&str],
    ) -> Result<Finished, Vec<ErrorKind>> {
        let State {
            nodes,
            edges,
            inputs,
            outputs,
            input_keys,
            output_keys,
            type_variables,
            defaults,
            dependencies,
            mut errors,
            ..
        } = self.state.into_inner();
        for (side, declared, added) in [
            ("input", declared_inputs, &input_keys),
            ("output", declared_outputs, &output_keys),
        ] {
            for key in declared {
                if !added.iter().any(|(k, _)| k == key) {
                    errors.push(ErrorKind::Missing {
                        side,
                        key: (*key).to_owned(),
                    });
                }
            }
            for (key, _) in added {
                if !declared.contains(&key.as_str()) {
                    errors.push(ErrorKind::Undeclared {
                        side,
                        key: key.clone(),
                    });
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let nodes: Vec<GraphNode> = nodes.into_iter().map(|r| r.node).collect();
        let mut branch = 0;
        let id = self.id;
        let branches = reroute_branches(
            &nodes,
            &edges,
            |_| true,
            || {
                branch += 1;
                stable_id(&format!("{id}/branch/{branch}"))
            },
            &HashSet::new(),
        );
        let moved: std::collections::HashMap<&str, &GraphEdge> =
            branches.moved.iter().map(|e| (e.id.as_str(), e)).collect();
        let edges: Vec<GraphEdge> = edges
            .iter()
            .map(|e| {
                moved
                    .get(e.id.as_str())
                    .map_or_else(|| e.clone(), |m| (*m).clone())
            })
            .chain(branches.links.iter().cloned())
            .collect();
        let nodes: Vec<GraphNode> = nodes.into_iter().chain(branches.reroutes).collect();
        Ok(Finished {
            document: GraphDocument {
                nodes,
                edges,
                inputs,
                outputs,
                defaults,
                type_variables,
                ..Default::default()
            },
            dependencies,
            input_keys,
            output_keys,
        })
    }
}
