//! What links do with textures, and the renders a frame needs.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use super::sources::{SocketRef, TextureResolution, TextureSource, TextureSources};
use crate::codegen::CodegenGraph;
use crate::graph::GraphEdge;

/// What a link does with a texture, at a group instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureLink {
    /// A texture linked to a value: sampled at the pixel.
    Sample,
    /// A value linked to a Texture input: rendered to a texture first.
    Render { id: String },
    /// Nothing it can read: it's drawn red, with why.
    Invalid { reason: String },
}

/// The textures of the graph as compiled, the same `TextureSources`
/// codegen reads: what each link does with them, and the renders the frame needs.
pub struct PassAnalysis<'g> {
    pub sources: TextureSources<'g>,
    graph: CodegenGraph<'g>,
    renders: RefCell<Option<BTreeMap<String, SocketRef>>>,
}

impl<'g> PassAnalysis<'g> {
    pub fn new(graph: CodegenGraph<'g>) -> Self {
        Self { sources: TextureSources::new(graph), graph, renders: RefCell::new(None) }
    }

    /// What `edge`, of the document computed at `path`, does with a texture, if anything.
    pub fn link(&self, path: &[String], edge: &GraphEdge) -> Option<TextureLink> {
        let instance = self.sources.instance(path)?;
        let target = instance.doc.node(&edge.target)?;
        let resolver = self.graph.resolver;
        let target_type = resolver
            .resolve(self.graph.project, target, &instance.bindings)
            .input_type(&edge.target_handle);
        let source_type = resolver.infer_handle_type(
            self.graph.project,
            &edge.source,
            &edge.source_handle,
            &instance.bindings,
        );
        if target_type.is_some_and(|t| t.is_texture()) {
            if self.sources.feed(path, &edge.target, &edge.target_handle)?.id != edge.id {
                return None;
            }
            return match self.sources.input(path, &edge.target, &edge.target_handle) {
                TextureResolution::Invalid { reason, edge: found } if found == edge.id => {
                    Some(TextureLink::Invalid { reason })
                }
                TextureResolution::Source(TextureSource::Render { id, .. }) => {
                    Some(TextureLink::Render { id })
                }
                _ => None,
            };
        }
        if !source_type.is_texture() {
            return None;
        }
        Some(match self.sources.output(path, &edge.source, &edge.source_handle) {
            TextureResolution::Invalid { reason, .. } => TextureLink::Invalid { reason },
            TextureResolution::Source(_) => TextureLink::Sample,
        })
    }

    /// The values the frame renders to textures, by texture id: a pass each.
    pub fn renders(&self) -> BTreeMap<String, SocketRef> {
        if let Some(cached) = &*self.renders.borrow() {
            return cached.clone();
        }
        let mut renders = BTreeMap::new();
        let mut seen = HashSet::new();
        let outputs = self.graph.project.root().nodes.iter().filter(|n| n.is("frag-output")).map(|n| n.id.clone()).collect();
        self.walk(&[], outputs, &mut seen, &mut renders);
        *self.renders.borrow_mut() = Some(renders.clone());
        renders
    }

    fn walk(
        &self,
        path: &[String],
        seeds: Vec<String>,
        seen: &mut HashSet<String>,
        renders: &mut BTreeMap<String, SocketRef>,
    ) {
        let Some(instance) = self.sources.instance(path) else { return };
        let mut queue = seeds;
        let mut next = 0;
        while let Some(id) = queue.get(next).cloned() {
            next += 1;
            if !seen.insert(format!("{}|{id}", path.join("/"))) {
                continue;
            }
            for edge in instance.edges_into(&id) {
                if let Some(TextureLink::Render { .. }) = self.link(path, edge)
                    && let TextureResolution::Source(TextureSource::Render { id, target, .. }) =
                        self.sources.input(path, &edge.target, &edge.target_handle)
                {
                    renders.insert(id, target);
                }
                queue.push(edge.source.clone());
            }
            let def = instance.doc.node(&id).and_then(|n| n.definition_ref()).and_then(|r| self.graph.project.document(r));
            if let Some(def) = def {
                let outputs = def.nodes.iter().filter(|n| n.is("group-output")).map(|n| n.id.clone()).collect();
                let inner: Vec<String> = path.iter().cloned().chain([id.clone()]).collect();
                self.walk(&inner, outputs, seen, renders);
            }
        }
    }
}
