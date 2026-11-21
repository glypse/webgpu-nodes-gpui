//! What a shader draws besides the fragment output: a socket's value, some
//! of a simulation's state items, or some items of a Render Texture.

use crate::passes::{RenderItemDraw, SimulationRef, SocketRef, StateItem, texel_type};
use crate::types::WgslType;

/// A simulation's state at its start (its initial values), or this frame's (its step).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimulationStage {
    Start,
    Step,
}

impl SimulationStage {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Step => "step",
        }
    }
}

/// Some of a simulation's state items, at a stage, each drawn into the texture keeping it.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationTarget {
    pub simulation: SimulationRef,
    pub stage: SimulationStage,
    pub items: Vec<StateItem>,
}

/// Some of a Render Texture node's items, each drawn into the texture it gives.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderTextureTarget {
    pub path: Vec<String>,
    pub node: String,
    pub items: Vec<RenderItemDraw>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShaderTarget {
    /// An output, drawn as a colour.
    Socket(SocketRef),
    Simulation(SimulationTarget),
    RenderTexture(RenderTextureTarget),
}

/// What a shader computes in a group nested at `path` (group node ids, from
/// the document at hand), besides its seeds: each group on the way gives it
/// its caller as fields of its outputs it doesn't have, so a shader can draw
/// any socket, or compute any simulation, of any group instance.
#[derive(Clone, Debug)]
pub(super) struct Probe {
    pub path: Vec<String>,
    pub kind: ProbeKind,
}

#[derive(Clone, Debug)]
pub(super) enum ProbeKind {
    Socket { node: String, handle: String },
    Simulation { zone: String, stage: SimulationStage, items: Vec<StateItem> },
    Textures { node: String, items: Vec<RenderItemDraw> },
}

impl Probe {
    pub fn of(target: &ShaderTarget) -> Self {
        match target {
            ShaderTarget::Socket(s) => Self {
                path: s.path.clone(),
                kind: ProbeKind::Socket { node: s.node.clone(), handle: s.handle.clone() },
            },
            ShaderTarget::Simulation(t) => Self {
                path: t.simulation.path.clone(),
                kind: ProbeKind::Simulation {
                    zone: t.simulation.zone.clone(),
                    stage: t.stage,
                    items: t.items.clone(),
                },
            },
            ShaderTarget::RenderTexture(t) => Self {
                path: t.path.clone(),
                kind: ProbeKind::Textures { node: t.node.clone(), items: t.items.clone() },
            },
        }
    }

    /// The node the probe is computed at, where it's computed: the group node
    /// it goes through, else the socket's own. A simulation's isn't a node here.
    pub fn seed_node(&self) -> &str {
        match (self.path.first(), &self.kind) {
            (Some(inner), _) => inner,
            (None, ProbeKind::Simulation { .. }) => "",
            (None, ProbeKind::Socket { node, .. } | ProbeKind::Textures { node, .. }) => node,
        }
    }

    /// The probe of the group node it goes through at its level, as that group computes it.
    pub fn inner(&self) -> Self {
        Self { path: self.path.iter().skip(1).cloned().collect(), kind: self.kind.clone() }
    }

    pub fn key(&self) -> String {
        let at = self.path.join("/");
        match &self.kind {
            ProbeKind::Socket { node, handle } => {
                let at = self.path.iter().chain([node]).cloned().collect::<Vec<_>>().join("/");
                format!("{at}:{handle}")
            }
            ProbeKind::Textures { node, items } => {
                let at = self.path.iter().chain([node]).cloned().collect::<Vec<_>>().join("/");
                let items: Vec<&str> = items.iter().map(|i| i.item.as_str()).collect();
                format!("{at}:textures:{}", items.join(","))
            }
            ProbeKind::Simulation { zone, stage, items } => {
                let at = if at.is_empty() { zone.clone() } else { format!("{at}/{zone}") };
                let items: Vec<&str> = items.iter().map(|i| i.texture.as_str()).collect();
                format!("{at}:{}:{}", stage.id(), items.join(","))
            }
        }
    }

    /// The types of the values a probe gives: a colour, or a texel per item.
    pub fn types(&self) -> Vec<WgslType> {
        match &self.kind {
            ProbeKind::Socket { .. } => vec![WgslType::Vec4f],
            ProbeKind::Simulation { items, .. } => items.iter().map(|i| texel_type(i.format)).collect(),
            ProbeKind::Textures { items, .. } => items.iter().map(|i| texel_type(i.format)).collect(),
        }
    }

    /// The output struct field of a group giving its caller the probe's `index`th value.
    pub fn field(index: usize) -> String {
        format!("viewer{index}")
    }
}
