use shader_graph::passes::{TextureFormat, TextureSampleType};

/// The wgpu format a program's texture format stands for.
pub fn wgpu_format(format: TextureFormat) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    match format {
        TextureFormat::Rgba16Float => F::Rgba16Float,
        TextureFormat::Rg16Float => F::Rg16Float,
        TextureFormat::R16Float => F::R16Float,
        TextureFormat::Rgba32Float => F::Rgba32Float,
        TextureFormat::Rg32Float => F::Rg32Float,
        TextureFormat::R32Float => F::R32Float,
        TextureFormat::Rgba8Unorm => F::Rgba8Unorm,
        TextureFormat::Rgba32Uint => F::Rgba32Uint,
        TextureFormat::Rg32Uint => F::Rg32Uint,
        TextureFormat::R32Uint => F::R32Uint,
        TextureFormat::Rgba32Sint => F::Rgba32Sint,
        TextureFormat::Rg32Sint => F::Rg32Sint,
        TextureFormat::R32Sint => F::R32Sint,
    }
}

pub fn wgpu_sample_type(sample: TextureSampleType) -> wgpu::TextureSampleType {
    match sample {
        TextureSampleType::Float => wgpu::TextureSampleType::Float { filterable: true },
        TextureSampleType::UnfilterableFloat => wgpu::TextureSampleType::Float { filterable: false },
        TextureSampleType::Sint => wgpu::TextureSampleType::Sint,
        TextureSampleType::Uint => wgpu::TextureSampleType::Uint,
    }
}
