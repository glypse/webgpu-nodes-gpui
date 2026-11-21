use std::collections::BTreeMap;

use anyhow::{Context as _, Result};

use crate::clock::FrameInputs;
use crate::gpu_timer::{GpuTimer, TIMING_BYTES};
use crate::present::{PresentOptions, Presenter};
use shader_graph::ShaderProgram;

use crate::uniforms::{UniformBlock, pack_block};

/// The format programs draw into: linear, and wide enough for colours outside sRGB.
const LINEAR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Rows copied out of a texture start on multiples of this.
const ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

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

/// A compiled program with its own uniform buffers.
pub struct Program {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    frame_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    source: ShaderProgram,
}

impl Program {
    pub fn source(&self) -> &ShaderProgram {
        &self.source
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
    /// Seconds the program's pass took on the GPU, on devices that can time it.
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

/// Draws programs with wgpu on a device of its own and reads the frames back,
/// so they can be shown without sharing the UI's GPU context.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    presenter: Presenter,
    timer: Option<GpuTimer>,
    linear: Option<wgpu::Texture>,
    output: Option<wgpu::Texture>,
    readback: Option<(Size, Readback)>,
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
                ..Default::default()
            }))?;
        // wgpu's default handler panics on errors outside an error scope.
        device.on_uncaptured_error(std::sync::Arc::new(|error| log::error!("GPU error: {error}")));
        let timer = GpuTimer::new(&device, &queue);
        Ok(Self {
            presenter: Presenter::new(&device),
            device,
            queue,
            timer,
            linear: None,
            output: None,
            readback: None,
        })
    }

    /// Compiles a program; its uniform block stays zero until `set_uniforms`.
    pub fn compile(&mut self, source: &ShaderProgram) -> Result<Program, ProgramError> {
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

        let blocks = [&source.frame, &source.uniforms];
        let layout_entries: Vec<_> = blocks
            .iter()
            .filter(|block| block.size > 0)
            .map(|block| wgpu::BindGroupLayoutEntry {
                binding: block.binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
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
                    targets: &[Some(LINEAR_FORMAT.into())],
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
        let bind_entries: Vec<_> = [
            (&source.frame, &frame_buffer),
            (&source.uniforms, &uniform_buffer),
        ]
        .into_iter()
        .filter(|(block, _)| block.size > 0)
        .map(|(block, buffer)| wgpu::BindGroupEntry {
            binding: block.binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("program"),
            layout: &layout,
            entries: &bind_entries,
        });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(fail(error.to_string(), None));
        }
        let program = Program {
            pipeline,
            bind_group,
            frame_buffer,
            uniform_buffer,
            source: source.clone(),
        };
        Ok(program)
    }

    /// Writes the values of a program's uniform block, e.g. after an edit that doesn't change its code.
    pub fn set_uniforms(&self, program: &Program, values: &BTreeMap<String, Vec<f64>>) {
        let block = &program.source.uniforms;
        if block.size == 0 {
            return;
        }
        let mut bytes = vec![0u8; block.size as usize];
        pack_block(block, values, &mut bytes);
        self.queue.write_buffer(&program.uniform_buffer, 0, &bytes);
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

        let frame_block = &program.source.frame;
        if frame_block.size > 0 {
            let mut bytes = vec![0u8; frame_block.size as usize];
            let values = BTreeMap::from([
                ("time".to_string(), vec![inputs.time]),
                ("delta".to_string(), vec![inputs.delta]),
                ("frame".to_string(), vec![inputs.frame as f64]),
                (
                    "resolution".to_string(),
                    vec![f64::from(size.width), f64::from(size.height)],
                ),
            ]);
            pack_block(frame_block, &values, &mut bytes);
            self.queue.write_buffer(&program.frame_buffer, 0, &bytes);
        }

        let (linear, output) = (
            self.linear.as_ref().context("no target")?,
            self.output.as_ref().context("no target")?,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let view = linear.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("program"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: self.timer.as_ref().map(GpuTimer::writes),
                ..Default::default()
            });
            pass.set_pipeline(&program.pipeline);
            pass.set_bind_group(0, &program.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.presenter.encode(
            &self.device,
            &mut encoder,
            linear,
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
        for row in mapped
            .chunks(padded_row as usize)
            .take(size.height as usize)
        {
            pixels.extend_from_slice(&row[..(size.width * 4) as usize]);
        }
        drop(mapped);
        readback.pixels.unmap();
        let gpu_time = self.timer.as_ref().and_then(|timer| {
            let seconds = timer.seconds(&timing_slice.get_mapped_range().ok()?);
            readback.timing.unmap();
            seconds
        });
        Ok(Frame {
            size,
            format: options.format,
            pixels,
            gpu_time,
        })
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

    fn source(code: &str) -> ShaderProgram {
        let field = |offset, components| UniformField {
            offset,
            scalar: UniformScalar::F32,
            components,
        };
        ShaderProgram {
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
        }
    }

    #[test]
    fn draws_a_frame_with_its_inputs() {
        let mut renderer = Renderer::new().unwrap();
        let program = renderer.compile(&source(CODE)).unwrap();
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
        let mut renderer = Renderer::new().unwrap();
        let error = renderer
            .compile(&source(&CODE.replace("frame.time", "frame.nothing")))
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
