//! Decoding the images project assets give into what programs read: linear,
//! straight-alpha half floats.

use std::io::Cursor;

use half::f16;
use image::{DynamicImage, ImageDecoder as _, ImageReader, Limits};
use thiserror::Error;

use crate::Size;

mod ffmpeg_decoder;
mod library;
mod video;

pub use library::{MediaLibrary, MediaSource, MediaStatus};
pub use video::SystemVideoProbe;

/// Pixels an image may have, whatever the GPU's side limit: bounds the memory
/// a decode takes, since a small file can declare a huge image.
const MAX_PIXELS: u64 = 8192 * 8192;
/// Bytes the decoder may allocate for one image: its own pixels plus RGBA8 and
/// the f16 texels built from them.
const MAX_DECODE_BYTES: u64 = MAX_PIXELS * 16;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("the image can't be decoded: {0}")]
    Decode(String),
    #[error("the image is {width} by {height} pixels, more than the GPU takes ({max})")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("the image is {width} by {height} pixels, more than can be decoded")]
    TooManyPixels { width: u32, height: u32 },
    #[error("the image's pixels don't match its size")]
    Size,
    #[error("the video can't be decoded: {0}")]
    Video(String),
}

/// An image as the texels of a texture: linear RGBA half floats, rows top first.
pub struct DecodedImage {
    pub size: Size,
    /// Little-endian f16, four per pixel.
    pub texels: Vec<u8>,
}

/// An sRGB-encoded value as linear light.
fn linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) }
}

/// RGBA8 pixels, sRGB-encoded with straight alpha, as linear half floats.
pub fn texels_of_srgb8(size: Size, rgba: &[u8]) -> Result<DecodedImage, MediaError> {
    if size.width == 0 || size.height == 0 || rgba.len() != size.width as usize * size.height as usize * 4 {
        return Err(MediaError::Size);
    }
    let table: Vec<f16> = (0..=255u8).map(|v| f16::from_f32(linear(f32::from(v) / 255.0))).collect();
    let mut texels = vec![0u8; rgba.len() * 2];
    for (pixel, texel) in rgba.as_chunks::<4>().0.iter().zip(texels.as_chunks_mut::<8>().0.iter_mut()) {
        for (channel, (value, bytes)) in pixel.iter().zip(texel.as_chunks_mut::<2>().0.iter_mut()).enumerate() {
            let half = if channel < 3 {
                table[usize::from(*value)]
            } else {
                f16::from_f32(f32::from(*value) / 255.0)
            };
            *bytes = half.to_le_bytes();
        }
    }
    Ok(DecodedImage { size, texels })
}

/// Decodes an image file, rotated upright by its orientation.
pub fn decode_image(file: &[u8], max_dimension: u32) -> Result<DecodedImage, MediaError> {
    let describe = |e: image::ImageError| MediaError::Decode(e.to_string());
    let mut reader = ImageReader::new(Cursor::new(file)).with_guessed_format().map_err(|e| MediaError::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(describe)?;
    let (width, height) = decoder.dimensions();
    if width > max_dimension || height > max_dimension {
        return Err(MediaError::TooLarge { width, height, max: max_dimension });
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(MediaError::TooManyPixels { width, height });
    }
    let orientation = decoder.orientation().map_err(describe)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(describe)?;
    image.apply_orientation(orientation);
    let rgba = image.to_rgba8();
    texels_of_srgb8(Size::new(rgba.width(), rgba.height()), rgba.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(pixel));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn texel(image: &DecodedImage, index: usize) -> [f32; 4] {
        std::array::from_fn(|c| {
            let at = (index * 4 + c) * 2;
            f16::from_le_bytes([image.texels[at], image.texels[at + 1]]).to_f32()
        })
    }

    #[test]
    fn decodes_to_linear_straight_alpha() {
        let image = decode_image(&png(3, 2, [255, 128, 0, 128]), 8192).expect("decodes");
        assert_eq!(image.size, Size::new(3, 2));
        let [r, g, b, a] = texel(&image, 4);
        assert_eq!((r, b), (1.0, 0.0));
        assert!((g - 0.2158).abs() < 1e-3, "sRGB 128 is about 0.216 linear: {g}");
        assert!((a - 128.0 / 255.0).abs() < 1e-3, "alpha stays straight: {a}");
    }

    #[test]
    fn refuses_what_isnt_an_image_or_is_too_large() {
        assert!(matches!(decode_image(b"not an image", 8192), Err(MediaError::Decode(_))));
        assert!(matches!(decode_image(&png(9, 1, [0; 4]), 8), Err(MediaError::TooLarge { .. })));
        assert!(matches!(texels_of_srgb8(Size::new(2, 2), &[0; 4]), Err(MediaError::Size)));
    }
}
