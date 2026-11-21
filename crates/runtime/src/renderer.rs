use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context as _, Result};
use shader_graph::ShaderProgram;
use shader_graph::codegen::{PLAN_TARGET, PassDecl, RenderPlan, ResourceDecl, SimulationStage};
use shader_graph::passes::{
    SizeBase, SizeEnv, TextureFormat, TextureSize, mip_level_count, resolve_size,
};
use shader_graph::types::{SamplerAddress, SamplerFilter};

use crate::clock::FrameInputs;
use crate::formats::{wgpu_format, wgpu_sample_type};
use crate::gpu_timer::{GpuTimer, TIMING_BYTES};
use crate::mips::MipGenerator;
use crate::present::{PresentOptions, Presenter};
use crate::simulation::{SimulationState, Step, StepKind};
use crate::uniforms::{UniformBlock, pack_block};

pub use shader_graph::passes::Size;

/// The format programs draw into: linear, and wide enough for colours outside sRGB.
const LINEAR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Rows copied out of a texture start on multiples of this.
const ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

#[derive(Clone, Debug, PartialEq)]
pub struct ProgramDiagnostic {
    pub message: String,
    /// 1-based line and column in the program's code, when known.
    pub location: Option<(u32, u32)>,
}

/// A program that failed to compile, and why.
#[derive(Debug, thiserror::Error)]
#[error("{}", .diagnostics.iter().map(|d| d.message.as_str()).collect::<Vec<_>>().join("\n"))]
pub struct ProgramError {
    pub diagnostics: Vec<ProgramDiagnostic>,
    pub code: String,
}

/// One pass of a compiled plan, with its own frame and uniform buffers.
struct CompiledPass {
    decl: PassDecl,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
}

/// A compiled plan: its passes in order, each drawing textures of its own
/// (kept while their size and format stay the same) or of the simulations'
/// state, but the last, which draws the frame.
pub struct Program {
    passes: Vec<CompiledPass>,
    plan: RenderPlan,
}

impl Program {
    /// The program of the pass drawing the frame.
    pub fn source(&self) -> &ShaderProgram {
        self.passes.last().map_or_else(|| &self.plan.passes[0].program, |p| &p.decl.program)
    }

    pub fn plan(&self) -> &RenderPlan {
        &self.plan
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// What GPUI's images are made of.
    Bgra8,
    Rgba8,
}

impl PixelFormat {
    fn texture_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Bgra8 => wgpu::TextureFormat::Bgra8Unorm,
            Self::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        }
    }
}

/// A rendered frame, read back.
pub struct Frame {
    pub size: Size,
    pub format: PixelFormat,
    /// Tightly packed rows, top first.
    pub pixels: Vec<u8>,
    /// Seconds the program's passes took on the GPU, on devices that can time it.
    pub gpu_time: Option<f64>,
}

impl Frame {
    /// The frame as a PNG; straight alpha when rendered with `premultiply` off.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut rgba = self.pixels.clone();
        if self.format == PixelFormat::Bgra8 {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let image = image::RgbaImage::from_raw(self.size.width, self.size.height, rgba)
            .context("the frame's pixels don't match its size")?;
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png)?;
        Ok(out.into_inner())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub size: Size,
    pub format: PixelFormat,
    /// Premultiply the colour by its alpha, as a window composites; exports keep it straight.
    pub premultiply: bool,
}

struct Readback {
    pixels: wgpu::Buffer,
    timing: wgpu::Buffer,
}

/// The sizes of a plan's resources for a canvas, found as asked for: a size
/// may be relative to another resource's.
struct Sizes<'a> {
    plan: &'a RenderPlan,
    canvas: Size,
    max_dimension: u32,
    /// The size of each media resource's image.
    media: HashMap<String, Size>,
    known: HashMap<String, Size>,
    resolving: HashSet<String>,
}

impl Sizes<'_> {
    fn get(&mut self, id: &str) -> Size {
        if let Some(known) = self.known.get(id) {
            return *known;
        }
        let size = match self.plan.resource(id) {
            Some(ResourceDecl::Transient { size, .. } | ResourceDecl::State { size, .. }) => {
                // A size that depends on itself has nothing to start from.
                if !self.resolving.insert(id.to_owned()) {
                    return self.canvas;
                }
                let base = match size {
                    TextureSize::Scale { of: SizeBase::Resource(of), .. } => Some((of.clone(), self.get(of))),
                    _ => None,
                };
                self.resolving.remove(id);
                let env = SizeEnv {
                    canvas: self.canvas,
                    resource: &|of| base.as_ref().filter(|(r, _)| r == of).map(|(_, s)| *s),
                    max_dimension: self.max_dimension,
                };
                resolve_size(size, &env)
            }
            Some(ResourceDecl::Media { id, .. }) => self.media.get(id).copied().unwrap_or(Size::new(1, 1)),
            Some(ResourceDecl::Empty { .. }) | None => Size::new(1, 1),
        };
        self.known.insert(id.to_owned(), size);
        size
    }
}

/// Draws plans of programs with wgpu on a device of its own and reads the
/// frames back, so they can be shown without sharing the UI's GPU context.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    presenter: Presenter,
    mips: MipGenerator,
    timer: Option<GpuTimer>,
    /// The texture read where nothing is: transparent black.
    empty: wgpu::Texture,
    samplers: HashMap<(SamplerFilter, SamplerAddress), wgpu::Sampler>,
    /// The textures passes draw for others to read, by resource id.
    transients: HashMap<String, wgpu::Texture>,
    /// The images project assets give, by asset id.
    media: HashMap<String, wgpu::Texture>,
    simulation: SimulationState,
    linear: Option<wgpu::Texture>,
    output: Option<wgpu::Texture>,
    readback: Option<(Size, Readback)>,
}

fn filter_mode(filter: SamplerFilter) -> wgpu::FilterMode {
    match filter {
        SamplerFilter::Linear => wgpu::FilterMode::Linear,
        SamplerFilter::Nearest => wgpu::FilterMode::Nearest,
    }
}

fn address_mode(address: SamplerAddress) -> wgpu::AddressMode {
    match address {
        SamplerAddress::Clamp => wgpu::AddressMode::ClampToEdge,
        SamplerAddress::Repeat => wgpu::AddressMode::Repeat,
        SamplerAddress::Mirror => wgpu::AddressMode::MirrorRepeat,
    }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .context("no GPU adapter")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                // Sizes are bound by the adapter's limit, not the default.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                    ..Default::default()
                },
                ..Default::default()
            }))?;
        // wgpu's default handler panics on errors outside an error scope.
        device.on_uncaptured_error(std::sync::Arc::new(|error| log::error!("GPU error: {error}")));
        let timer = GpuTimer::new(&device, &queue);
        let empty = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("empty"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LINEAR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Ok(Self {
            presenter: Presenter::new(&device),
            mips: MipGenerator::new(&device),
            device,
            queue,
            timer,
            empty,
            samplers: HashMap::new(),
            transients: HashMap::new(),
            media: HashMap::new(),
            simulation: SimulationState::default(),
            linear: None,
            output: None,
            readback: None,
        })
    }

    /// Compiles a plan's programs; their uniform blocks stay zero until `set_uniforms`.
    pub fn compile(&mut self, plan: &RenderPlan) -> Result<Program, ProgramError> {
        let mut passes = Vec::with_capacity(plan.passes.len());
        for decl in &plan.passes {
            let formats: Vec<wgpu::TextureFormat> = decl
                .writes
                .iter()
                .map(|id| match plan.resource(id) {
                    Some(ResourceDecl::Transient { format, .. } | ResourceDecl::State { format, .. }) => {
                        wgpu_format(*format)
                    }
                    _ => LINEAR_FORMAT,
                })
                .collect();
            passes.push(self.compile_pass(decl, &formats)?);
        }
        if passes.is_empty() {
            return Err(ProgramError {
                diagnostics: vec![ProgramDiagnostic { message: "the plan draws nothing".to_owned(), location: None }],
                code: String::new(),
            });
        }
        Ok(Program { passes, plan: plan.clone() })
    }

    fn compile_pass(
        &self,
        decl: &PassDecl,
        formats: &[wgpu::TextureFormat],
    ) -> Result<CompiledPass, ProgramError> {
        let source = &decl.program;
        let fail = |message: String, location| ProgramError {
            diagnostics: vec![ProgramDiagnostic { message, location }],
            code: source.code.clone(),
        };
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("program"),
            source: wgpu::ShaderSource::Wgsl(source.code.as_str().into()),
        });
        let info = pollster::block_on(module.get_compilation_info());
        let diagnostics: Vec<ProgramDiagnostic> = info
            .messages
            .iter()
            .filter(|m| m.message_type == wgpu::CompilationMessageType::Error)
            .map(|m| ProgramDiagnostic {
                message: m.message.clone(),
                location: m.location.as_ref().map(|l| (l.line_number, l.line_position)),
            })
            .collect();
        if !diagnostics.is_empty() {
            pollster::block_on(scope.pop());
            return Err(ProgramError { diagnostics, code: source.code.clone() });
        }

        let visibility = wgpu::ShaderStages::FRAGMENT;
        let buffer_entry = |block: &UniformBlock| wgpu::BindGroupLayoutEntry {
            binding: block.binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let mut layout_entries = vec![buffer_entry(&source.frame), buffer_entry(&source.uniforms)];
        layout_entries.extend(source.textures.iter().map(|t| wgpu::BindGroupLayoutEntry {
            binding: t.binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu_sample_type(t.sample_type),
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }));
        layout_entries.extend(source.samplers.iter().map(|s| wgpu::BindGroupLayoutEntry {
            binding: s.binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }));
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("program"),
            entries: &layout_entries,
        });
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("program"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let targets: Vec<Option<wgpu::ColorTargetState>> =
            formats.iter().map(|f| Some((*f).into())).collect();
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("program"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(&source.vertex_entry),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(&source.fragment_entry),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let buffer = |label, block: &UniformBlock| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(block.size.max(16)),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_buffer = buffer("frame", &source.frame);
        let uniform_buffer = buffer("uniforms", &source.uniforms);
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(fail(error.to_string(), None));
        }
        Ok(CompiledPass { decl: decl.clone(), pipeline, layout, frame_buffer, uniform_buffer })
    }

    /// Writes the values of a program's uniform blocks, e.g. after an edit that doesn't change its code.
    pub fn set_uniforms(&self, program: &Program, values: &BTreeMap<String, Vec<f64>>) {
        for pass in &program.passes {
            let block = &pass.decl.program.uniforms;
            if block.size == 0 {
                continue;
            }
            let mut bytes = vec![0u8; block.size as usize];
            pack_block(block, values, &mut bytes);
            self.queue.write_buffer(&pass.uniform_buffer, 0, &bytes);
        }
    }

    /// Gives a project asset's image to the programs reading it: straight-alpha
    /// linear RGBA, 8 bits per channel, `size.width * size.height * 4` bytes.
    pub fn set_media(&mut self, asset: &str, size: Size, rgba: &[u8]) -> Result<()> {
        anyhow::ensure!(
            rgba.len() == size.width as usize * size.height as usize * 4 && size.width > 0 && size.height > 0,
            "the image's pixels don't match its size"
        );
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media"),
            size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        self.media.insert(asset.to_owned(), texture);
        Ok(())
    }

    /// Forgets every simulation's state: they all start over on the next frame.
    pub fn reset_simulations(&mut self) {
        self.simulation.reset();
    }

    fn sampler(&mut self, filter: SamplerFilter, address: SamplerAddress) -> wgpu::Sampler {
        self.samplers
            .entry((filter, address))
            .or_insert_with(|| {
                let mode = address_mode(address);
                self.device.create_sampler(&wgpu::SamplerDescriptor {
                    mag_filter: filter_mode(filter),
                    min_filter: filter_mode(filter),
                    mipmap_filter: match filter {
                        SamplerFilter::Linear => wgpu::MipmapFilterMode::Linear,
                        SamplerFilter::Nearest => wgpu::MipmapFilterMode::Nearest,
                    },
                    address_mode_u: mode,
                    address_mode_v: mode,
                    ..Default::default()
                })
            })
            .clone()
    }

    /// A pass's own texture, of `size` and `format`, with all its mip levels if `mips`.
    fn transient(&mut self, id: &str, size: Size, format: TextureFormat, mips: bool) -> wgpu::Texture {
        let levels = if mips { mip_level_count(size) } else { 1 };
        let format = wgpu_format(format);
        if let Some(existing) = self.transients.get(id)
            && existing.width() == size.width
            && existing.height() == size.height
            && existing.format() == format
            && existing.mip_level_count() == levels
        {
            return existing.clone();
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(id),
            size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.transients.insert(id.to_owned(), texture.clone());
        texture
    }

    /// The texture of every resource of `plan` for a frame at `canvas` size,
    /// by id, and how each simulation steps.
    fn resolve_resources(
        &mut self,
        plan: &RenderPlan,
        inputs: FrameInputs,
        canvas: Size,
    ) -> (HashMap<String, wgpu::Texture>, HashMap<String, Step>) {
        let media = plan
            .resources
            .iter()
            .filter_map(|r| match r {
                ResourceDecl::Media { id, asset } => {
                    let texture = self.media.get(asset)?;
                    Some((id.clone(), Size::new(texture.width(), texture.height())))
                }
                _ => None,
            })
            .collect();
        let mut sizes = Sizes {
            plan,
            canvas,
            max_dimension: self.device.limits().max_texture_dimension_2d,
            media,
            known: HashMap::new(),
            resolving: HashSet::new(),
        };
        // State is sized like any other texture, so it's kept once sizes can be resolved.
        let steps = self.simulation.advance(&self.device, plan, inputs, &mut |id| sizes.get(id));
        let mut textures = HashMap::new();
        for resource in &plan.resources {
            let texture = match resource {
                ResourceDecl::Transient { id, format, mips, .. } => {
                    self.transient(id, sizes.get(id), *format, *mips)
                }
                ResourceDecl::State { texture, read, .. } => {
                    self.simulation.texture(texture, *read).unwrap_or(&self.empty).clone()
                }
                ResourceDecl::Media { asset, .. } => self.media.get(asset).unwrap_or(&self.empty).clone(),
                ResourceDecl::Empty { .. } => self.empty.clone(),
            };
            textures.insert(resource.id().to_owned(), texture);
        }
        self.transients.retain(|id, _| textures.contains_key(id));
        (textures, steps)
    }

    /// Encodes the passes drawing one frame into `target`.
    fn encode_plan(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        inputs: FrameInputs,
        target: &wgpu::Texture,
    ) {
        let canvas = Size::new(target.width(), target.height());
        let (textures, steps) = self.resolve_resources(&program.plan, inputs, canvas);
        // A simulation's start only draws on the frames it starts.
        let drawn: Vec<&CompiledPass> = program
            .passes
            .iter()
            .filter(|pass| match &pass.decl.simulation {
                Some(sim) if sim.stage == Some(SimulationStage::Start) => {
                    steps.get(&sim.id).is_some_and(|s| s.kind == StepKind::Start)
                }
                _ => true,
            })
            .collect();
        let count = drawn.len();
        for (i, pass) in drawn.into_iter().enumerate() {
            let decl = &pass.decl;
            let targets: Vec<wgpu::Texture> = decl
                .writes
                .iter()
                .map(|id| if id == PLAN_TARGET { target.clone() } else { textures.get(id).cloned().unwrap_or_else(|| self.empty.clone()) })
                .collect();
            let first = targets.first();
            let delta = decl
                .simulation
                .as_ref()
                .and_then(|s| steps.get(&s.id))
                .map_or(0.0, |s| s.delta);
            self.write_frame_block(pass, inputs, first, delta);
            let views: Vec<wgpu::TextureView> = targets
                .iter()
                .map(|t| {
                    // One level at a time: the rest are made after the pass.
                    t.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: 0,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            let program_source = &decl.program;
            let sampler_list: Vec<wgpu::Sampler> =
                program_source.samplers.iter().map(|s| self.sampler(s.filter, s.address)).collect();
            let read_views: Vec<wgpu::TextureView> = program_source
                .textures
                .iter()
                .map(|t| {
                    let id = t.source.id();
                    textures.get(&id).unwrap_or(&self.empty).create_view(&Default::default())
                })
                .collect();
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: program_source.frame.binding,
                    resource: pass.frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: program_source.uniforms.binding,
                    resource: pass.uniform_buffer.as_entire_binding(),
                },
            ];
            entries.extend(program_source.textures.iter().zip(&read_views).map(|(t, view)| {
                wgpu::BindGroupEntry {
                    binding: t.binding,
                    resource: wgpu::BindingResource::TextureView(view),
                }
            }));
            entries.extend(program_source.samplers.iter().zip(&sampler_list).map(|(s, sampler)| {
                wgpu::BindGroupEntry {
                    binding: s.binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                }
            }));
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("program"),
                layout: &pass.layout,
                entries: &entries,
            });
            let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = views
                .iter()
                .map(|view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect();
            {
                let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("program"),
                    color_attachments: &attachments,
                    timestamp_writes: self
                        .timer
                        .as_ref()
                        .filter(|_| i == 0 || i + 1 == count)
                        .map(|t| t.writes(i == 0, i + 1 == count)),
                    ..Default::default()
                });
                render.set_pipeline(&pass.pipeline);
                render.set_bind_group(0, &bind_group, &[]);
                render.draw(0..3, 0..1);
            }
            for texture in &targets {
                if texture.mip_level_count() > 1 {
                    self.mips.generate(&self.device, encoder, texture);
                }
            }
        }
    }

    fn write_frame_block(
        &self,
        pass: &CompiledPass,
        inputs: FrameInputs,
        first: Option<&wgpu::Texture>,
        simulation_delta: f64,
    ) {
        let block = &pass.decl.program.frame;
        if block.size == 0 {
            return;
        }
        let mut bytes = vec![0u8; block.size as usize];
        let (width, height) = first.map_or((0, 0), |t| (t.width(), t.height()));
        let values = BTreeMap::from([
            ("time".to_string(), vec![inputs.time]),
            ("delta".to_string(), vec![inputs.delta]),
            ("frame".to_string(), vec![inputs.frame as f64]),
            ("resolution".to_string(), vec![f64::from(width), f64::from(height)]),
            ("simulation_delta".to_string(), vec![simulation_delta]),
        ]);
        pack_block(block, &values, &mut bytes);
        self.queue.write_buffer(&pass.frame_buffer, 0, &bytes);
    }

    /// Draws `frames` at `size` without reading them back, only to step the
    /// simulations: for the frames before an export's first.
    pub fn simulate(
        &mut self,
        program: &Program,
        frames: impl IntoIterator<Item = FrameInputs>,
        size: Size,
    ) -> Result<()> {
        anyhow::ensure!(size.width > 0 && size.height > 0, "the canvas has no size");
        self.ensure_targets(size, PixelFormat::Rgba8);
        let linear = self.linear.clone().context("no target")?;
        for inputs in frames {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.encode_plan(&mut encoder, program, inputs, &linear);
            self.queue.submit([encoder.finish()]);
        }
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    /// Draws one frame of `program` and reads it back.
    pub fn render(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        options: RenderOptions,
    ) -> Result<Frame> {
        let size = options.size;
        anyhow::ensure!(size.width > 0 && size.height > 0, "the canvas has no size");
        self.ensure_targets(size, options.format);

        let (linear, output) = (
            self.linear.clone().context("no target")?,
            self.output.clone().context("no target")?,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_plan(&mut encoder, program, inputs, &linear);
        self.presenter.encode(
            &self.device,
            &mut encoder,
            &linear,
            &output.create_view(&Default::default()),
            PresentOptions {
                format: options.format.texture_format(),
                premultiply: options.premultiply,
            },
        );

        let padded_row = (size.width * 4).next_multiple_of(ROW_ALIGNMENT);
        let (_, readback) = self.readback.as_ref().context("no readback")?;
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback.pixels,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: None,
                },
            },
            output.size(),
        );
        if let Some(timer) = &self.timer {
            timer.resolve(&mut encoder, &readback.timing, 0);
        }
        self.queue.submit([encoder.finish()]);

        let pixel_slice = readback.pixels.slice(..);
        pixel_slice.map_async(wgpu::MapMode::Read, |_| {});
        let timing_slice = readback.timing.slice(..);
        if self.timer.is_some() {
            timing_slice.map_async(wgpu::MapMode::Read, |_| {});
        }
        self.device.poll(wgpu::PollType::wait_indefinitely())?;

        let mapped = pixel_slice.get_mapped_range()?;
        let mut pixels = Vec::with_capacity((size.width * size.height * 4) as usize);
        for row in mapped.chunks(padded_row as usize).take(size.height as usize) {
            pixels.extend_from_slice(&row[..(size.width * 4) as usize]);
        }
        drop(mapped);
        readback.pixels.unmap();
        let gpu_time = self.timer.as_ref().and_then(|timer| {
            let seconds = timer.seconds(&timing_slice.get_mapped_range().ok()?);
            readback.timing.unmap();
            seconds
        });
        Ok(Frame { size, format: options.format, pixels, gpu_time })
    }

    /// Textures and buffers for frames of `size`, kept while it stays the same.
    fn ensure_targets(&mut self, size: Size, format: PixelFormat) {
        let extent = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let fits = |t: &wgpu::Texture, format| {
            t.width() == size.width && t.height() == size.height && t.format() == format
        };
        let linear_ok = self.linear.as_ref().is_some_and(|t| fits(t, LINEAR_FORMAT));
        if !linear_ok {
            self.linear = Some(texture(
                "linear",
                LINEAR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ));
        }
        let output_format = format.texture_format();
        if !self.output.as_ref().is_some_and(|t| fits(t, output_format)) {
            self.output = Some(texture(
                "output",
                output_format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            ));
        }
        if self.readback.as_ref().map(|(s, _)| *s) != Some(size) {
            let padded_row = u64::from((size.width * 4).next_multiple_of(ROW_ALIGNMENT));
            let buffer = |label, size| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            };
            self.readback = Some((
                size,
                Readback {
                    pixels: buffer("frame readback", padded_row * u64::from(size.height)),
                    timing: buffer("timing readback", TIMING_BYTES),
                },
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uniforms::{UniformField, UniformScalar};
    use shader_graph::codegen::{PassDecl, RenderPlan};

    const CODE: &str = r"
struct Frame { time: f32, delta: f32, resolution: vec2f, frame: u32 }
@group(0) @binding(0) var<uniform> frame: Frame;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32(index & 1u) * 4.0 - 1.0, f32(index >> 1u) * 4.0 - 1.0);
    return vec4f(corner, 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    return vec4f(position.x / frame.resolution.x, position.y / frame.resolution.y, frame.time, 1.0);
}
";

    fn plan(code: &str) -> RenderPlan {
        let field = |offset, components| UniformField {
            offset,
            scalar: UniformScalar::F32,
            components,
        };
        let program = ShaderProgram {
            code: code.into(),
            vertex_entry: "vertex".into(),
            fragment_entry: "fragment".into(),
            frame: UniformBlock {
                binding: 0,
                size: 32,
                fields: BTreeMap::from([
                    ("time".into(), field(0, 1)),
                    ("delta".into(), field(4, 1)),
                    ("resolution".into(), field(8, 2)),
                    (
                        "frame".into(),
                        UniformField { offset: 16, scalar: UniformScalar::U32, components: 1 },
                    ),
                ]),
            },
            uniforms: UniformBlock { binding: 1, size: 0, fields: BTreeMap::new() },
            textures: Vec::new(),
            samplers: Vec::new(),
            targets: None,
        };
        RenderPlan {
            resources: Vec::new(),
            passes: vec![PassDecl {
                id: "pass:target".into(),
                program,
                line_map: BTreeMap::new(),
                reads: Vec::new(),
                writes: vec![PLAN_TARGET.to_owned()],
                simulation: None,
            }],
            uniforms: Vec::new(),
        }
    }

    #[test]
    fn draws_a_frame_with_its_inputs() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let options = RenderOptions {
            size: Size::new(100, 50),
            format: PixelFormat::Rgba8,
            premultiply: false,
        };
        let inputs = FrameInputs { time: 1.0, delta: 0.0, frame: 0 };
        let frame = renderer.render(&program, inputs, options).unwrap();
        assert_eq!(frame.pixels.len(), 100 * 50 * 4);
        let at = |x: usize, y: usize| &frame.pixels[(y * 100 + x) * 4..][..4];
        assert!(at(0, 0)[0] < 30 && at(0, 0)[1] < 40);
        assert!(at(99, 0)[0] > 245, "red follows x");
        assert!(at(0, 49)[1] > 240, "green follows y, downwards");
        assert!(at(10, 10)[2] > 245, "blue is the time, at 1");
        assert!(frame.to_png().unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn reports_where_a_program_fails() {
        let mut renderer = Renderer::new().unwrap();
        let error = renderer
            .compile(&plan(&CODE.replace("frame.time", "frame.nothing")))
            .err()
            .unwrap();
        assert!(!error.diagnostics.is_empty());
        assert!(error.diagnostics[0].location.is_some_and(|(line, _)| line > 0));
    }
}
