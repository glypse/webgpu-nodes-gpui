//! How a frame is drawn: passes in order, each reading the textures earlier
//! ones wrote (or, for a simulation's state, the previous frame's), the last
//! drawing the frame.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use super::generate::{CodegenError, CodegenGraph, ShaderOptions, generate_shader_with};
use super::literals::UniformValue;
use super::program::{ProgramParts, ShaderProgram, build_program};
use super::target::{RenderTextureTarget, ShaderTarget, SimulationStage, SimulationTarget};
use crate::assets::VideoPlayback;
use crate::frame::{FrameCompiler, FrameCompilerOptions, FrameProgram};
use crate::passes::{
    CANVAS_SIZE, FrameArg, SimulationRef, StateItem, StateRead, TextureFormat, TextureSize,
    TextureSource, TextureSources, attachment_groups, state_read_id, state_source,
};

/// What the last pass of a plan writes: the frame drawn.
pub const PLAN_TARGET: &str = "target";

/// A texture a plan reads or writes.
#[derive(Clone, Debug, PartialEq)]
pub enum ResourceDecl {
    /// Written by a pass, then read.
    Transient {
        id: String,
        size: TextureSize,
        format: TextureFormat,
        /// Whether it has a full chain of mip levels, made after its pass.
        mips: bool,
    },
    /// A project asset's image, which the runtime is given; a video's frame at
    /// the project's time or, with `time`, at that project time instead.
    Media {
        id: String,
        asset: String,
        video: Option<VideoPlayback>,
        time: Option<FrameArg>,
    },
    /// A simulation's state item, as of the previous frame or this one: one of
    /// the two textures keeping it (`texture`), which swap roles as frames go
    /// by. The simulation starts over when the size they resolve to changes.
    State {
        id: String,
        texture: String,
        read: StateRead,
        format: TextureFormat,
        size: TextureSize,
        /// The simulation it's state of, by `SimulationRef::id`.
        simulation: String,
    },
    Empty {
        id: String,
    },
}

impl ResourceDecl {
    pub fn id(&self) -> &str {
        match self {
            Self::Transient { id, .. }
            | Self::Media { id, .. }
            | Self::State { id, .. }
            | Self::Empty { id } => id,
        }
    }
}

/// The simulation a pass computes something of, by `SimulationRef::id`; a
/// start pass draws its initial state, only on the frames it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassSimulation {
    pub id: String,
    pub stage: Option<SimulationStage>,
}

/// A program drawing into textures (`writes`, or the frame for `PLAN_TARGET`).
#[derive(Clone, Debug, PartialEq)]
pub struct PassDecl {
    pub id: String,
    pub program: ShaderProgram,
    /// The lines of the program's code each node's statements are on, by node id.
    pub line_map: BTreeMap<String, Range<usize>>,
    /// The resources the program reads, by id.
    pub reads: Vec<String>,
    /// The resources it draws into, a colour attachment each, in the program's order.
    pub writes: Vec<String>,
    /// It reads the seconds since the previous step of the simulation it computes something of.
    pub simulation: Option<PassSimulation>,
}

/// Passes rendering values linked to Texture inputs and computing
/// simulations' state, then the one drawing the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    pub resources: Vec<ResourceDecl>,
    /// In order; the last draws the frame.
    pub passes: Vec<PassDecl>,
    /// The uniform values of every pass: each program's block holds those it reads.
    pub uniforms: Vec<(String, UniformValue)>,
    /// The frame values the passes' programs read, computed on the CPU for each pass.
    pub frame: FrameProgram,
}

impl RenderPlan {
    /// The pass drawing the frame.
    pub fn target(&self) -> Option<&PassDecl> {
        self.passes.last()
    }

    /// Whether the plan keeps state between frames, which drawing it then needs.
    pub fn simulates(&self) -> bool {
        self.resources
            .iter()
            .any(|r| matches!(r, ResourceDecl::State { .. }))
    }

    pub fn resource(&self, id: &str) -> Option<&ResourceDecl> {
        self.resources.iter().find(|r| r.id() == id)
    }
}

fn resource_of(source: &TextureSource) -> ResourceDecl {
    match source {
        TextureSource::Render {
            id,
            size,
            format,
            mips,
            ..
        } => ResourceDecl::Transient {
            id: id.clone(),
            size: size.clone(),
            format: *format,
            mips: *mips,
        },
        TextureSource::Media {
            id,
            asset,
            video,
            time,
        } => ResourceDecl::Media {
            id: id.clone(),
            asset: asset.clone(),
            video: *video,
            time: time.clone(),
        },
        TextureSource::State {
            id,
            texture,
            read,
            format,
            size,
            simulation,
        } => ResourceDecl::State {
            id: id.clone(),
            texture: texture.clone(),
            read: *read,
            format: *format,
            size: size.clone(),
            simulation: simulation.id(),
        },
        TextureSource::Empty => ResourceDecl::Empty { id: source.id() },
    }
}

struct PlanBuilder<'a, 'g> {
    graph: CodegenGraph<'g>,
    options: &'a ShaderOptions,
    sources: &'a TextureSources<'g>,
    frame: &'a FrameCompiler<'g>,
    passes: Vec<PassDecl>,
    resources: Vec<ResourceDecl>,
    uniforms: Vec<(String, UniformValue)>,
    visited: HashSet<String>,
    error: Option<CodegenError>,
}

impl PlanBuilder<'_, '_> {
    fn set_resource(&mut self, resource: ResourceDecl) {
        super::upsert(&mut self.resources, resource, |a, b| a.id() == b.id());
    }

    /// The run of a simulation's items, in its passes, holding `texture`'s.
    fn run_of(&self, reference: &SimulationRef, texture: &str) -> Option<Vec<StateItem>> {
        let state = self.sources.simulation(&reference.path, &reference.zone)?;
        attachment_groups(&state.items)
            .into_iter()
            .find(|run| run.iter().any(|i| i.texture == texture))
    }

    /// Declares a texture a pass reads, and adds the passes making it before the one reading it.
    fn include(&mut self, source: &TextureSource) {
        self.set_resource(resource_of(source));
        match source {
            TextureSource::Render {
                id,
                size,
                mips,
                target,
                draw,
                ..
            } => match draw {
                Some(draw) => {
                    let ids: Vec<String> = draw.run.iter().map(|i| i.id.clone()).collect();
                    self.visit(
                        format!("textures:{}", ids.join(",")),
                        ids,
                        Some(ShaderTarget::RenderTexture(RenderTextureTarget {
                            path: draw.path.clone(),
                            node: draw.node.clone(),
                            items: draw.run.clone(),
                        })),
                        None,
                        Some(size.clone()),
                        draw.like.as_deref().cloned(),
                    );
                    for item in &draw.run {
                        self.set_resource(ResourceDecl::Transient {
                            id: item.id.clone(),
                            size: size.clone(),
                            format: item.format,
                            mips: *mips,
                        });
                    }
                }
                None => self.visit(
                    id.clone(),
                    vec![id.clone()],
                    Some(ShaderTarget::Socket(target.clone())),
                    None,
                    Some(size.clone()),
                    None,
                ),
            },
            TextureSource::State {
                simulation,
                texture,
                read,
                ..
            } => {
                if let Some(items) = self.run_of(simulation, texture) {
                    // What's read as the previous frame's state is the last frame's step, so
                    // it has to be stepped every frame, read as its current state or not.
                    if *read == StateRead::Previous {
                        self.visit_simulation(simulation, SimulationStage::Start, &items);
                    }
                    self.visit_simulation(simulation, SimulationStage::Step, &items);
                }
            }
            TextureSource::Media { .. } | TextureSource::Empty => {}
        }
    }

    fn visit(
        &mut self,
        key: String,
        writes: Vec<String>,
        target: Option<ShaderTarget>,
        simulation: Option<PassSimulation>,
        target_size: Option<TextureSize>,
        // A texture the pass doesn't read, but its size follows.
        sized_by: Option<TextureSource>,
    ) {
        if !self.visited.insert(key.clone()) {
            return;
        }
        let generated = generate_shader_with(
            self.graph,
            self.options,
            self.sources,
            target.as_ref(),
            &target_size.unwrap_or(CANVAS_SIZE),
            self.options.hoist_frame_values.then_some(self.frame),
        );
        let generated = match generated {
            Ok(generated) => generated,
            Err(error) => {
                self.error.get_or_insert(error);
                return;
            }
        };
        let read: Vec<TextureSource> = generated
            .textures
            .iter()
            .map(|t| t.source.clone())
            .collect();
        for source in read.iter().chain(sized_by.as_ref()) {
            self.include(source);
        }
        for entry in &generated.uniforms {
            super::upsert(&mut self.uniforms, entry.clone(), |a, b| a.0 == b.0);
        }
        let program = build_program(
            &generated.fragment,
            &generated.uniforms,
            &ProgramParts {
                textures: &generated.textures,
                samplers: &generated.samplers,
                targets: generated.targets.as_deref(),
                frame_values: &generated.frame_values,
            },
        );
        let simulation = simulation.or_else(|| {
            generated
                .simulation
                .map(|id| PassSimulation { id, stage: None })
        });
        self.passes.push(PassDecl {
            id: format!("pass:{key}"),
            program,
            line_map: generated.line_map,
            reads: read.iter().map(TextureSource::id).collect(),
            writes,
            simulation,
        });
    }

    fn visit_simulation(
        &mut self,
        reference: &SimulationRef,
        stage: SimulationStage,
        items: &[StateItem],
    ) {
        let read = if stage == SimulationStage::Start {
            StateRead::Previous
        } else {
            StateRead::Current
        };
        let writes: Vec<String> = items
            .iter()
            .map(|i| state_read_id(&i.texture, read))
            .collect();
        for (item, write) in items.iter().zip(&writes) {
            debug_assert_eq!(*write, state_source(reference, item, read).id());
            self.set_resource(resource_of(&state_source(reference, item, read)));
        }
        self.visit(
            format!("{}:{}", stage.id(), writes.join(",")),
            writes,
            Some(ShaderTarget::Simulation(SimulationTarget {
                simulation: reference.clone(),
                stage,
                items: items.to_vec(),
            })),
            Some(PassSimulation {
                id: reference.id(),
                stage: Some(stage),
            }),
            items.first().map(|i| i.size.clone()),
            items.first().and_then(|i| i.like.as_deref().cloned()),
        );
    }
}

/// The passes drawing the graph: the fragment output's, after those of the
/// values it reads rendered to textures and of the simulations whose state
/// it reads, and of what those read, each once however many read it. A
/// simulation's state is drawn by a start and a step pass per run of items
/// a pass can write.
pub fn generate_render_plan(
    graph: CodegenGraph<'_>,
    options: &ShaderOptions,
) -> Result<RenderPlan, CodegenError> {
    let sources = Rc::new(TextureSources::new(graph));
    let frame = FrameCompiler::new(
        graph,
        sources.clone(),
        FrameCompilerOptions {
            uniform_scopes: options.uniform_scopes.clone(),
            texture_size: true,
        },
    );
    let mut builder = PlanBuilder {
        graph,
        options,
        sources: &sources,
        frame: &frame,
        passes: Vec::new(),
        resources: Vec::new(),
        uniforms: Vec::new(),
        visited: HashSet::new(),
        error: None,
    };
    builder.visit(
        PLAN_TARGET.to_owned(),
        vec![PLAN_TARGET.to_owned()],
        None,
        None,
        None,
        None,
    );
    if let Some(error) = builder.error {
        return Err(error);
    }
    // Frame values may read the size of a texture no pass reads.
    for source in frame.sized_textures() {
        if builder.resources.iter().all(|r| r.id() != source.id()) {
            builder.resources.push(resource_of(&source));
        }
    }
    for entry in frame.uniforms() {
        super::upsert(&mut builder.uniforms, entry, |a, b| a.0 == b.0);
    }
    Ok(RenderPlan {
        resources: builder.resources,
        passes: builder.passes,
        uniforms: builder.uniforms,
        frame: frame.program(),
    })
}
