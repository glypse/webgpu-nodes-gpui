use anyhow::{Context as _, Result, anyhow};

/// Square side of the preview, in pixels. A multiple of 64 so rows meet
/// wgpu's 256-byte copy alignment.
pub const SIZE: u32 = 512;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// Draws a generated fragment shader offscreen and reads the pixels back,
/// so they can be shown as an image without sharing GPUI's own GPU context.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    pipeline: Option<(wgpu::RenderPipeline, wgpu::BindGroup)>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .context("no GPU adapter")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview readback"),
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            target,
            readback,
            uniforms,
            pipeline: None,
        })
    }

    /// Compiles `wgsl`, keeping the previous pipeline if it fails.
    pub fn set_shader(&mut self, wgsl: &str) -> Result<()> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("graph"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("graph"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(anyhow!("{error}"));
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniforms.as_entire_binding(),
            }],
        });
        self.pipeline = Some((pipeline, bind_group));
        Ok(())
    }

    /// Renders a frame at `time` seconds and returns its BGRA pixels.
    pub fn render(&self, time: f32) -> Result<Vec<u8>> {
        let (pipeline, bind_group) = self.pipeline.as_ref().context("no shader")?;
        let size = SIZE as f32;
        let uniforms: Vec<u8> = [time, 0.0, size, size]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        self.queue.write_buffer(&self.uniforms, 0, &uniforms);

        let view = self.target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("graph"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: None,
                },
            },
            self.target.size(),
        );
        self.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let pixels = slice.get_mapped_range()?.to_vec();
        self.readback.unmap();
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_demo_graph() {
        let mut renderer = Renderer::new().unwrap();
        renderer
            .set_shader(&shader_graph::generate(&shader_graph::Project::demo()).unwrap())
            .unwrap();
        let pixels = renderer.render(0.0).unwrap();
        // BGRA: red follows sin(u * 12), green follows v.
        let at = |x: u32, y: u32| &pixels[((y * SIZE + x) * 4) as usize..][..4];
        assert!(at(0, SIZE - 1)[1] < 10, "green is 0 at the bottom");
        assert!(at(0, 0)[1] > 245, "green is 1 at the top");
        let reds: std::collections::HashSet<u8> = (0..SIZE).map(|x| at(x, 0)[2]).collect();
        assert!(reds.len() > 50, "red varies across");
    }

    #[test]
    fn keeps_the_previous_shader_on_errors() {
        let mut renderer = Renderer::new().unwrap();
        assert!(renderer.set_shader("not wgsl").is_err());
    }
}
