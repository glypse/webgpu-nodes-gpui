use std::collections::HashMap;

use shader_graph::passes::{TextureFormat, mip_level_count};

use crate::formats::wgpu_format;
use crate::renderer::{LINEAR_FORMAT, Program, Size};
use crate::simulation::SimulationState;
#[cfg(target_os = "macos")]
use crate::surface::SurfacePool;

/// The frame and frame-value uniform blocks of one pass. Their values depend on the
/// canvas a frame is drawn at, so each target keeps its own for every pass: that is
/// what lets the frames of several targets share one submit.
pub(crate) struct PassBlocks {
    pub frame: wgpu::Buffer,
    pub frame_values: wgpu::Buffer,
}

/// Everything drawing at one canvas size keeps between frames: the linear texture
/// frames are drawn into, the passes' own textures, the simulations' state and the
/// surfaces shown. Media and uniform values are the renderer's, shared by all targets.
pub(crate) struct Target {
    pub size: Size,
    pub linear: wgpu::Texture,
    pub transients: HashMap<String, wgpu::Texture>,
    pub simulation: SimulationState,
    /// The blocks of the program with this id, one entry per pass.
    blocks: Option<(u64, Vec<PassBlocks>)>,
    #[cfg(target_os = "macos")]
    pub pool: Option<SurfacePool>,
}

impl Target {
    /// A target for `size` made out of one for another size. The blocks and the transient textures
    /// are kept; the transients are remade on first use at the new size. The linear texture, the
    /// simulation state and the surface pool start over.
    pub fn resized(self, device: &wgpu::Device, size: Size) -> Self {
        Self { transients: self.transients, blocks: self.blocks, ..Self::new(device, size) }
    }

    pub fn new(device: &wgpu::Device, size: Size) -> Self {
        Self {
            size,
            linear: render_target(
                device,
                "linear",
                size,
                LINEAR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            transients: HashMap::new(),
            simulation: SimulationState::default(),
            blocks: None,
            #[cfg(target_os = "macos")]
            pool: None,
        }
    }

    /// Makes the blocks the program's passes write, where they are not the program's already.
    pub fn ensure_blocks(&mut self, device: &wgpu::Device, program: &Program) {
        if self.blocks.as_ref().is_some_and(|(id, _)| *id == program.id) {
            return;
        }
        let buffer = |label, size: u32| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(size.max(16)),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let blocks = program
            .passes
            .iter()
            .map(|pass| PassBlocks {
                frame: buffer("frame", pass.decl.program.frame.size),
                frame_values: buffer("frame values", pass.decl.program.frame_values.size),
            })
            .collect();
        self.blocks = Some((program.id, blocks));
    }

    pub fn pass_blocks(&self, index: usize) -> Option<&PassBlocks> {
        self.blocks.as_ref()?.1.get(index)
    }

    /// A pass's own texture, of `size` and `format`, with all its mip levels if `mips`.
    pub fn transient(
        &mut self,
        device: &wgpu::Device,
        id: &str,
        size: Size,
        format: TextureFormat,
        mips: bool,
    ) -> wgpu::Texture {
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
        let texture = device.create_texture(&wgpu::TextureDescriptor {
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
}

pub(crate) fn render_target(
    device: &wgpu::Device,
    label: &str,
    size: Size,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
