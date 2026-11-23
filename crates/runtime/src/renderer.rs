use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use anyhow::{Context as _, Result};
use shader_graph::codegen::{
    PLAN_TARGET, PassDecl, RenderPlan, ResourceDecl, SimulationStage, UniformValue,
};
use shader_graph::frame::{FrameEnv, FrameRef, FrameValues, evaluate_frame};
use shader_graph::passes::{SizeEnv, mip_level_count, resolve_size};
use shader_graph::types::{SamplerAddress, SamplerFilter};
use shader_graph::{ShaderProgram, UniformBlock};

use crate::clock::FrameInputs;
use crate::formats::{wgpu_format, wgpu_sample_type};
use crate::gpu_timer::{GpuTimer, TIMING_BYTES};
use crate::media::{MediaLibrary, MediaSource, MediaStatus};
use crate::mips::MipGenerator;
use crate::present::{PresentOptions, Presenter};
use crate::simulation::{Step, StepKind};
#[cfg(target_os = "macos")]
use crate::surface::{Surface, SurfacePool};
use crate::target::{Target, render_target};
use crate::uniforms::pack_block;
use std::sync::atomic::{AtomicU64, Ordering};

pub use shader_graph::passes::Size;

/// The format programs draw into: linear, and wide enough for colours outside sRGB.
pub(crate) const LINEAR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
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

/// One pass of a compiled plan, with its own uniform buffer. The frame blocks, which depend on the
/// canvas a frame is drawn at, belong to each target.
pub(crate) struct CompiledPass {
    pub(crate) decl: PassDecl,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

static NEXT_PROGRAM: AtomicU64 = AtomicU64::new(1);

/// A compiled plan: its passes in order, each drawing textures of its own
/// (kept while their size and format stay the same) or of the simulations'
/// state, but the last, which draws the frame.
pub struct Program {
    /// Tells programs apart, for what targets keep per program.
    pub(crate) id: u64,
    pub(crate) passes: Vec<CompiledPass>,
    plan: RenderPlan,
    /// The values of the uniform inputs, which the frame values read. Shared so that each frame
    /// drawn can read them without copying the list.
    uniforms: std::sync::Mutex<Arc<Vec<(String, UniformValue)>>>,
}

impl Program {
    /// The program of the pass drawing the frame.
    pub fn source(&self) -> Option<&ShaderProgram> {
        self.passes.last().map(|p| &p.decl.program)
    }

    pub fn plan(&self) -> &RenderPlan {
        &self.plan
    }

    /// Tells compiled programs apart.
    pub fn id(&self) -> u64 {
        self.id
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

/// A frame as the renderer hands it out: its pixels, or the surface it was drawn into.
pub enum Presented {
    Pixels(Frame),
    /// One surface per distinct canvas size asked for.
    #[cfg(target_os = "macos")]
    Surfaces {
        surfaces: Vec<(Size, Surface)>,
        gpu_time: Option<f64>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub size: Size,
    pub format: PixelFormat,
    /// Premultiply the colour by its alpha, as a window composites; exports keep it straight.
    pub premultiply: bool,
}

/// A GPU step's completion, reported once it has been seen; `None` after that.
type Ready = Option<Receiver<anyhow::Result<()>>>;

/// Where a submitted frame's output goes: copied to a staging buffer, or drawn into a surface.
enum Output {
    Pixels {
        size: Size,
        format: PixelFormat,
        buffer: wgpu::Buffer,
    },
    #[cfg(target_os = "macos")]
    Surfaces(Vec<(Size, Surface)>),
}

/// A frame's pass timestamps, read once mapped.
struct Timing {
    buffer: wgpu::Buffer,
    ready: Ready,
}

/// A frame whose GPU work is on its way to the CPU or the window.
struct Submitted {
    output: Output,
    /// Done when the output can be read: its copy is mapped, or the GPU has finished drawing it.
    ready: Ready,
    timing: Option<Timing>,
    submission: wgpu::SubmissionIndex,
}

impl Submitted {
    /// Whether the output and its timing are ready, without waiting for them.
    fn poll(&mut self) -> Result<bool> {
        let output = poll_ready(&mut self.ready)?;
        let timing = match &mut self.timing {
            Some(timing) => poll_ready(&mut timing.ready)?,
            None => true,
        };
        Ok(output && timing)
    }
}

/// Frames in flight at once: one being read out while the next draws.
const READBACK_DEPTH: usize = 2;

/// Maps a staging buffer for reading, once the copy into it has run.
fn map_for_read(buffer: &wgpu::Buffer) -> Ready {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            // The receiver is gone only when the frame was dropped unread.
            let _ = sender.send(result.map_err(anyhow::Error::from));
        });
    Some(receiver)
}

/// Reports once the GPU has finished the work submitted so far.
fn gpu_done(queue: &wgpu::Queue) -> Ready {
    let (sender, receiver) = std::sync::mpsc::channel();
    queue.on_submitted_work_done(move || {
        let _ = sender.send(Ok(()));
    });
    Some(receiver)
}

/// Whether a step has finished, without waiting; a failed step is an error.
fn poll_ready(ready: &mut Ready) -> Result<bool> {
    let Some(receiver) = ready else {
        return Ok(true);
    };
    match receiver.try_recv() {
        Ok(result) => {
            result?;
            *ready = None;
            Ok(true)
        }
        Err(TryRecvError::Empty) => Ok(false),
        Err(TryRecvError::Disconnected) => anyhow::bail!("a frame was dropped before it finished"),
    }
}

/// Bytes per row of a readback `width` pixels wide, padded as the copy requires.
fn padded_row_bytes(width: u32) -> Result<u32> {
    width
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_next_multiple_of(ROW_ALIGNMENT))
        .context("the canvas is too wide to read back")
}

/// What a frame is drawn with: its inputs, and the canvas' size.
#[derive(Clone, Copy)]
struct FrameContext<'a> {
    inputs: FrameInputs,
    canvas: Size,
    uniforms: &'a [(String, UniformValue)],
}

/// The sizes of a plan's resources for a canvas, found as asked for: a size
/// may be relative to another resource's, or made of frame values, which may
/// read the size of a texture an earlier pass made.
struct Sizes<'a> {
    plan: &'a RenderPlan,
    context: FrameContext<'a>,
    max_dimension: u32,
    /// The size of each media resource's image.
    media: HashMap<String, Size>,
    known: RefCell<HashMap<String, Size>>,
    resolving: RefCell<HashSet<String>>,
}

impl Sizes<'_> {
    fn canvas(&self) -> Size {
        self.context.canvas
    }

    /// The plan's frame values, for a pass of `pass_size`.
    fn frame_values(&self, pass_size: Size) -> FrameValues {
        let texture_size = |id: &str| {
            let size = self.get_known(id)?;
            let levels = match self.plan.resource(id) {
                Some(ResourceDecl::Transient { mips: true, .. }) => mip_level_count(size),
                _ => 1,
            };
            Some((size, Some(levels)))
        };
        let FrameInputs { time, delta, frame } = self.context.inputs;
        evaluate_frame(
            &self.plan.frame,
            &FrameEnv {
                time,
                delta,
                frame: frame as f64,
                canvas: self.canvas(),
                pass_size,
                iteration: None,
                texture_size: Some(&texture_size),
                uniforms: self.context.uniforms,
            },
        )
    }

    /// A resource's size, when it has one.
    fn get_known(&self, id: &str) -> Option<Size> {
        self.plan.resource(id).map(|_| self.get(id))
    }

    fn get(&self, id: &str) -> Size {
        if let Some(known) = self.known.borrow().get(id) {
            return *known;
        }
        let canvas = self.canvas();
        let size = match self.plan.resource(id) {
            Some(ResourceDecl::Transient { size, .. } | ResourceDecl::State { size, .. }) => {
                // A size that depends on itself has nothing to start from.
                if !self.resolving.borrow_mut().insert(id.to_owned()) {
                    return canvas;
                }
                let values = OnceCell::new();
                let frame = |reference: &FrameRef| {
                    values
                        .get_or_init(|| self.frame_values(canvas))
                        .value(reference)
                        .first()
                        .copied()
                        .unwrap_or(0.0)
                };
                let resource = |of: &str| Some(self.get(of));
                let resolved = resolve_size(
                    size,
                    &SizeEnv {
                        canvas,
                        resource: &resource,
                        max_dimension: self.max_dimension,
                        frame: Some(&frame),
                    },
                );
                self.resolving.borrow_mut().remove(id);
                resolved
            }
            Some(ResourceDecl::Media { id: media_id, .. }) => {
                self.media.get(media_id).copied().unwrap_or(Size::new(1, 1))
            }
            Some(ResourceDecl::Empty { .. }) | None => Size::new(1, 1),
        };
        self.known.borrow_mut().insert(id.to_owned(), size);
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
    /// What drawing at each canvas size keeps. Frames of the same size share a target.
    targets: HashMap<Size, Target>,
    media: MediaLibrary,
    output: Option<wgpu::Texture>,
    /// Staging buffers no frame is reading, reused by the next frame of their size.
    free_pixels: Vec<(Size, wgpu::Buffer)>,
    free_timing: Vec<wgpu::Buffer>,
    in_flight: VecDeque<Submitted>,
    /// Uniform block bytes are packed here before upload, so drawing a frame doesn't allocate them.
    scratch: Vec<u8>,
}

/// Compiles plans on a renderer's device. The programs it makes are drawn by that renderer.
#[derive(Clone)]
pub struct Compiler {
    device: wgpu::Device,
}

impl Compiler {
    /// Compiles a plan's programs; their uniform blocks stay zero until `Renderer::set_uniforms`.
    pub fn compile(&self, plan: &RenderPlan) -> Result<Program, ProgramError> {
        let mut passes = Vec::with_capacity(plan.passes.len());
        for decl in &plan.passes {
            let formats: Vec<wgpu::TextureFormat> = decl
                .writes
                .iter()
                .map(|id| match plan.resource(id) {
                    Some(
                        ResourceDecl::Transient { format, .. } | ResourceDecl::State { format, .. },
                    ) => wgpu_format(*format),
                    _ => LINEAR_FORMAT,
                })
                .collect();
            passes.push(self.compile_pass(decl, &formats)?);
        }
        if passes.is_empty() {
            return Err(ProgramError {
                diagnostics: vec![ProgramDiagnostic {
                    message: "the plan draws nothing".to_owned(),
                    location: None,
                }],
                code: String::new(),
            });
        }
        Ok(Program {
            id: NEXT_PROGRAM.fetch_add(1, Ordering::Relaxed),
            passes,
            plan: plan.clone(),
            uniforms: std::sync::Mutex::new(Arc::new(plan.uniforms.clone())),
        })
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
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
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
                location: m
                    .location
                    .as_ref()
                    .map(|l| (l.line_number, l.line_position)),
            })
            .collect();
        if !diagnostics.is_empty() {
            pollster::block_on(scope.pop());
            return Err(ProgramError {
                diagnostics,
                code: source.code.clone(),
            });
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
        let mut layout_entries = vec![
            buffer_entry(&source.frame),
            buffer_entry(&source.uniforms),
            buffer_entry(&source.frame_values),
        ];
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
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("program"),
                entries: &layout_entries,
            });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("program"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let targets: Vec<Option<wgpu::ColorTargetState>> =
            formats.iter().map(|f| Some((*f).into())).collect();
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let uniform_buffer = buffer("uniforms", &source.uniforms);
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(fail(error.to_string(), None));
        }
        Ok(CompiledPass {
            decl: decl.clone(),
            pipeline,
            layout,
            uniform_buffer,
        })
    }
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
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            log::error!("GPU error: {error}")
        }));
        let timer = GpuTimer::new(&device, &queue);
        let empty = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("empty"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LINEAR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let media = MediaLibrary::new(&device, &queue);
        Ok(Self {
            presenter: Presenter::new(&device),
            mips: MipGenerator::new(&device),
            device,
            queue,
            timer,
            empty,
            samplers: HashMap::new(),
            targets: HashMap::new(),
            media,
            output: None,
            free_pixels: Vec::new(),
            free_timing: Vec::new(),
            in_flight: VecDeque::new(),
            scratch: Vec::new(),
        })
    }

    /// Compiles a plan's programs; their uniform blocks stay zero until `set_uniforms`.
    pub fn compile(&self, plan: &RenderPlan) -> Result<Program, ProgramError> {
        self.compiler().compile(plan)
    }

    /// A handle that compiles on the renderer's device from any thread.
    pub fn compiler(&self) -> Compiler {
        Compiler {
            device: self.device.clone(),
        }
    }

    /// Writes the values of a program's uniform blocks, e.g. after an edit that doesn't change its code.
    pub fn set_uniforms(&self, program: &Program, values: &BTreeMap<String, Vec<f64>>) {
        {
            let mut held = program
                .uniforms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let list = Arc::make_mut(&mut held);
            for (name, value) in values {
                let value = UniformValue(value.iter().map(|c| *c as f32).collect());
                match list.iter_mut().find(|(n, _)| n == name) {
                    Some((_, existing)) => *existing = value,
                    None => list.push((name.clone(), value)),
                }
            }
        }
        for pass in &program.passes {
            let block = &pass.decl.program.uniforms;
            if block.size == 0 {
                continue;
            }
            let mut bytes = vec![0u8; block.size as usize];
            pack_block(
                block,
                |name| values.get(name).map(|v| Cow::Borrowed(v.as_slice())),
                &mut bytes,
            );
            self.queue.write_buffer(&pass.uniform_buffer, 0, &bytes);
        }
    }

    /// Makes the images the programs read the project's assets' images, see
    /// `MediaLibrary::sync`.
    pub fn sync_media(&mut self, sources: &BTreeMap<String, MediaSource>) {
        self.media.sync(sources);
    }

    /// Uploads the images decoded since the last call. Returns whether any changed.
    pub fn poll_media(&mut self) -> bool {
        self.media.poll()
    }

    /// Called from the decoding threads when an image finishes, so the owner polls.
    pub fn set_media_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        self.media.set_waker(waker);
    }

    pub fn media_status(&self, asset: &str) -> MediaStatus {
        self.media.status(asset)
    }

    /// How many canvas sizes frames are kept for.
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    fn take_target(&mut self, size: Size) -> Target {
        self.targets
            .remove(&size)
            .unwrap_or_else(|| Target::new(&self.device, size))
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

    /// The texture of every resource of the plan `sizes` is of, by id, and
    /// how each simulation steps.
    fn resolve_resources(
        &mut self,
        sizes: &Sizes<'_>,
        inputs: FrameInputs,
        target: &mut Target,
    ) -> (HashMap<String, wgpu::Texture>, HashMap<String, Step>) {
        let plan = sizes.plan;
        // State is sized like any other texture, so it's kept once sizes can be resolved.
        let steps = target
            .simulation
            .advance(&self.device, plan, inputs, &mut |id| sizes.get(id));
        let mut textures = HashMap::new();
        let mut frame_values = None;
        for resource in &plan.resources {
            let texture = match resource {
                ResourceDecl::Transient {
                    id, format, mips, ..
                } => target.transient(&self.device, id, sizes.get(id), *format, *mips),
                ResourceDecl::State { texture, read, .. } => target
                    .simulation
                    .texture(texture, *read)
                    .unwrap_or(&self.empty)
                    .clone(),
                ResourceDecl::Media {
                    id,
                    asset,
                    video,
                    time,
                } => match video {
                    Some(playback) => {
                        let values =
                            frame_values.get_or_insert_with(|| sizes.frame_values(sizes.canvas()));
                        let at = time.as_ref().map_or(inputs.time, |arg| values.arg(arg));
                        self.media
                            .video_texture(asset, id, playback, at)
                            .unwrap_or_else(|| self.empty.clone())
                    }
                    None => self.media.texture(asset).unwrap_or(&self.empty).clone(),
                },
                ResourceDecl::Empty { .. } => self.empty.clone(),
            };
            textures.insert(resource.id().to_owned(), texture);
        }
        target.transients.retain(|id, _| textures.contains_key(id));
        (textures, steps)
    }

    /// Encodes the passes drawing one frame into `target`'s linear texture. `timed` says whether the
    /// plan holds the first and the last pass the frame's timing spans, which several plans share.
    fn encode_plan(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        inputs: FrameInputs,
        target: &mut Target,
        timed: (bool, bool),
    ) {
        let canvas = target.size;
        target.ensure_blocks(&self.device, program);
        let uniforms = program
            .uniforms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let media = program
            .plan
            .resources
            .iter()
            .filter_map(|r| match r {
                ResourceDecl::Media { id, asset, .. } => match self.media.status(asset) {
                    MediaStatus::Ready(size) => Some((id.clone(), size)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let sizes = Sizes {
            plan: &program.plan,
            context: FrameContext {
                inputs,
                canvas,
                uniforms: uniforms.as_slice(),
            },
            max_dimension: self.device.limits().max_texture_dimension_2d,
            media,
            known: RefCell::default(),
            resolving: RefCell::default(),
        };
        let (textures, steps) = self.resolve_resources(&sizes, inputs, target);
        // A simulation's start only draws on the frames it starts.
        let drawn: Vec<(usize, &CompiledPass)> = program
            .passes
            .iter()
            .enumerate()
            .filter(|(_, pass)| match &pass.decl.simulation {
                Some(sim) if sim.stage == Some(SimulationStage::Start) => steps
                    .get(&sim.id)
                    .is_some_and(|s| s.kind == StepKind::Start),
                _ => true,
            })
            .collect();
        let count = drawn.len();
        for (i, (index, pass)) in drawn.into_iter().enumerate() {
            let decl = &pass.decl;
            let targets: Vec<wgpu::Texture> = decl
                .writes
                .iter()
                .map(|id| {
                    if id == PLAN_TARGET {
                        target.linear.clone()
                    } else {
                        textures
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| self.empty.clone())
                    }
                })
                .collect();
            let first = targets.first();
            let delta = decl
                .simulation
                .as_ref()
                .and_then(|s| steps.get(&s.id))
                .map_or(0.0, |s| s.delta);
            if let Some(blocks) = target.pass_blocks(index) {
                self.write_frame_block(pass, &blocks.frame, inputs, first, delta);
                self.write_frame_values(pass, &blocks.frame_values, &sizes, first);
            }
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
            let sampler_list: Vec<wgpu::Sampler> = program_source
                .samplers
                .iter()
                .map(|s| self.sampler(s.filter, s.address))
                .collect();
            let read_views: Vec<wgpu::TextureView> = program_source
                .textures
                .iter()
                .map(|t| {
                    let id = t.source.id();
                    textures
                        .get(&id)
                        .unwrap_or(&self.empty)
                        .create_view(&Default::default())
                })
                .collect();
            let Some(blocks) = target.pass_blocks(index) else {
                continue;
            };
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: program_source.frame.binding,
                    resource: blocks.frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: program_source.uniforms.binding,
                    resource: pass.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: program_source.frame_values.binding,
                    resource: blocks.frame_values.as_entire_binding(),
                },
            ];
            entries.extend(
                program_source
                    .textures
                    .iter()
                    .zip(&read_views)
                    .map(|(t, view)| wgpu::BindGroupEntry {
                        binding: t.binding,
                        resource: wgpu::BindingResource::TextureView(view),
                    }),
            );
            entries.extend(program_source.samplers.iter().zip(&sampler_list).map(
                |(s, sampler)| wgpu::BindGroupEntry {
                    binding: s.binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ));
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
                        .filter(|_| (i == 0 && timed.0) || (i + 1 == count && timed.1))
                        .map(|t| t.writes(i == 0 && timed.0, i + 1 == count && timed.1)),
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

    /// Writes the frame values a pass's program reads, computed for its size.
    fn write_frame_values(
        &mut self,
        pass: &CompiledPass,
        buffer: &wgpu::Buffer,
        sizes: &Sizes<'_>,
        first: Option<&wgpu::Texture>,
    ) {
        let program = &pass.decl.program;
        if program.frame_value_refs.is_empty() {
            return;
        }
        let (width, height) = first.map_or((0, 0), |t| (t.width(), t.height()));
        let computed = sizes.frame_values(Size::new(width, height));
        let refs = &program.frame_value_refs;
        self.scratch.clear();
        self.scratch.resize(program.frame_values.size as usize, 0);
        pack_block(
            &program.frame_values,
            |name| {
                refs.get(name)
                    .map(|reference| Cow::Owned(computed.value(reference)))
            },
            &mut self.scratch,
        );
        self.queue.write_buffer(buffer, 0, &self.scratch);
    }

    fn write_frame_block(
        &mut self,
        pass: &CompiledPass,
        buffer: &wgpu::Buffer,
        inputs: FrameInputs,
        first: Option<&wgpu::Texture>,
        simulation_delta: f64,
    ) {
        let block = &pass.decl.program.frame;
        if block.size == 0 {
            return;
        }
        let (width, height) = first.map_or((0, 0), |t| (t.width(), t.height()));
        let frame = inputs.frame as f64;
        let resolution = [f64::from(width), f64::from(height)];
        let lookup = |name: &str| {
            let value: &[f64] = match name {
                "time" => std::slice::from_ref(&inputs.time),
                "delta" => std::slice::from_ref(&inputs.delta),
                "frame" => std::slice::from_ref(&frame),
                "resolution" => &resolution,
                "simulation_delta" => std::slice::from_ref(&simulation_delta),
                _ => return None,
            };
            Some(Cow::Borrowed(value))
        };
        self.scratch.clear();
        self.scratch.resize(block.size as usize, 0);
        pack_block(block, lookup, &mut self.scratch);
        self.queue.write_buffer(buffer, 0, &self.scratch);
    }

    /// Draws `frames` at `size` without reading them back, only to step the
    /// simulations: for the frames before an export's first.
    pub fn simulate(
        &mut self,
        program: &Program,
        frames: impl IntoIterator<Item = FrameInputs>,
        size: Size,
    ) -> Result<()> {
        self.check_canvas(size)?;
        let mut target = self.take_target(size);
        for inputs in frames {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            self.encode_plan(&mut encoder, program, inputs, &mut target, (true, true));
            self.queue.submit([encoder.finish()]);
        }
        self.targets.insert(size, target);
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    /// Draws one frame of `program` and reads it back, waiting for the GPU.
    pub fn render(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        options: RenderOptions,
    ) -> Result<Frame> {
        let mut submitted = self.start_pixels(program, inputs, options)?;
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        anyhow::ensure!(submitted.poll()?, "the frame didn't finish");
        match self.read_out(submitted)? {
            Presented::Pixels(frame) => Ok(frame),
            #[cfg(target_os = "macos")]
            Presented::Surfaces { .. } => anyhow::bail!("a readback came out as a surface"),
        }
    }

    /// Starts drawing a frame and copying its pixels to the CPU, without waiting for the GPU.
    /// `take_frame` returns it once the copy has finished.
    pub fn submit(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        options: RenderOptions,
    ) -> Result<()> {
        self.ensure_room()?;
        let submitted = self.start_pixels(program, inputs, options)?;
        self.in_flight.push_back(submitted);
        Ok(())
    }

    /// Starts drawing a frame into surfaces a window can show, one per distinct size of `sizes`
    /// and without copying anything to the CPU. Every size is drawn with the same inputs, in one
    /// GPU submit; each keeps its own simulation state.
    #[cfg(target_os = "macos")]
    pub fn submit_surfaces(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        sizes: &[Size],
    ) -> Result<()> {
        self.ensure_room()?;
        let submitted = self.start_surfaces(program, inputs, sizes)?;
        self.in_flight.push_back(submitted);
        Ok(())
    }

    /// The oldest submitted frame, once its GPU work has finished; never waits.
    /// A frame that failed is returned as an error, and is consumed either way.
    pub fn take_frame(&mut self) -> Option<Result<Presented>> {
        if self.in_flight.is_empty() {
            return None;
        }
        // Completion callbacks run while the device is polled; this does not wait for the GPU.
        if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
            self.in_flight.clear();
            return Some(Err(error.into()));
        }
        let front = self.in_flight.front_mut()?;
        match front.poll() {
            Ok(false) => None,
            Ok(true) => {
                let submitted = self.in_flight.pop_front()?;
                Some(self.read_out(submitted))
            }
            Err(error) => {
                self.in_flight.pop_front();
                Some(Err(error))
            }
        }
    }

    /// The oldest submitted frame, waiting for its GPU work (not for frames submitted after it).
    pub fn wait_frame(&mut self) -> Option<Result<Presented>> {
        let submission = self.in_flight.front()?.submission.clone();
        let wait = wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        };
        if let Err(error) = self.device.poll(wait) {
            self.in_flight.clear();
            return Some(Err(error.into()));
        }
        self.take_frame()
    }

    fn ensure_room(&self) -> Result<()> {
        anyhow::ensure!(
            self.in_flight.len() < READBACK_DEPTH,
            "frames are already waiting to be read back"
        );
        Ok(())
    }

    /// Rejects a canvas no texture or readback can be made for, before the sizes derived from it are computed.
    fn check_canvas(&self, size: Size) -> Result<()> {
        let max = self.device.limits().max_texture_dimension_2d;
        anyhow::ensure!(size.width > 0 && size.height > 0, "the canvas has no size");
        anyhow::ensure!(
            size.width <= max && size.height <= max,
            "the canvas is {}x{}, larger than the GPU's limit of {max}",
            size.width,
            size.height
        );
        padded_row_bytes(size.width).map(|_| ())
    }

    fn start_pixels(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        options: RenderOptions,
    ) -> Result<Submitted> {
        let size = options.size;
        self.check_canvas(size)?;
        let bytes_per_row = padded_row_bytes(size.width)?;
        self.ensure_output(size, options.format);
        let output = self.output.clone().context("no target")?;
        let buffer = self.take_pixel_buffer(size)?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut target = self.take_target(size);
        self.encode_plan(&mut encoder, program, inputs, &mut target, (true, true));
        let linear = target.linear.clone();
        self.targets.insert(size, target);
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
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            output.size(),
        );
        let (timing, submission) = self.submit_frame(encoder);
        Ok(Submitted {
            submission,
            ready: map_for_read(&buffer),
            output: Output::Pixels {
                size,
                format: options.format,
                buffer,
            },
            timing,
        })
    }

    /// Draws a frame into one surface per distinct size of `sizes`, all in one submit. Targets of
    /// other sizes are dropped, with their simulation state; a size that is new takes over what it can of one.
    #[cfg(target_os = "macos")]
    fn start_surfaces(
        &mut self,
        program: &Program,
        inputs: FrameInputs,
        sizes: &[Size],
    ) -> Result<Submitted> {
        let mut distinct: Vec<Size> = Vec::new();
        for size in sizes {
            self.check_canvas(*size)?;
            if !distinct.contains(size) {
                distinct.push(*size);
            }
        }
        anyhow::ensure!(!distinct.is_empty(), "there is no canvas to draw");
        let gone: Vec<Size> = self
            .targets
            .keys()
            .filter(|size| !distinct.contains(size))
            .copied()
            .collect();
        let mut stale: Vec<Target> = gone
            .iter()
            .filter_map(|size| self.targets.remove(size))
            .collect();

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut surfaces = Vec::with_capacity(distinct.len());
        let last = distinct.len() - 1;
        for (i, size) in distinct.into_iter().enumerate() {
            let mut target = match self.targets.remove(&size) {
                Some(target) => target,
                None => match stale.pop() {
                    Some(old) => old.resized(&self.device, size),
                    None => Target::new(&self.device, size),
                },
            };
            let surface = self.encode_surface(
                &mut encoder,
                program,
                inputs,
                &mut target,
                (i == 0, i == last),
            );
            self.targets.insert(size, target);
            surfaces.push((size, surface?));
        }
        let (timing, submission) = self.submit_frame(encoder);
        Ok(Submitted {
            submission,
            ready: gpu_done(&self.queue),
            output: Output::Surfaces(surfaces),
            timing,
        })
    }

    #[cfg(target_os = "macos")]
    fn encode_surface(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        inputs: FrameInputs,
        target: &mut Target,
        timed: (bool, bool),
    ) -> Result<Surface> {
        if target.pool.is_none() {
            target.pool = Some(SurfacePool::new(target.size)?);
        }
        let pool = target.pool.as_mut().context("no surface pool")?;
        let (surface, drawn) = pool.next(&self.device)?;
        self.encode_plan(encoder, program, inputs, target, timed);
        let linear = target.linear.clone();
        self.presenter.encode(
            &self.device,
            encoder,
            &linear,
            &drawn.create_view(&Default::default()),
            PresentOptions {
                format: wgpu::TextureFormat::Bgra8Unorm,
                premultiply: true,
            },
        );
        Ok(surface)
    }

    /// Submits a frame's commands; the timestamps it wrote are read once the frame is done.
    fn submit_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
    ) -> (Option<Timing>, wgpu::SubmissionIndex) {
        let buffer = self.start_timing(&mut encoder);
        let submission = self.queue.submit([encoder.finish()]);
        let timing = buffer.map(|buffer| Timing {
            ready: map_for_read(&buffer),
            buffer,
        });
        (timing, submission)
    }

    /// Copies a finished frame out, and recycles its buffers.
    fn read_out(&mut self, submitted: Submitted) -> Result<Presented> {
        let Submitted { output, timing, .. } = submitted;
        let gpu_time = timing.and_then(|timing| self.read_seconds(timing));
        Ok(match output {
            Output::Pixels {
                size,
                format,
                buffer,
            } => {
                let pixels = read_pixels(&buffer, size)?;
                buffer.unmap();
                if self.free_pixels.len() < READBACK_DEPTH {
                    self.free_pixels.push((size, buffer));
                }
                Presented::Pixels(Frame {
                    size,
                    format,
                    pixels,
                    gpu_time,
                })
            }
            #[cfg(target_os = "macos")]
            Output::Surfaces(surfaces) => Presented::Surfaces { surfaces, gpu_time },
        })
    }

    fn take_pixel_buffer(&mut self, size: Size) -> Result<wgpu::Buffer> {
        self.free_pixels.retain(|(kept, _)| *kept == size);
        if let Some((_, buffer)) = self.free_pixels.pop() {
            return Ok(buffer);
        }
        let padded_row = u64::from(padded_row_bytes(size.width)?);
        Ok(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame readback"),
            size: padded_row * u64::from(size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }))
    }

    /// Encodes the pass timestamps into a buffer, when the device can time passes.
    fn start_timing(&mut self, encoder: &mut wgpu::CommandEncoder) -> Option<wgpu::Buffer> {
        let timer = self.timer.as_ref()?;
        let buffer = self.free_timing.pop().unwrap_or_else(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timing readback"),
                size: TIMING_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        timer.resolve(encoder, &buffer, 0);
        Some(buffer)
    }

    /// The seconds a frame's passes took, read from its mapped timestamps; the buffer is recycled.
    fn read_seconds(&mut self, timing: Timing) -> Option<f64> {
        let Timing { buffer, .. } = timing;
        let seconds = self.timer.as_ref().and_then(|timer| {
            let view = buffer.slice(..).get_mapped_range().ok()?;
            timer.seconds(&view)
        });
        buffer.unmap();
        if self.free_timing.len() < READBACK_DEPTH {
            self.free_timing.push(buffer);
        }
        seconds
    }

    /// The target readback frames are copied from, kept while its size and format stay the same.
    fn ensure_output(&mut self, size: Size, format: PixelFormat) {
        let format = format.texture_format();
        if !self.output.as_ref().is_some_and(|t| fits(t, size, format)) {
            self.output = Some(render_target(
                &self.device,
                "output",
                size,
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            ));
        }
    }
}

fn fits(texture: &wgpu::Texture, size: Size, format: wgpu::TextureFormat) -> bool {
    texture.width() == size.width && texture.height() == size.height && texture.format() == format
}

/// The rows of a mapped staging buffer, tightly packed, top first.
fn read_pixels(buffer: &wgpu::Buffer, size: Size) -> Result<Vec<u8>> {
    let row_bytes = size.width as usize * 4;
    let padded_row = padded_row_bytes(size.width)? as usize;
    let mapped = buffer.slice(..).get_mapped_range()?;
    let mut pixels = Vec::with_capacity(row_bytes * size.height as usize);
    for y in 0..size.height as usize {
        let row = mapped
            .get(y * padded_row..)
            .and_then(|rest| rest.get(..row_bytes))
            .context("the readback is shorter than the frame")?;
        pixels.extend_from_slice(row);
    }
    Ok(pixels)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use shader_graph::codegen::{PassDecl, RenderPlan};
    use shader_graph::{UniformField, UniformScalar};

    pub(crate) const CODE: &str = r"
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

    /// A plan drawing `code`, whose `Frame` block is laid out as `CODE` declares.
    pub(crate) fn plan(code: &str) -> RenderPlan {
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
                        UniformField {
                            offset: 16,
                            scalar: UniformScalar::U32,
                            components: 1,
                        },
                    ),
                ]),
            },
            uniforms: UniformBlock {
                binding: 1,
                size: 0,
                fields: BTreeMap::new(),
            },
            frame_values: UniformBlock {
                binding: 2,
                size: 0,
                fields: BTreeMap::new(),
            },
            frame_value_refs: BTreeMap::new(),
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
            frame: Default::default(),
        }
    }

    #[test]
    fn submitted_frames_come_out_as_rendered_ones() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let options = RenderOptions {
            size: Size::new(100, 50),
            format: PixelFormat::Bgra8,
            premultiply: true,
        };
        let first = FrameInputs {
            time: 0.25,
            delta: 0.0,
            frame: 0,
        };
        let second = FrameInputs {
            time: 0.75,
            delta: 0.0,
            frame: 1,
        };
        let expected = [
            renderer.render(&program, first, options).unwrap(),
            renderer.render(&program, second, options).unwrap(),
        ];

        renderer.submit(&program, first, options).unwrap();
        renderer.submit(&program, second, options).unwrap();
        assert!(
            renderer.submit(&program, first, options).is_err(),
            "no more frames than the readback depth wait at once"
        );
        let mut taken = Vec::new();
        while taken.len() < 2 {
            renderer
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            match renderer.take_frame() {
                Some(Ok(Presented::Pixels(frame))) => taken.push(frame),
                Some(_) => panic!("a readback came out as something else"),
                None => {}
            }
        }
        assert!(renderer.take_frame().is_none());
        for (frame, want) in taken.iter().zip(&expected) {
            assert_eq!(frame.size, want.size);
            assert_eq!(frame.pixels, want.pixels);
        }
        assert_ne!(taken[0].pixels, taken[1].pixels, "each frame is its own");
    }

    #[cfg(target_os = "macos")]
    fn draw_surfaces(
        renderer: &mut Renderer,
        program: &Program,
        inputs: FrameInputs,
        sizes: &[Size],
    ) -> Vec<(Size, Surface)> {
        renderer.submit_surfaces(program, inputs, sizes).unwrap();
        loop {
            renderer
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            match renderer.take_frame() {
                Some(Ok(Presented::Surfaces { surfaces, .. })) => break surfaces,
                Some(_) => panic!("a surface frame came out as something else"),
                None => {}
            }
        }
    }

    /// Holds a surface's base address locked for reading, so it is unlocked however the test exits.
    #[cfg(target_os = "macos")]
    struct ReadLock<'a>(&'a Surface);

    #[cfg(target_os = "macos")]
    impl<'a> ReadLock<'a> {
        fn new(surface: &'a Surface) -> Self {
            use objc2_core_video::{
                CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, kCVReturnSuccess,
            };
            // SAFETY: the buffer is one the test holds, and the lock is released by `Drop`.
            let status =
                unsafe { CVPixelBufferLockBaseAddress(surface, CVPixelBufferLockFlags::ReadOnly) };
            assert_eq!(status, kCVReturnSuccess);
            Self(surface)
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for ReadLock<'_> {
        fn drop(&mut self) {
            use objc2_core_video::{CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress};
            // SAFETY: balances the successful lock taken in `new`.
            unsafe { CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly) };
        }
    }

    /// The surface's rows, tightly packed.
    #[cfg(target_os = "macos")]
    fn surface_pixels(surface: &Surface, size: Size) -> Vec<u8> {
        use objc2_core_video::{CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow};

        let _lock = ReadLock::new(surface);
        let row_bytes = size.width as usize * 4;
        let base = CVPixelBufferGetBaseAddress(surface).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(surface);
        let mut pixels = Vec::with_capacity(row_bytes * size.height as usize);
        for y in 0..size.height as usize {
            // SAFETY: the buffer is locked for reading by `_lock`, and each row of `row_bytes` bytes starts
            // at `y * stride` inside it.
            pixels.extend_from_slice(unsafe {
                std::slice::from_raw_parts(base.add(y * stride), row_bytes)
            });
        }
        pixels
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn surface_holds_the_frame_the_readback_gives() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let size = Size::new(100, 50);
        let options = RenderOptions {
            size,
            format: PixelFormat::Bgra8,
            premultiply: true,
        };
        let inputs = FrameInputs {
            time: 0.5,
            delta: 0.0,
            frame: 0,
        };
        let expected = renderer.render(&program, inputs, options).unwrap();

        let surface = draw_surfaces(&mut renderer, &program, inputs, &[size])
            .remove(0)
            .1;
        assert_eq!(surface_pixels(&surface, size), expected.pixels);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn targets_of_different_sizes_draw_the_program_at_their_sizes_in_one_submit() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let (wide, tall) = (Size::new(100, 50), Size::new(30, 60));
        let inputs = FrameInputs {
            time: 0.5,
            delta: 0.0,
            frame: 0,
        };
        let read = |renderer: &mut Renderer, size| {
            let options = RenderOptions {
                size,
                format: PixelFormat::Bgra8,
                premultiply: true,
            };
            renderer.render(&program, inputs, options).unwrap().pixels
        };
        let expected = [read(&mut renderer, wide), read(&mut renderer, tall)];

        let surfaces = draw_surfaces(&mut renderer, &program, inputs, &[wide, tall]);
        assert_eq!(surfaces.len(), 2);
        for ((size, surface), want) in surfaces.iter().zip(&expected) {
            assert_eq!(surface_pixels(surface, *size), *want, "{size:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn targets_of_the_same_size_are_shared_and_unused_ones_dropped() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let (a, b) = (Size::new(40, 20), Size::new(20, 40));
        let inputs = FrameInputs {
            time: 0.5,
            delta: 0.0,
            frame: 0,
        };
        let surfaces = draw_surfaces(&mut renderer, &program, inputs, &[a, a, b, a]);
        assert_eq!(
            surfaces.iter().map(|(size, _)| *size).collect::<Vec<_>>(),
            [a, b]
        );
        assert_eq!(renderer.target_count(), 2);
        draw_surfaces(&mut renderer, &program, inputs, &[b]);
        assert_eq!(
            renderer.target_count(),
            1,
            "the target of a size no frame asks for is dropped"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_run_of_size_changes_keeps_one_target_and_draws_each_size() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&plan(CODE)).unwrap();
        let inputs = FrameInputs {
            time: 0.5,
            delta: 0.0,
            frame: 0,
        };
        for width in 300..360 {
            let size = Size::new(width, 200);
            let surfaces = draw_surfaces(&mut renderer, &program, inputs, &[size]);
            assert_eq!(surfaces[0].0, size);
            assert_eq!(renderer.target_count(), 1);
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
        let inputs = FrameInputs {
            time: 1.0,
            delta: 0.0,
            frame: 0,
        };
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
        let renderer = Renderer::new().unwrap();
        let error = renderer
            .compile(&plan(&CODE.replace("frame.time", "frame.nothing")))
            .err()
            .unwrap();
        assert!(!error.diagnostics.is_empty());
        assert!(
            error.diagnostics[0]
                .location
                .is_some_and(|(line, _)| line > 0)
        );
    }
}
