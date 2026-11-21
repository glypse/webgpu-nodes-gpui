//! Resolves Texture sockets to the images they read.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use super::item_size::{
    ItemSizeMode, SizeHandles, item_size_handles, node_size_handles, stored_size,
};
use super::simulation::{
    Attachment, SimulationRef, SimulationState, StateItem, StateRead, attachment_groups, state_format,
    state_read_id, state_texture_id,
};
use super::textures::{CANVAS_SIZE, FrameArg, SizeBase, TextureFormat, TextureSize};
use crate::assets::{DEFAULT_PLAYBACK, VideoEnd, VideoPlayback};
use crate::codegen::CodegenGraph;
use crate::enums::{EnumDocument, MATCH, case_handle, group_constants};
use crate::graph::{GraphDocument, GraphEdge, GraphNode, NODE_GROUP};
use crate::nodes::{
    INDEX_SWITCH, INDEX_SWITCH_INDEX, RENDER_LIKE, RENDER_TEXTURE, index_switch_cases,
    mips_problem, render_item_format, render_item_type, renders_in_pixels, renders_mips,
    render_texture_items, resolve_descriptor,
};
use crate::types::{TypeBindings, WgslType, ZoneItem, unavailable_edges};
use crate::zones::{
    SIMULATION_INPUT, SIMULATION_OUTPUT, ZoneAnalysis, ZoneKind, analyze_zones, zone_items,
    zone_options,
};

/// What a value linked to a Texture input is rendered into: the canvas' size, in half float.
pub const IMPLICIT_RENDER_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// The id of the texture an unlinked Texture input reads: transparent black.
pub const EMPTY_TEXTURE: &str = "empty";


/// An output of a node of the document computed at `path` (see `TextureSources::instance`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SocketRef {
    pub path: Vec<String>,
    pub node: String,
    pub handle: String,
}

/// The id of the texture a value is rendered into: one per output and group instance.
pub fn render_texture_id(target: &SocketRef) -> String {
    format!("render:{}:{}", path_key(target.path.iter().chain([&target.node])), target.handle)
}

fn path_key<'a>(parts: impl IntoIterator<Item = &'a String>) -> String {
    parts.into_iter().cloned().collect::<Vec<_>>().join("/")
}

/// The id of the texture an asset gives, played as `video` says when it's a
/// video: one per Video node (`owner`), as each shows a time of its own.
pub fn media_texture_id(asset: &str, video: Option<&VideoPlayback>, owner: &str) -> String {
    match video {
        Some(v) => format!("media:{asset}:{owner}:{}:{}:{}", v.start, v.speed, end_id(v.end)),
        None => format!("media:{asset}"),
    }
}

fn end_id(end: VideoEnd) -> &'static str {
    match end {
        VideoEnd::Loop => "loop",
        VideoEnd::Hold => "hold",
        VideoEnd::Transparent => "transparent",
    }
}

/// Compiles the frame values sizes are made of. Sources have none until a
/// plan gives them one: sizes then are the node's stored numbers.
pub trait FrameSizing {
    fn input_arg(&self, path: &[String], node: &str, input: &str, component: usize) -> Option<FrameArg>;
}

/// An item of a Render Texture node, as it's typed and kept.
#[derive(Clone)]
struct RenderItem {
    item: ZoneItem,
    ty: WgslType,
    format: TextureFormat,
}

impl Attachment for RenderItem {
    fn format(&self) -> TextureFormat {
        self.format
    }

    fn size(&self) -> Option<&TextureSize> {
        None
    }
}

/// An item of a Render Texture node as a pass draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderItemDraw {
    /// The id of the texture it's drawn into.
    pub id: String,
    /// The id of the item on the node.
    pub item: String,
    pub ty: WgslType,
    pub format: TextureFormat,
}

/// How an item of a Render Texture node is drawn: one pass draws a run of its
/// items, as colour attachments, at the node's size.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderDraw {
    pub path: Vec<String>,
    pub node: String,
    pub run: Vec<RenderItemDraw>,
    /// The texture its size is relative to, if it has one: it exists before the pass.
    pub like: Option<Box<TextureSource>>,
}

/// An image a Texture socket reads, known while compiling. Its `id` names
/// it: sockets reading the same image have the same id.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSource {
    /// An asset's file, which the runtime is given; a video's frame at the
    /// project's time or, with `time`, at that project time instead.
    Media { id: String, asset: String, video: Option<VideoPlayback>, time: Option<FrameArg> },
    /// A value rendered over a texture of `size` and `format`, in a pass of its own.
    Render {
        id: String,
        target: SocketRef,
        size: TextureSize,
        format: TextureFormat,
        /// Whether it has a full chain of mip levels.
        mips: bool,
        /// Set for an item of a Render Texture node, drawn with the others of its run.
        draw: Option<RenderDraw>,
    },
    /// A simulation's state item, kept in a texture between frames.
    State {
        id: String,
        /// The texture keeping it (see `state_texture_id`), whichever frame is read.
        texture: String,
        read: StateRead,
        format: TextureFormat,
        size: TextureSize,
        simulation: SimulationRef,
    },
    Empty,
}

impl TextureSource {
    pub fn id(&self) -> String {
        match self {
            Self::Media { id, .. } | Self::Render { id, .. } | Self::State { id, .. } => id.clone(),
            Self::Empty => EMPTY_TEXTURE.to_owned(),
        }
    }

    /// The format a source's texels are in; media is half float.
    pub fn format(&self) -> TextureFormat {
        match self {
            Self::Render { format, .. } | Self::State { format, .. } => *format,
            _ => TextureFormat::Rgba16Float,
        }
    }

    /// The size of a source made by a pass, if it's known while compiling: media has its own.
    pub fn size(&self) -> Option<&TextureSize> {
        match self {
            Self::Render { size, .. } | Self::State { size, .. } => Some(size),
            _ => None,
        }
    }
}

/// What a Texture socket reads: an image, or nothing it can read, and why.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum TextureResolution {
    Source(TextureSource),
    Invalid {
        reason: String,
        /// The link where it can't be read; empty until it's found right past one.
        edge: String,
    },
}

impl TextureResolution {
    /// The image it reads: nothing for an invalid one.
    pub fn source(&self) -> TextureSource {
        match self {
            Self::Source(s) => s.clone(),
            Self::Invalid { .. } => TextureSource::Empty,
        }
    }
}

const EMPTY: TextureResolution = TextureResolution::Source(TextureSource::Empty);

fn invalid(reason: impl Into<String>, edge: impl Into<String>) -> TextureResolution {
    TextureResolution::Invalid { reason: reason.into(), edge: edge.into() }
}

const ZONE_STATE_REASON: &str =
    "A value computed in a repeat zone can't be rendered to a texture: it changes with every iteration.";

/// A document as computed for one group node (the root for the empty path).
pub struct Instance<'g> {
    pub path: Vec<String>,
    pub doc: &'g GraphDocument,
    pub bindings: TypeBindings,
    pub zones: ZoneAnalysis,
    /// Enum values known while compiling, by group-input handle.
    pub constants: BTreeMap<String, i64>,
    /// The links codegen follows.
    pub edges: Vec<GraphEdge>,
}

impl Instance<'_> {
    /// The links into a node, in document order.
    pub fn edges_into<'a>(&'a self, node: &'a str) -> impl Iterator<Item = &'a GraphEdge> {
        self.edges.iter().filter(move |e| e.target == node)
    }
}

/// Resolves Texture sockets to the images they read, per group instance:
/// a group's Texture input reads what the group node it's computed for is
/// given, so a group used twice may read two images. Memoised: build one
/// per graph state.
pub struct TextureSources<'g> {
    graph: CodegenGraph<'g>,
    instances: RefCell<HashMap<String, Option<Rc<Instance<'g>>>>>,
    resolved: RefCell<HashMap<String, TextureResolution>>,
    visiting: RefCell<HashSet<String>>,
    simulations: RefCell<HashMap<String, Option<Rc<SimulationState>>>>,
    building: RefCell<HashSet<String>>,
    zone_state: RefCell<HashMap<String, bool>>,
    /// Where the sizes of Render Textures' textures read their frame values.
    sizing: RefCell<Option<std::rc::Weak<dyn FrameSizing + 'g>>>,
}

impl<'g> TextureSources<'g> {
    pub fn new(graph: CodegenGraph<'g>) -> Self {
        Self {
            graph,
            instances: RefCell::default(),
            resolved: RefCell::default(),
            visiting: RefCell::default(),
            simulations: RefCell::default(),
            building: RefCell::default(),
            zone_state: RefCell::default(),
            sizing: RefCell::default(),
        }
    }

    pub fn set_sizing(&self, sizing: std::rc::Weak<dyn FrameSizing + 'g>) {
        *self.sizing.borrow_mut() = Some(sizing);
    }

    fn sizing(&self) -> Option<Rc<dyn FrameSizing + 'g>> {
        self.sizing.borrow().as_ref().and_then(std::rc::Weak::upgrade)
    }

    /// The size the size inputs `handles` of a node give, in `mode`: the
    /// stored numbers, or the frame values linked to them once a frame
    /// compiler is sizing.
    fn size_of(
        &self,
        path: &[String],
        node: &GraphNode,
        handles: &SizeHandles,
        mode: ItemSizeMode,
        like: Option<&TextureSource>,
    ) -> TextureSize {
        let base = like.map_or(SizeBase::Canvas, |l| SizeBase::Resource(l.id()));
        let stored = stored_size(&node.data, handles, mode, base);
        let Some(sizing) = self.sizing() else { return stored };
        let arg = |input: &str, component: usize, fallback: FrameArg| {
            sizing.input_arg(path, &node.id, input, component).unwrap_or(fallback)
        };
        match stored {
            TextureSize::Pixels { width, height } => TextureSize::Pixels {
                width: arg(&handles.pixels, 0, width),
                height: arg(&handles.pixels, 1, height),
            },
            TextureSize::Scale { scale, of } => {
                TextureSize::Scale { scale: arg(&handles.scale, 0, scale), of }
            }
        }
    }

    pub fn graph(&self) -> CodegenGraph<'g> {
        self.graph
    }

    /// The document computed at `path`, as group nodes there bind and set it.
    pub fn instance(&self, path: &[String]) -> Option<Rc<Instance<'g>>> {
        let key = path.join("/");
        if let Some(found) = self.instances.borrow().get(&key) {
            return found.clone();
        }
        let instance = self.create_instance(path).map(Rc::new);
        self.instances.borrow_mut().insert(key, instance.clone());
        instance
    }

    fn create_instance(&self, path: &[String]) -> Option<Instance<'g>> {
        let project = self.graph.project;
        let Some((last, parent_path)) = path.split_last() else {
            return Some(self.document_instance(path, project.root(), TypeBindings::new(), BTreeMap::new()));
        };
        let parent = self.instance(parent_path)?;
        let node = parent.doc.node(last)?;
        let doc = project.document(node.definition_ref()?)?;
        let bindings = self.graph.resolver.resolve(project, node, &parent.bindings).bindings.clone().unwrap_or_default();
        let constants = self.with_enums(&parent, |enums| {
            group_constants(enums, node, project, &bindings, &parent.constants)
        });
        Some(self.document_instance(path, doc, bindings, constants))
    }

    fn document_instance(
        &self,
        path: &[String],
        doc: &'g GraphDocument,
        bindings: TypeBindings,
        constants: BTreeMap<String, i64>,
    ) -> Instance<'g> {
        let project = self.graph.project;
        let zones = analyze_zones(&doc.nodes, &doc.edges, &zone_options(project));
        let unavailable = unavailable_edges(&doc.edges, |id| {
            doc.node(id).map(|n| self.graph.resolver.resolve(project, n, &bindings))
        });
        let edges = doc
            .edges
            .iter()
            .filter(|e| !zones.invalid_edges.contains(&e.id) && !unavailable.contains(&e.id))
            .cloned()
            .collect();
        Instance { path: path.to_vec(), doc, bindings, zones, constants, edges }
    }

    /// Runs `f` with the enum tracing of an instance's document.
    pub fn with_enums<R>(&self, instance: &Instance<'g>, f: impl FnOnce(&EnumDocument<'_>) -> R) -> R {
        let project = self.graph.project;
        let input_type = |node: &str, input: &str| {
            let node = instance.doc.node(node)?;
            let ty = self.graph.resolver.resolve(project, node, &instance.bindings).input_type(input)?;
            (!ty.is_texture()).then_some(ty)
        };
        f(&EnumDocument {
            nodes: &instance.doc.nodes,
            edges: &instance.edges,
            inputs: &instance.doc.inputs,
            input_type: Some(&input_type),
        })
    }

    /// The link codegen follows into a node's input at `path`, if any.
    pub fn feed(&self, path: &[String], node: &str, input: &str) -> Option<GraphEdge> {
        self.instance(path)?.edges_into(node).find(|e| e.target_handle == input).cloned()
    }

    /// What a node's Texture input reads, at `path`.
    pub fn input(&self, path: &[String], node: &str, input: &str) -> TextureResolution {
        self.memo(format!("in|{}|{node}|{input}", path.join("/")), || {
            let Some(instance) = self.instance(path) else { return EMPTY };
            let Some(edge) = self.feed(path, node, input) else { return EMPTY };
            let source_type = self.graph.resolver.infer_handle_type(
                self.graph.project,
                &edge.source,
                &edge.source_handle,
                &instance.bindings,
            );
            if source_type.is_texture() {
                return through(&edge, self.output(path, &edge.source, &edge.source_handle));
            }
            // A value is rendered once per frame: it can't change per iteration of a loop.
            if self.reads_zone_state(path, &edge.source, &edge.source_handle) {
                return invalid(ZONE_STATE_REASON, edge.id);
            }
            let target = SocketRef { path: path.to_vec(), node: edge.source, handle: edge.source_handle };
            TextureResolution::Source(TextureSource::Render {
                id: render_texture_id(&target),
                target,
                size: CANVAS_SIZE,
                format: IMPLICIT_RENDER_FORMAT,
                mips: false,
                draw: None,
            })
        })
    }

    /// What a node's Texture output gives, at `path`.
    pub fn output(&self, path: &[String], node: &str, handle: &str) -> TextureResolution {
        self.memo(format!("out|{}|{node}|{handle}", path.join("/")), || {
            self.compute_output(path, node, handle)
        })
    }

    fn compute_output(&self, path: &[String], node_id: &str, handle: &str) -> TextureResolution {
        let Some(instance) = self.instance(path) else { return EMPTY };
        let Some(node) = instance.doc.node(node_id) else { return EMPTY };
        let project = self.graph.project;
        match node.node_type.as_str() {
            crate::nodes::IMAGE | crate::nodes::VIDEO => self.media(path, node),
            SIMULATION_INPUT => self.state(path, &instance, node_id, handle, StateRead::Previous),
            SIMULATION_OUTPUT => self.state(path, &instance, node_id, handle, StateRead::Current),
            "reroute" => self.input(path, node_id, "in"),
            "group-input" => match path.split_last() {
                None => EMPTY,
                Some((caller, parent)) => self.input(parent, caller, handle),
            },
            NODE_GROUP => {
                let Some(def) = node.definition_ref().and_then(|r| project.document(r)) else {
                    return EMPTY;
                };
                let outputs: Vec<&GraphNode> =
                    def.nodes.iter().filter(|n| n.is("group-output")).collect();
                let output = outputs
                    .iter()
                    .find(|n| def.edges_into(&n.id).any(|e| e.target_handle == handle))
                    .or(outputs.first());
                let inner: Vec<String> = path.iter().cloned().chain([node_id.to_owned()]).collect();
                output.map_or(EMPTY, |o| self.input(&inner, &o.id, handle))
            }
            MATCH => {
                match self.with_enums(&instance, |e| e.matched_case(node_id, &instance.constants)) {
                    None => self.unknown_case(node, "an enum known while compiling"),
                    Some(picked) => self.input(path, node_id, &picked),
                }
            }
            RENDER_TEXTURE => self.render_texture(path, &instance, node, handle),
            INDEX_SWITCH => {
                let Some(index) = self.known_index(&instance, node) else {
                    return self.unknown_case(node, "an index known while compiling");
                };
                let last = i64::from(index_switch_cases(&node.data)) - 1;
                self.input(path, node_id, &case_handle(index.clamp(0, last)))
            }
            _ => EMPTY,
        }
    }

    /// An item of the Render Texture node `node`, drawn with the items of its run.
    fn render_texture(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        node: &GraphNode,
        handle: &str,
    ) -> TextureResolution {
        let items: Vec<RenderItem> = render_texture_items(&node.data)
            .into_iter()
            .map(|item| RenderItem {
                ty: render_item_type(&item),
                format: render_item_format(&item),
                item,
            })
            .collect();
        let Some(own) = items.iter().find(|i| i.item.id == handle) else { return EMPTY };
        if let Some(problem) = mips_problem(&node.data) {
            return invalid(problem, "");
        }
        if let Some(feed) = self.feed(path, &node.id, handle)
            && self.reads_zone_state(path, &feed.source, &feed.source_handle)
        {
            return invalid(ZONE_STATE_REASON, feed.id);
        }
        let run = attachment_groups(&items)
            .into_iter()
            .find(|r| r.iter().any(|i| i.item.id == handle))
            .unwrap_or_default();
        let first = items.first().map(|i| i.item.id.as_str());
        let like = self.like_of(path, instance, node, first);
        let mode =
            if renders_in_pixels(&node.data) { ItemSizeMode::Pixels } else { ItemSizeMode::Scale };
        let size = self.size_of(path, node, &node_size_handles(), mode, like.as_ref());
        let target = SocketRef { path: path.to_vec(), node: node.id.clone(), handle: handle.to_owned() };
        TextureResolution::Source(TextureSource::Render {
            id: render_texture_id(&target),
            target,
            size,
            format: own.format,
            mips: renders_mips(&node.data),
            draw: Some(RenderDraw {
                path: path.to_vec(),
                node: node.id.clone(),
                run: run
                    .iter()
                    .map(|i| RenderItemDraw {
                        id: render_texture_id(&SocketRef {
                            path: path.to_vec(),
                            node: node.id.clone(),
                            handle: i.item.id.clone(),
                        }),
                        item: i.item.id.clone(),
                        ty: i.ty,
                        format: i.format,
                    })
                    .collect(),
                like: like.map(Box::new),
            }),
        })
    }

    /// The texture linked to a Texture input, if it gives an image.
    fn linked_texture(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        node: &str,
        input: &str,
    ) -> Option<TextureSource> {
        let edge = self.feed(path, node, input)?;
        let source_type = self.graph.resolver.infer_handle_type(
            self.graph.project,
            &edge.source,
            &edge.source_handle,
            &instance.bindings,
        );
        if !source_type.is_texture() {
            return None;
        }
        match self.output(path, &edge.source, &edge.source_handle) {
            TextureResolution::Source(TextureSource::Empty) | TextureResolution::Invalid { .. } => None,
            TextureResolution::Source(source) => Some(source),
        }
    }

    /// What a Render Texture's scale is relative to: its `like` texture, else
    /// the texture linked to its first item, else the canvas.
    fn like_of(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        node: &GraphNode,
        first: Option<&str>,
    ) -> Option<TextureSource> {
        self.linked_texture(path, instance, &node.id, RENDER_LIKE)
            .or_else(|| first.and_then(|f| self.linked_texture(path, instance, &node.id, f)))
    }

    /// The state of the simulation zone `zone` (its input's id) of the document
    /// computed at `path`, its items typed as they are there.
    pub fn simulation(&self, path: &[String], zone: &str) -> Option<Rc<SimulationState>> {
        let reference = SimulationRef { path: path.to_vec(), zone: zone.to_owned() };
        let id = reference.id();
        if let Some(found) = self.simulations.borrow().get(&id) {
            return found.clone();
        }
        // Sizes read other textures, which may be this simulation's own state:
        // asked for again while they're found, it has the canvas' size.
        let provisional = !self.building.borrow_mut().insert(id.clone());
        let state = self.build_simulation(&reference, provisional).map(Rc::new);
        if !provisional {
            self.building.borrow_mut().remove(&id);
            self.simulations.borrow_mut().insert(id, state.clone());
        }
        state
    }

    fn build_simulation(&self, reference: &SimulationRef, provisional: bool) -> Option<SimulationState> {
        let instance = self.instance(&reference.path)?;
        let analysed = instance.zones.zones.get(&reference.zone)?;
        if analysed.kind != ZoneKind::Simulation {
            return None;
        }
        let input = instance.doc.node(&reference.zone)?;
        let outputs = self.graph.resolver.resolve(self.graph.project, input, &instance.bindings);
        let items = zone_items(instance.doc.node(&analysed.output_id))
            .into_iter()
            .map(|item| {
                let ty = outputs
                    .resolved_outputs
                    .iter()
                    .find(|o| o.id == item.id)
                    .map_or(WgslType::F32, |o| o.ty);
                let (size, like) = if provisional {
                    (CANVAS_SIZE, None)
                } else {
                    self.item_sizing(&reference.path, &instance, input, &item)
                };
                StateItem {
                    format: item.format.as_deref().and_then(TextureFormat::from_id).unwrap_or_else(|| state_format(ty)),
                    texture: state_texture_id(reference, &item.id),
                    item,
                    ty,
                    size,
                    like: like.map(Box::new),
                }
            })
            .collect();
        Some(SimulationState { reference: reference.clone(), id: reference.id(), items })
    }

    /// The size of the texture keeping a state item, and the texture it's relative to.
    fn item_sizing(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        input: &GraphNode,
        item: &ZoneItem,
    ) -> (TextureSize, Option<TextureSource>) {
        let Some(mode) = ItemSizeMode::parse(item.size.as_deref()) else { return (CANVAS_SIZE, None) };
        let handles = item_size_handles(&item.id);
        let like = (mode == ItemSizeMode::Scale)
            .then(|| self.linked_texture(path, instance, &input.id, &handles.like))
            .flatten();
        (self.size_of(path, input, &handles, mode, like.as_ref()), like)
    }

    /// A state item of the simulation computed at `path` that a boundary node gives.
    fn state(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        node: &str,
        handle: &str,
        read: StateRead,
    ) -> TextureResolution {
        let Some(zone) = instance.zones.zone_of_boundary.get(node) else { return EMPTY };
        let Some(state) = self.simulation(path, zone) else { return EMPTY };
        let Some(item) = state.items.iter().find(|i| i.item.id == handle) else { return EMPTY };
        TextureResolution::Source(state_source(&state.reference, item, read))
    }

    fn media(&self, path: &[String], node: &GraphNode) -> TextureResolution {
        let Some(asset) = node
            .data
            .asset
            .as_deref()
            .filter(|id| self.graph.project.assets.iter().any(|a| a.id() == *id))
        else {
            return EMPTY;
        };
        if !node.is(crate::nodes::VIDEO) {
            return TextureResolution::Source(TextureSource::Media {
                id: media_texture_id(asset, None, ""),
                asset: asset.to_owned(),
                video: None,
                time: None,
            });
        }
        let video = node.data.playback.unwrap_or(DEFAULT_PLAYBACK);
        let owner = path.iter().map(String::as_str).chain([node.id.as_str()]).collect::<Vec<_>>().join("/");
        // A time that isn't a frame value can't be read: the link is invalid.
        let time = self
            .feed(path, &node.id, crate::nodes::VIDEO_TIME)
            .and_then(|_| self.sizing()?.input_arg(path, &node.id, crate::nodes::VIDEO_TIME, 0));
        TextureResolution::Source(TextureSource::Media {
            id: media_texture_id(asset, Some(&video), &owner),
            asset: asset.to_owned(),
            video: Some(video),
            time,
        })
    }

    /// An Index Switch's index while compiling: set by the calling group node, or stored while unlinked.
    fn known_index(&self, instance: &Instance<'g>, node: &GraphNode) -> Option<i64> {
        let constant = self.with_enums(instance, |e| e.constant_at(&node.id, INDEX_SWITCH_INDEX, &instance.constants));
        if constant.is_some() {
            return constant;
        }
        if instance.edges_into(&node.id).any(|e| e.target_handle == INDEX_SWITCH_INDEX) {
            return None;
        }
        Some(node.data.values.get(INDEX_SWITCH_INDEX).map_or(0, crate::types::InputValue::as_i64))
    }

    /// A switch over textures whose case is only known per pixel: it reads nothing.
    fn unknown_case(&self, node: &GraphNode, what: &str) -> TextureResolution {
        let label = resolve_descriptor(node, self.graph.project).label.clone();
        invalid(format!("A {label} picks a texture only by {what}."), "")
    }

    /// Whether a node's outputs at `path` depend on the state or iteration of
    /// a repeat zone they're computed in (or that the group node they're
    /// computed through is fed from). A zone's final values don't, nor does a
    /// simulation's state, the same over a frame.
    pub fn reads_zone_state(&self, path: &[String], node: &str, handle: &str) -> bool {
        let key = format!("{}|{node}|{handle}", path.join("/"));
        if let Some(known) = self.zone_state.borrow().get(&key) {
            return *known;
        }
        let Some(instance) = self.instance(path) else { return false };
        let reads = self.visit_zone_state(
            path,
            &instance,
            node,
            handle,
            &BTreeSet::new(),
            &mut HashMap::new(),
        );
        self.zone_state.borrow_mut().insert(key, reads);
        reads
    }

    /// The repeat zone a node is computed in, a Repeat Output's being around its loop.
    fn zone_of(instance: &Instance<'g>, id: &str) -> Option<String> {
        let zones = &instance.zones;
        let own = zones.zone_of_boundary.get(id).and_then(|z| zones.zones.get(z));
        let zone = match own {
            Some(own) if id == own.output_id => own.parent.clone(),
            Some(own) => Some(own.id.clone()),
            None => zones.node_zone.get(id).cloned().flatten(),
        };
        // A simulation is never in a repeat zone: a node in one is in none.
        zone.filter(|z| zones.zones.get(z).is_none_or(|zone| zone.kind != ZoneKind::Simulation))
    }

    fn visit_zone_state(
        &self,
        path: &[String],
        instance: &Instance<'g>,
        id: &str,
        handle: &str,
        // Zones whose final values are read: what they compute is complete.
        closed: &BTreeSet<String>,
        done: &mut HashMap<String, bool>,
    ) -> bool {
        let node = instance.doc.node(id);
        let is_input = node.is_some_and(|n| n.is("group-input"));
        let key = format!(
            "{id}|{}|{}",
            if is_input { handle } else { "" },
            closed.iter().cloned().collect::<Vec<_>>().join(",")
        );
        if let Some(known) = done.get(&key) {
            return *known;
        }
        done.insert(key.clone(), false);
        let zone = Self::zone_of(instance, id);
        let mut reads = zone.is_some_and(|z| !closed.contains(&z));
        if !reads && is_input && let Some((caller, parent)) = path.split_last() {
            reads = self.feeds_zone_state(parent, caller, handle);
        }
        if !reads {
            let zones = &instance.zones;
            let own = zones.zone_of_boundary.get(id).and_then(|z| zones.zones.get(z));
            let closing = own.filter(|z| z.output_id == id);
            let mut inner = closed.clone();
            if let Some(z) = closing {
                inner.insert(z.id.clone());
            }
            let mut feeds: Vec<&GraphEdge> = instance.edges_into(id).collect();
            if let Some(z) = closing {
                feeds.extend(instance.edges_into(&z.input_id));
            }
            reads = feeds
                .iter()
                .any(|e| self.visit_zone_state(path, instance, &e.source, &e.source_handle, &inner, done));
        }
        done.insert(key, reads);
        reads
    }

    /// Whether what feeds a group node's input at `path` reads a zone's state there.
    fn feeds_zone_state(&self, path: &[String], group_node: &str, input: &str) -> bool {
        self.feed(path, group_node, input)
            .is_some_and(|feed| self.reads_zone_state(path, &feed.source, &feed.source_handle))
    }

    fn memo(&self, key: String, compute: impl FnOnce() -> TextureResolution) -> TextureResolution {
        if let Some(done) = self.resolved.borrow().get(&key) {
            return done.clone();
        }
        // A texture sized by itself has no size to start from.
        if !self.visiting.borrow_mut().insert(key.clone()) {
            return invalid("A texture can't depend on itself.", "");
        }
        let result = compute();
        self.visiting.borrow_mut().remove(&key);
        self.resolved.borrow_mut().insert(key, result.clone());
        result
    }
}

/// `resolution` reached through `edge`: an invalid one found right past it is shown on it.
fn through(edge: &GraphEdge, resolution: TextureResolution) -> TextureResolution {
    match resolution {
        TextureResolution::Invalid { reason, edge: found } if found.is_empty() => {
            TextureResolution::Invalid { reason, edge: edge.id.clone() }
        }
        other => other,
    }
}

/// The texture keeping a state item, as read in `read`'s frame.
pub fn state_source(simulation: &SimulationRef, item: &StateItem, read: StateRead) -> TextureSource {
    TextureSource::State {
        id: state_read_id(&item.texture, read),
        texture: item.texture.clone(),
        read,
        format: item.format,
        size: item.size.clone(),
        simulation: simulation.clone(),
    }
}
