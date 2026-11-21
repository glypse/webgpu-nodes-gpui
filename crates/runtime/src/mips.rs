use std::collections::HashMap;

const SHADER: &str = r"
@group(0) @binding(0) var source: texture_2d<f32>;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let corner = vec2f(f32(index & 1u) * 4.0 - 1.0, f32(index >> 1u) * 4.0 - 1.0);
    return vec4f(corner, 0.0, 1.0);
}

// The mean of a 2x2 block; the last row or column of an odd size is read twice.
@fragment
fn fragment_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let last = vec2i(textureDimensions(source)) - 1;
    let base = vec2i(position.xy) * 2;
    let a = textureLoad(source, min(base, last), 0);
    let b = textureLoad(source, min(base + vec2i(1, 0), last), 0);
    let c = textureLoad(source, min(base + vec2i(0, 1), last), 0);
    let d = textureLoad(source, min(base + vec2i(1, 1), last), 0);
    return (a + b + c + d) * 0.25;
}
";

/// Fills the mip levels of textures their pass drew the first level of, each
/// level the mean of 2x2 texels of the one before it. One pipeline per format.
pub struct MipGenerator {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl MipGenerator {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mips"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        // Unfilterable binds any float texture, and reads are exact texels.
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mips"),
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
            label: Some("mips"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self { module, layout, pipeline_layout, pipelines: HashMap::new() }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mips"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vertex_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fragment_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Encodes the passes making every level of `texture` after its first.
    pub fn generate(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) {
        let level_view = |level| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for level in 1..texture.mip_level_count() {
            let source = level_view(level - 1);
            let target = level_view(level);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mips"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source),
                }],
            });
            let pipeline = self.pipeline(device, texture.format());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mips"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
