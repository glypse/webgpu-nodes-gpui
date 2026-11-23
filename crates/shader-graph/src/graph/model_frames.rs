//! Putting nodes of the document in view in frames, and taking them out.

use std::collections::{BTreeMap, HashSet};

use super::frames::{FrameAnalysis, LAYOUT_FRAME, analyze_frames};
use super::model::GraphModel;
use super::{GraphDocument, GraphNode, Point};
use crate::nodes::default_data_for_type;

impl GraphModel {
    /// The frames of a document and what they hold.
    pub fn frame_analysis(&self, doc: &GraphDocument) -> FrameAnalysis {
        analyze_frames(&doc.nodes)
    }

    pub fn active_frames(&self) -> FrameAnalysis {
        self.frame_analysis(self.active_document())
    }

    /// The ids given, plus every node the frames among them hold.
    pub(crate) fn with_frame_contents(&self, ids: &[String]) -> HashSet<String> {
        let analysis = self.active_frames();
        let mut all: HashSet<String> = ids.iter().cloned().collect();
        for id in ids {
            if let Some(frame) = analysis.frames.get(id) {
                all.extend(frame.descendants.iter().cloned());
            }
        }
        all
    }

    /// Put nodes of the active document in a new frame, itself in the
    /// innermost frame they share, and select it. Returns its id.
    pub fn frame_nodes(&mut self, node_ids: &[String]) -> Option<String> {
        if self.is_locked() {
            return None;
        }
        let key = self.active_definition_ref();
        let doc = self.project.document(&key)?;
        let analysis = self.frame_analysis(doc);
        let wanted: HashSet<String> = node_ids
            .iter()
            .filter(|id| doc.has_node(id))
            .cloned()
            .collect();
        let moved: HashSet<String> = analysis.outermost(&wanted).into_iter().collect();
        let members: Vec<&GraphNode> = doc.nodes.iter().filter(|n| moved.contains(&n.id)).collect();
        if members.is_empty() {
            return None;
        }
        let parent = analysis.common_frame(moved.iter().map(String::as_str));
        let position = Point::new(
            members
                .iter()
                .map(|n| n.position.x)
                .fold(f32::INFINITY, f32::min),
            members
                .iter()
                .map(|n| n.position.y)
                .fold(f32::INFINITY, f32::min),
        );
        let mut frame = GraphNode::new(LAYOUT_FRAME, position);
        frame.data = default_data_for_type(LAYOUT_FRAME);
        frame.frame = parent;
        let id = frame.id.clone();
        let doc = self.doc_mut(&key)?;
        for node in &mut doc.nodes {
            if moved.contains(&node.id) {
                node.frame = Some(id.clone());
            }
        }
        doc.nodes.push(frame);
        self.selected = vec![id.clone()];
        self.changed();
        Some(id)
    }

    /// Put nodes of the active document in the frame `frame_id`, or in none
    /// with `None`. A frame can't go in itself or a frame it holds; nodes in
    /// the frame of another node given stay in it. Returns whether any moved.
    pub fn set_nodes_frame(&mut self, node_ids: &[String], frame_id: Option<&str>) -> bool {
        let moved = self.assign_frame(node_ids, frame_id);
        if moved {
            self.changed();
        }
        moved
    }

    /// Move nodes of the active document to `positions` and put each group of
    /// nodes in its frame, as one edit.
    pub fn drop_nodes(
        &mut self,
        positions: &BTreeMap<String, Point>,
        targets: &[(Option<String>, Vec<String>)],
    ) -> bool {
        if self.is_locked() {
            return false;
        }
        let key = self.active_definition_ref();
        if let Some(doc) = self.doc_mut(&key) {
            for node in &mut doc.nodes {
                if let Some(p) = positions.get(&node.id) {
                    node.position = *p;
                }
            }
        }
        for (frame, ids) in targets {
            self.assign_frame(ids, frame.as_deref());
        }
        self.changed();
        true
    }

    fn assign_frame(&mut self, node_ids: &[String], frame_id: Option<&str>) -> bool {
        if self.is_locked() {
            return false;
        }
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else {
            return false;
        };
        if frame_id.is_some_and(|f| !doc.node(f).is_some_and(|n| n.is(LAYOUT_FRAME))) {
            return false;
        }
        let analysis = self.frame_analysis(doc);
        let around: Vec<String> = frame_id.map_or_else(Vec::new, |f| {
            std::iter::once(f.to_owned())
                .chain(analysis.frames_around(f))
                .collect()
        });
        let wanted: HashSet<String> = node_ids
            .iter()
            .filter(|id| doc.has_node(id))
            .cloned()
            .collect();
        let moved: HashSet<String> = analysis
            .outermost(&wanted)
            .into_iter()
            .filter(|id| {
                !around.contains(id) && analysis.frame_of.get(id).map(String::as_str) != frame_id
            })
            .collect();
        if moved.is_empty() {
            return false;
        }
        let Some(doc) = self.doc_mut(&key) else {
            return false;
        };
        for node in doc.nodes.iter_mut().filter(|n| moved.contains(&n.id)) {
            node.frame = frame_id.map(str::to_owned);
        }
        true
    }

    /// Take nodes of the active document out of their frame, into the frame
    /// around it if any. Returns whether any moved.
    pub fn remove_from_frame(&mut self, node_ids: &[String]) -> bool {
        if self.is_locked() {
            return false;
        }
        let key = self.active_definition_ref();
        let Some(doc) = self.project.document(&key) else {
            return false;
        };
        let analysis = self.frame_analysis(doc);
        let wanted: HashSet<String> = node_ids.iter().cloned().collect();
        let moved: Vec<(String, Option<String>)> = analysis
            .outermost(&wanted)
            .into_iter()
            .filter_map(|id| {
                let frame = analysis.frame_of.get(&id)?;
                Some((id, analysis.frame_of.get(frame).cloned()))
            })
            .collect();
        if moved.is_empty() {
            return false;
        }
        let Some(doc) = self.doc_mut(&key) else {
            return false;
        };
        for (id, frame) in moved {
            if let Some(node) = doc.node_mut(&id) {
                node.frame = frame;
            }
        }
        self.changed();
        true
    }
}
