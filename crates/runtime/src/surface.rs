//! Frames drawn into IOSurface-backed buffers that a window shows, so they
//! never cross to the CPU. The buffers are BGRA, the layout the window's image
//! path samples, and each is wrapped as a wgpu render target.

use std::ptr::{self, NonNull};

use anyhow::{Context as _, Result};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferPool,
    kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};

use crate::renderer::Size;

/// A frame's buffer, which the window shows once it has been drawn.
pub type Surface = CFRetained<CVPixelBuffer>;

/// Hands out surfaces of one size, each with the texture it is drawn through.
/// CoreVideo recycles a surface once the window has let go of it, so a surface
/// is never drawn while it is shown.
pub struct SurfacePool {
    pool: CFRetained<CVPixelBufferPool>,
    /// One texture per IOSurface the pool has handed out. Keeping the IOSurface
    /// alive also keeps its address from being reused while the entry exists.
    textures: Vec<(CFRetained<IOSurfaceRef>, wgpu::Texture)>,
}

impl SurfacePool {
    pub fn new(size: Size) -> Result<Self> {
        let format = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
        let width = CFNumber::new_i32(i32::try_from(size.width)?);
        let height = CFNumber::new_i32(i32::try_from(size.height)?);
        let io_surface_properties = CFDictionary::<CFType, CFType>::empty();
        // SAFETY: CoreVideo exports these keys as process-wide statics that stay valid for the process's lifetime.
        let (format_key, width_key, height_key, io_key) = unsafe {
            (
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferWidthKey,
                kCVPixelBufferHeightKey,
                kCVPixelBufferIOSurfacePropertiesKey,
            )
        };
        let keys = [format_key, width_key, height_key, io_key];
        let values: [&CFType; 4] = [&format, &width, &height, &io_surface_properties];
        let attributes = CFDictionary::from_slices(&keys, &values);

        let mut pool = ptr::null_mut();
        // SAFETY: the attributes live for the call, and the output is writable.
        let status = unsafe {
            CVPixelBufferPool::create(
                None,
                None,
                Some(attributes.as_opaque()),
                NonNull::from(&mut pool),
            )
        };
        anyhow::ensure!(
            status == kCVReturnSuccess,
            "the surface pool wasn't created: status {status}"
        );
        let pool = NonNull::new(pool).context("the surface pool wasn't created")?;
        // SAFETY: the successful call hands over one retained reference.
        let pool = unsafe { CFRetained::from_raw(pool) };
        Ok(Self {
            pool,
            textures: Vec::new(),
        })
    }

    /// The next surface the pool can give, with the texture drawing into it.
    pub fn next(&mut self, device: &wgpu::Device) -> Result<(Surface, wgpu::Texture)> {
        let mut buffer = ptr::null_mut();
        // SAFETY: the pool is live, and the output is writable.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, &self.pool, NonNull::from(&mut buffer))
        };
        anyhow::ensure!(
            status == kCVReturnSuccess,
            "the surface pool gave no buffer: status {status}"
        );
        let buffer = NonNull::new(buffer).context("the surface pool gave no buffer")?;
        // SAFETY: the successful call hands over one retained reference.
        let surface: Surface = unsafe { CFRetained::from_raw(buffer) };

        let io_surface =
            CVPixelBufferGetIOSurface(Some(&surface)).context("the surface has no IOSurface")?;
        let known = self
            .textures
            .iter()
            .find(|(known, _)| CFRetained::as_ptr(known) == CFRetained::as_ptr(&io_surface));
        if let Some((_, texture)) = known {
            return Ok((surface, texture.clone()));
        }
        let texture = wrap(device, &surface, &io_surface)?;
        self.textures.push((io_surface, texture.clone()));
        Ok((surface, texture))
    }
}

/// The surface's pixels as a wgpu texture, which the frame is drawn into.
/// The texture keeps the IOSurface alive for as long as it is used.
fn wrap(
    device: &wgpu::Device,
    surface: &CVPixelBuffer,
    io_surface: &IOSurfaceRef,
) -> Result<wgpu::Texture> {
    anyhow::ensure!(
        CVPixelBufferGetPixelFormatType(surface) == kCVPixelFormatType_32BGRA,
        "the surface isn't 32BGRA, so it can't be drawn"
    );
    let width = u32::try_from(CVPixelBufferGetWidth(surface)).context("the surface is too wide")?;
    let height =
        u32::try_from(CVPixelBufferGetHeight(surface)).context("the surface is too tall")?;
    let raw_device = {
        // SAFETY: the device is only read for its Metal handle.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }
            .context("the GPU isn't Metal, so surfaces can't be drawn")?;
        hal.raw_device().clone()
    };
    // SAFETY: a plain constructor; Metal validates the values when the texture is made below.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::BGRA8Unorm,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(MTLStorageMode::Shared);
    let raw: objc2::rc::Retained<ProtocolObject<dyn MTLTexture>> = raw_device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, io_surface, 0)
        .context("the device can't wrap a surface")?;

    let copy_size = wgpu::hal::CopyExtent {
        width,
        height,
        depth: 1,
    };
    // SAFETY: the texture was made from a BGRA8 descriptor of `copy_size`, one layer and one mip
    // level, which is what the arguments below describe; wgpu takes the reference the texture holds.
    let hal = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            raw,
            wgpu::TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            copy_size,
            None,
        )
    };
    let descriptor = wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    // SAFETY: the HAL texture was made on this device with the same format and size.
    Ok(unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal,
            &descriptor,
            wgpu::TextureUses::COLOR_TARGET,
        )
    })
}
