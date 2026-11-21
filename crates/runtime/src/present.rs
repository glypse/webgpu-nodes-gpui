use std::collections::HashMap;

/// Turns the linear-sRGB colour a program drew into a display encoding:
/// applies the sRGB transfer curve, and premultiplies alpha when asked
/// (exports keep it straight).
const PRESENT_WGSL: &str = r"
override premultiply: bool = true;

@group(0) @binding(0) var source: texture_2d<f32>;

fn encode(c: vec3f) -> vec3f {
    let v = max(c, vec3f(0.0));
    return select(1.055 * pow(v, vec3f(1.0 / 2.4)) - 0.055, v * 12.92, v <= vec3f(0.0031308));
}

@vertex
fn present_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32(index & 1u) * 4.0 - 1.0, f32(index >> 1u) * 4.0 - 1.0);
    return vec4f(corner, 0.0, 1.0);
}

@fragment
fn present_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let color = textureLoad(source, vec2u(position.xy), 0);
    let rgb = encode(color.rgb);
    let alpha = clamp(color.a, 0.0, 1.0);
    if (premultiply) {
        return vec4f(rgb * alpha, alpha);
    }
    return vec4f(rgb, alpha);
}

fn premultiplied_display(color: vec4f) -> vec3f {
    return encode(color.rgb) * clamp(color.a, 0.0, 1.0);
}

// Full-range BT.601 YCbCr, the matrix a window's NV12 surfaces are decoded with.
@fragment
fn luma_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let rgb = premultiplied_display(textureLoad(source, vec2u(position.xy), 0));
    return vec4f(dot(rgb, vec3f(0.299, 0.587, 0.114)), 0.0, 0.0, 1.0);
}

// Each chroma sample is the mean of the 2x2 pixels it covers.
@fragment
fn chroma_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let size = textureDimensions(source);
    let first = vec2u(position.xy) * 2u;
    var rgb = vec3f(0.0);
    for (var i = 0u; i < 4u; i++) {
        let at = min(first + vec2u(i & 1u, i >> 1u), size - vec2u(1u));
        rgb += premultiplied_display(textureLoad(source, at, 0));
    }
    rgb *= 0.25;
    let cb = 0.5 - 0.168736 * rgb.r - 0.331264 * rgb.g + 0.5 * rgb.b;
    let cr = 0.5 + 0.5 * rgb.r - 0.418688 * rgb.g - 0.081312 * rgb.b;
    return vec4f(cb, cr, 0.0, 1.0);
}
";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresentOptions {
    pub format: wgpu::TextureFormat,
    pub premultiply: bool,
}

/// Pipelines presenting a linear texture, one per output format and encoding.
pub struct Presenter {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<PresentOptions, wgpu::RenderPipeline>,
    #[cfg(target_os = "macos")]
    planes: Planes,
}

#[cfg(target_os = "macos")]
struct Planes {
    luma: wgpu::RenderPipeline,
    chroma: wgpu::RenderPipeline,
}

impl Presenter {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present"),
            source: wgpu::ShaderSource::Wgsl(PRESENT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        #[cfg(target_os = "macos")]
        let planes = Planes {
            luma: plane_pipeline(device, &module, &pipeline_layout, "luma_fragment", wgpu::TextureFormat::R8Unorm),
            chroma: plane_pipeline(device, &module, &pipeline_layout, "chroma_fragment", wgpu::TextureFormat::Rg8Unorm),
        };
        Self {
            module,
            layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            #[cfg(target_os = "macos")]
            planes,
        }
    }

    fn bind_group(&self, device: &wgpu::Device, source: &wgpu::Texture) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&source.create_view(&Default::default())),
            }],
        })
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        options: PresentOptions,
    ) -> &wgpu::RenderPipeline {
        self.pipelines.entry(options).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("present"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("present_vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("present_fragment"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[("premultiply", if options.premultiply { 1.0 } else { 0.0 })],
                        ..Default::default()
                    },
                    targets: &[Some(options.format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Encodes a pass drawing `source` into `target`, both the same size.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        target: &wgpu::TextureView,
        options: PresentOptions,
    ) {
        let bind_group = self.bind_group(device, source);
        let pipeline = self.pipeline(device, options);
        draw_full_screen(encoder, target, pipeline, &bind_group);
    }

    /// Encodes passes writing the luma and chroma planes of a surface from `source`, each sized for its plane.
    #[cfg(target_os = "macos")]
    pub fn encode_planes(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        luma: &wgpu::TextureView,
        chroma: &wgpu::TextureView,
    ) {
        let bind_group = self.bind_group(device, source);
        draw_full_screen(encoder, luma, &self.planes.luma, &bind_group);
        draw_full_screen(encoder, chroma, &self.planes.chroma, &bind_group);
    }
}

fn draw_full_screen(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("present"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(target_os = "macos")]
fn plane_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("present_vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    })
}
