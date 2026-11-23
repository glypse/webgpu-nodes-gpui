use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::codegen::CodegenGraph;
use crate::enums::{MATCH, WIDTH_SWITCH};
use crate::graph::{GROUP_INPUT, GraphEdge, GraphNode, NODE_GROUP};
use crate::nodes::{
    COMPONENT_COUNT, FALLBACK, FALLBACK_DEFAULT, FALLBACK_VALUE, TEXTURE_SIZE, TEXTURE_SIZE_LEVEL,
    VIDEO, is_video_metadata, resolve_descriptor,
};
use crate::passes::TextureSources;
use crate::zones::{ITERATIONS_HANDLE, REPEAT_INPUT, ZoneKind, zone_items, zone_kind_of};

/// Whether a value is the same for the whole frame, and if not, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameResult {
    Frame,
    Texture,
    PerPixel(String),
}

impl FrameResult {
    pub fn is_frame(&self) -> bool {
        matches!(self, Self::Frame)
    }
}

/// Why a link into an input needing a frame value is invalid.
pub const FRAME_VALUE_REQUIRED: &str = "Needs a value that's the same for the whole frame";

/// What a link does with frame values, at a group instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameLink {
    /// It carries a value that's the same for the whole frame.
    Frame,
    /// It gives a value computed per pixel to an input that needs a frame value.
    Invalid { reason: String },
}

#[cfg(test)]
const TEXTURE_REASON: &str = "A texture isn't a value.";

/// Nodes whose outputs the runtime knows before drawing, with no inputs of their own to check.
const FRAME_SOURCES: [&str; 3] = ["time", "delta", "resolution"];

fn no(reason: impl Into<String>) -> FrameResult {
    FrameResult::PerPixel(reason.into())
}

/// Which values of the graph are the same for a whole frame (the *frame*
/// tier of values, computed on the CPU), per group instance. Build one per
/// graph state.
pub struct FrameValueAnalysis<'g> {
    graph: CodegenGraph<'g>,
    sources: Rc<TextureSources<'g>>,
    memo: RefCell<HashMap<String, FrameResult>>,
    /// Results found while a zone is assumed to be a frame zone: it may turn
    /// out not to be, which would make what's in it wrong to keep.
    scratch: RefCell<HashMap<String, FrameResult>>,
    assuming: RefCell<HashSet<String>>,
    visiting: RefCell<HashSet<String>>,
}

fn key(path: &[String]) -> String {
    path.join("/")
}

impl<'g> FrameValueAnalysis<'g> {
    pub fn new(graph: CodegenGraph<'g>, sources: Rc<TextureSources<'g>>) -> Self {
        Self {
            graph,
            sources,
            memo: RefCell::default(),
            scratch: RefCell::default(),
            assuming: RefCell::default(),
            visiting: RefCell::default(),
        }
    }

    /// An analysis with texture sources of its own.
    pub fn of(graph: CodegenGraph<'g>) -> Self {
        Self::new(graph, Rc::new(TextureSources::new(graph)))
    }

    /// Whether an output of a node of the document computed at `path` is a frame value.
    pub fn is_frame_value(&self, path: &[String], node: &str, handle: &str) -> bool {
        self.output(path, node, handle).is_frame()
    }

    /// Why an output isn't a frame value, if it isn't.
    #[cfg(test)]
    pub(crate) fn reason(&self, path: &[String], node: &str, handle: &str) -> Option<String> {
        match self.output(path, node, handle) {
            FrameResult::Frame => None,
            FrameResult::Texture => Some(TEXTURE_REASON.to_owned()),
            FrameResult::PerPixel(reason) => Some(reason),
        }
    }

    /// What `edge`, of the document computed at `path`, does with frame values, if anything.
    pub fn link(&self, path: &[String], edge: &GraphEdge) -> Option<FrameLink> {
        let instance = self.sources.instance(path)?;
        let target = instance.doc.node(&edge.target)?;
        match self.output(path, &edge.source, &edge.source_handle) {
            FrameResult::Frame => Some(FrameLink::Frame),
            FrameResult::Texture => None,
            FrameResult::PerPixel(reason) => {
                let descriptor = resolve_descriptor(target, self.graph.project);
                let input = descriptor
                    .inputs
                    .iter()
                    .find(|i| i.id == edge.target_handle)?;
                input.frame.then(|| FrameLink::Invalid {
                    reason: format!("{FRAME_VALUE_REQUIRED}: {}.", reason.trim_end_matches('.')),
                })
            }
        }
    }

    /// Whether what a node's input reads is a frame value: an unlinked one is.
    #[cfg(test)]
    pub(crate) fn is_frame_input(&self, path: &[String], node: &str, input: &str) -> bool {
        self.input(path, node, input).is_frame()
    }

    /// The inputs of a node of the document computed at `path` its value
    /// depends on: those a Match, Width Switch or Fallback doesn't pick are
    /// left out, as is whatever a Component Count reads (only its type).
    pub fn inputs_read(&self, path: &[String], node_id: &str) -> Vec<String> {
        let Some(instance) = self.sources.instance(path) else {
            return Vec::new();
        };
        let Some(node) = instance.doc.node(node_id) else {
            return Vec::new();
        };
        let ids: Vec<String> = self
            .graph
            .resolver
            .resolve(self.graph.project, node, &instance.bindings)
            .resolved_inputs
            .iter()
            .map(|i| i.id.clone())
            .collect();
        match node.node_type.as_str() {
            MATCH => {
                let picked = self
                    .sources
                    .with_enums(&instance, |e| e.matched_case(node_id, &instance.constants));
                picked.map_or(ids, |p| vec![p])
            }
            WIDTH_SWITCH => {
                let picked = self
                    .sources
                    .with_enums(&instance, |e| e.width_case(node_id));
                picked.map_or(ids, |p| vec![p])
            }
            COMPONENT_COUNT => Vec::new(),
            FALLBACK => {
                let linked = instance
                    .edges_into(node_id)
                    .any(|e| e.target_handle == FALLBACK_VALUE);
                vec![
                    if linked {
                        FALLBACK_VALUE
                    } else {
                        FALLBACK_DEFAULT
                    }
                    .to_owned(),
                ]
            }
            _ => ids,
        }
    }

    fn input(&self, path: &[String], node_id: &str, input_id: &str) -> FrameResult {
        let Some(instance) = self.sources.instance(path) else {
            return FrameResult::Frame;
        };
        for edge in instance
            .edges_into(node_id)
            .filter(|e| e.target_handle == input_id)
        {
            let result = self.output(path, &edge.source, &edge.source_handle);
            if !result.is_frame() {
                return result;
            }
        }
        FrameResult::Frame
    }

    fn output(&self, path: &[String], node_id: &str, handle: &str) -> FrameResult {
        let cache_key = format!("{}|{node_id}|{handle}", key(path));
        let assuming = !self.assuming.borrow().is_empty();
        if let Some(known) = self.memo.borrow().get(&cache_key) {
            return known.clone();
        }
        if let Some(known) = self.scratch.borrow().get(&cache_key) {
            return known.clone();
        }
        if !self.visiting.borrow_mut().insert(cache_key.clone()) {
            return no("It depends on itself.");
        }
        let result = self.compute(path, node_id, handle);
        self.visiting.borrow_mut().remove(&cache_key);
        let cache = if assuming { &self.scratch } else { &self.memo };
        cache.borrow_mut().insert(cache_key, result.clone());
        result
    }

    fn compute(&self, path: &[String], node_id: &str, handle: &str) -> FrameResult {
        let Some(instance) = self.sources.instance(path) else {
            return no("There's nothing to read.");
        };
        let Some(node) = instance.doc.node(node_id) else {
            return no("There's nothing to read.");
        };
        let types = self
            .graph
            .resolver
            .resolve(self.graph.project, node, &instance.bindings);
        if types
            .resolved_outputs
            .iter()
            .find(|o| o.id == handle)
            .is_some_and(|o| o.ty.is_texture())
        {
            return FrameResult::Texture;
        }
        let label = || resolve_descriptor(node, self.graph.project).label.clone();
        let ty = node.node_type.as_str();
        if FRAME_SOURCES.contains(&ty) {
            return FrameResult::Frame;
        }
        // A texture's size is known to the runtime, at the level asked for.
        if ty == TEXTURE_SIZE {
            return self.input(path, node_id, TEXTURE_SIZE_LEVEL);
        }
        if ty == VIDEO {
            return if is_video_metadata(handle) {
                FrameResult::Frame
            } else {
                no(format!("{}'s frame is read per pixel.", label()))
            };
        }
        if ty == GROUP_INPUT {
            return match path.split_last() {
                None => no("A group input has no value outside a group."),
                Some((parent, outer)) => self.input(outer, parent, handle),
            };
        }
        if ty == NODE_GROUP {
            return self.group_output(path, node, handle);
        }
        match zone_kind_of(ty) {
            Some(ZoneKind::Simulation) => {
                return no(format!("{} keeps its state for each pixel.", label()));
            }
            Some(ZoneKind::Repeat) => return self.zone_value(path, node_id),
            Some(ZoneKind::Passes) => {
                return no(format!("{} repeats passes, not a computation.", label()));
            }
            None => {}
        }
        if resolve_descriptor(node, self.graph.project).cpu.is_none() {
            return no(format!("{} is computed for each pixel.", label()));
        }
        for id in self.inputs_read(path, node_id) {
            let result = self.input(path, node_id, &id);
            if !result.is_frame() {
                return result;
            }
        }
        FrameResult::Frame
    }

    fn group_output(&self, path: &[String], node: &GraphNode, handle: &str) -> FrameResult {
        let inner: Vec<String> = path.iter().cloned().chain([node.id.clone()]).collect();
        let Some(definition) = node
            .definition_ref()
            .and_then(|r| self.graph.project.document(r))
        else {
            return FrameResult::Frame;
        };
        definition
            .output_node_of(handle)
            .map_or(FrameResult::Frame, |o| self.input(&inner, &o.id, handle))
    }

    /// An output of a repeat zone's boundary. A zone whose inputs and next
    /// values are all frame values runs on the CPU, and so are its outputs and
    /// what's in it, its iteration included; otherwise none of them is.
    fn zone_value(&self, path: &[String], node_id: &str) -> FrameResult {
        let Some(instance) = self.sources.instance(path) else {
            return no("The zone isn't paired.");
        };
        let Some(zone) = instance
            .zones
            .zone_of_boundary
            .get(node_id)
            .and_then(|z| instance.zones.zones.get(z))
        else {
            return no("The zone isn't paired.");
        };
        let inside = instance
            .doc
            .node(node_id)
            .is_some_and(|n| n.is(REPEAT_INPUT));
        if inside
            && self
                .assuming
                .borrow()
                .contains(&format!("{}|{}", key(path), zone.id))
        {
            return FrameResult::Frame;
        }
        self.zone_result(path, &zone.id)
    }

    fn zone_result(&self, path: &[String], zone_id: &str) -> FrameResult {
        let cache_key = format!("{}|zone|{zone_id}", key(path));
        if let Some(known) = self
            .memo
            .borrow()
            .get(&cache_key)
            .or(self.scratch.borrow().get(&cache_key))
        {
            return known.clone();
        }
        let Some(instance) = self.sources.instance(path) else {
            return no("The zone isn't paired.");
        };
        let Some(zone) = instance.zones.zones.get(zone_id) else {
            return no("The zone isn't paired.");
        };
        let Some(output) = instance.doc.node(&zone.output_id) else {
            return no("The zone isn't paired.");
        };
        let items = zone_items(Some(output));
        let assumed = format!("{}|{zone_id}", key(path));
        self.assuming.borrow_mut().insert(assumed.clone());
        let mut result = FrameResult::Frame;
        let handles =
            std::iter::once(ITERATIONS_HANDLE.to_owned()).chain(items.iter().map(|i| i.id.clone()));
        for id in handles {
            let read = self.input(path, &zone.input_id, &id);
            if !read.is_frame() {
                result = read;
            }
        }
        if result.is_frame() {
            for item in &items {
                let read = self.input(path, &zone.output_id, &item.id);
                if !read.is_frame() {
                    result = read;
                }
            }
        }
        self.assuming.borrow_mut().remove(&assumed);
        if self.assuming.borrow().is_empty() {
            self.scratch.borrow_mut().clear();
            self.memo.borrow_mut().insert(cache_key, result.clone());
        } else {
            self.scratch.borrow_mut().insert(cache_key, result.clone());
        }
        result
    }
}
