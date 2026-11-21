//! Video encoders. The export drives one through `VideoEncoder`, so another
//! encoder can be swapped in without touching the frame loop.

use std::path::Path;
use runtime::Size;

use crate::{ExportError, Format, Quality};

/// Takes frames of BGRA pixels, top row first, at the size it was opened with.
pub trait VideoEncoder: Sized {
    fn open(path: &Path, format: Format, quality: Quality, size: Size, fps: f64) -> Result<Self, ExportError>;

    /// Frames are appended in order, each one frame apart.
    fn push(&mut self, bgra: &[u8]) -> Result<(), ExportError>;

    /// Writes the file out completely.
    fn finish(self) -> Result<(), ExportError>;

    /// Stops without finishing the file; the caller removes it.
    fn abandon(self);
}

/// The average bitrate in bits per second for `quality` at `size`: mediabunny's
/// H.264 estimate, which the original's export uses. It scales with the frame's
/// pixels and not with its frame rate.
pub fn average_bitrate(quality: Quality, size: Size) -> i64 {
    const REFERENCE_PIXELS: f64 = 1920.0 * 1080.0;
    const REFERENCE_BITRATE: f64 = 3_000_000.0;
    let pixels = f64::from(size.width) * f64::from(size.height);
    let scaled = REFERENCE_BITRATE * (pixels / REFERENCE_PIXELS).powf(0.95);
    let bitrate = scaled * quality_factor(quality);
    ((bitrate / 1000.0).ceil() * 1000.0) as i64
}

/// The multiplier of mediabunny's `Quality` levels (0.5, 0.75 and 1 on its 0-1 scale).
fn quality_factor(quality: Quality) -> f64 {
    let level: f64 = match quality {
        Quality::Medium => 0.5,
        Quality::High => 0.75,
        Quality::VeryHigh => 1.0,
    };
    0.3 * (2.5538 * level).exp()
}

/// The frame timestamps' timescale: milliframes, so fractional rates stay exact.
pub fn timescale(fps: f64) -> i32 {
    (fps * 1000.0).round() as i32
}

#[cfg(target_os = "macos")]
pub use av::AvFoundationEncoder;

#[cfg(target_os = "macos")]
mod av {
    use std::ptr::{self, NonNull};
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
        AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
        AVVideoCodecTypeH264, AVVideoColorPrimariesKey, AVVideoColorPrimaries_ITU_R_709_2,
        AVVideoColorPropertiesKey, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
        AVVideoMaxKeyFrameIntervalKey, AVVideoTransferFunctionKey,
        AVVideoTransferFunction_IEC_sRGB, AVVideoWidthKey, AVVideoYCbCrMatrixKey,
        AVVideoYCbCrMatrix_ITU_R_709_2,
    };
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
        kCVPixelFormatType_32BGRA, kCVReturnSuccess,
    };
    use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL};

    use super::{VideoEncoder, average_bitrate, timescale};
    use std::path::Path;

    use runtime::Size;

    use crate::{ExportError, Format, Quality};

    /// H.264 in an MP4, through AVAssetWriter: hardware encoding on macOS.
    pub struct AvFoundationEncoder {
        writer: Retained<AVAssetWriter>,
        input: Retained<AVAssetWriterInput>,
        adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
        size: Size,
        timescale: i32,
        next_frame: i64,
    }

    fn write_error(message: impl Into<String>) -> ExportError {
        ExportError::Encode(message.into())
    }

    fn ns_error(error: &NSError) -> String {
        error.localizedDescription().to_string()
    }

    fn key(name: Option<&'static NSString>, what: &str) -> Result<&'static NSString, ExportError> {
        name.ok_or_else(|| write_error(format!("AVFoundation has no {what} key")))
    }

    impl VideoEncoder for AvFoundationEncoder {
        fn open(path: &Path, format: Format, quality: Quality, size: Size, fps: f64) -> Result<Self, ExportError> {
            if format != Format::Mp4 {
                return Err(ExportError::Unsupported(format!("{} export isn't available yet", format.label())));
            }
            let size = crate::video_size(size);
            let path_text = NSString::from_str(&path.to_string_lossy());
            let url = NSURL::fileURLWithPath(&path_text);
            let file_type = unsafe { AVFileTypeMPEG4 }
                .ok_or_else(|| write_error("AVFoundation has no MP4 file type"))?;
            let writer = unsafe {
                AVAssetWriter::initWithURL_fileType_error(AVAssetWriter::alloc(), &url, file_type)
            }
            .map_err(|error| write_error(ns_error(&error)))?;

            let bitrate = average_bitrate(quality, size);
            let compression = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    key(unsafe { AVVideoAverageBitRateKey }, "average bitrate")?,
                    key(unsafe { AVVideoMaxKeyFrameIntervalKey }, "keyframe interval")?,
                ],
                &[
                    &*NSNumber::new_i64(bitrate) as &AnyObject,
                    &*NSNumber::new_i64(fps.round().max(1.0) as i64) as &AnyObject,
                ],
            );
            // The frames are sRGB: BT.709 primaries with the sRGB transfer, as the original tags them.
            let colour = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    key(unsafe { AVVideoColorPrimariesKey }, "colour primaries")?,
                    key(unsafe { AVVideoTransferFunctionKey }, "transfer function")?,
                    key(unsafe { AVVideoYCbCrMatrixKey }, "YCbCr matrix")?,
                ],
                &[
                    key(unsafe { AVVideoColorPrimaries_ITU_R_709_2 }, "BT.709 primaries")? as &AnyObject,
                    key(unsafe { AVVideoTransferFunction_IEC_sRGB }, "sRGB transfer")? as &AnyObject,
                    key(unsafe { AVVideoYCbCrMatrix_ITU_R_709_2 }, "BT.709 matrix")? as &AnyObject,
                ],
            );
            let codec = unsafe { AVVideoCodecTypeH264 }
                .ok_or_else(|| write_error("AVFoundation has no H.264 codec"))?;
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    key(unsafe { AVVideoCodecKey }, "codec")?,
                    key(unsafe { AVVideoWidthKey }, "width")?,
                    key(unsafe { AVVideoHeightKey }, "height")?,
                    key(unsafe { AVVideoCompressionPropertiesKey }, "compression")?,
                    key(unsafe { AVVideoColorPropertiesKey }, "colour properties")?,
                ],
                &[
                    codec as &AnyObject,
                    &*NSNumber::new_i64(i64::from(size.width)) as &AnyObject,
                    &*NSNumber::new_i64(i64::from(size.height)) as &AnyObject,
                    &*compression as &AnyObject,
                    &*colour as &AnyObject,
                ],
            );
            let media = unsafe { AVMediaTypeVideo }.ok_or_else(|| write_error("AVFoundation has no video type"))?;
            let input = unsafe { AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(media, Some(&settings)) };
            unsafe { input.setExpectsMediaDataInRealTime(false) };
            let adaptor = unsafe {
                AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                    &input, None,
                )
            };
            if !unsafe { writer.canAddInput(&input) } {
                return Err(write_error("the writer refuses the video input"));
            }
            unsafe { writer.addInput(&input) };
            if !unsafe { writer.startWriting() } {
                let detail = unsafe { writer.error() }.map(|e| ns_error(&e)).unwrap_or_default();
                return Err(write_error(detail));
            }
            unsafe { writer.startSessionAtSourceTime(CMTime::new(0, 1)) };

            Ok(Self {
                writer,
                input,
                adaptor,
                size,
                timescale: timescale(fps),
                next_frame: 0,
            })
        }

        fn push(&mut self, bgra: &[u8]) -> Result<(), ExportError> {
            let Size { width, height } = self.size;
            let row_bytes = width as usize * 4;
            if bgra.len() != row_bytes * height as usize {
                return Err(write_error("a frame's pixels don't match the export's size"));
            }
            self.wait_until_ready()?;
            let buffer = new_pixel_buffer(width, height)?;
            copy_into(&buffer, bgra, row_bytes, height as usize)?;
            // Frame n is shown at n / fps: its value counts milliframes against the timescale.
            let time = unsafe { CMTime::new(self.next_frame * 1000, self.timescale) };
            let appended = unsafe { self.adaptor.appendPixelBuffer_withPresentationTime(&buffer, time) };
            if !appended {
                return Err(self.failure("a frame couldn't be appended"));
            }
            self.next_frame += 1;
            Ok(())
        }

        fn finish(self) -> Result<(), ExportError> {
            unsafe { self.input.markAsFinished() };
            let (done, finished) = mpsc::channel::<()>();
            let block = RcBlock::new(move || {
                let _ = done.send(());
            });
            unsafe { self.writer.finishWritingWithCompletionHandler(&block) };
            finished.recv().map_err(|_| write_error("the writer never finished"))?;
            if unsafe { self.writer.status() } != AVAssetWriterStatus::Completed {
                return Err(self.failure("the file wasn't written"));
            }
            Ok(())
        }

        fn abandon(self) {
            unsafe { self.writer.cancelWriting() };
        }
    }

    impl AvFoundationEncoder {
        /// Waits for the input to take another frame, failing if the writer does first.
        fn wait_until_ready(&self) -> Result<(), ExportError> {
            while !unsafe { self.input.isReadyForMoreMediaData() } {
                if unsafe { self.writer.status() } == AVAssetWriterStatus::Failed {
                    return Err(self.failure("the writer stopped"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        }

        fn failure(&self, what: &str) -> ExportError {
            let detail = unsafe { self.writer.error() }.map(|e| ns_error(&e)).unwrap_or_default();
            write_error(format!("{what}: {detail}"))
        }
    }

    fn new_pixel_buffer(width: u32, height: u32) -> Result<CFRetained<CVPixelBuffer>, ExportError> {
        let mut buffer = ptr::null_mut();
        // SAFETY: no allocator or attributes are passed, and the output is writable.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                width as usize,
                height as usize,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::from(&mut buffer),
            )
        };
        if status != kCVReturnSuccess {
            return Err(write_error(format!("a pixel buffer wasn't created: status {status}")));
        }
        let buffer = NonNull::new(buffer).ok_or_else(|| write_error("a pixel buffer wasn't created"))?;
        // SAFETY: the successful call hands over one retained reference.
        Ok(unsafe { CFRetained::from_raw(buffer) })
    }

    /// Copies packed rows into a buffer, row by row, since the buffer may pad its rows.
    fn copy_into(buffer: &CVPixelBuffer, bgra: &[u8], row_bytes: usize, rows: usize) -> Result<(), ExportError> {
        // SAFETY: the buffer is locked for writing until it is unlocked below, and
        // every row written is inside its base address and bytes-per-row.
        unsafe {
            if CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags(0)) != kCVReturnSuccess {
                return Err(write_error("a pixel buffer couldn't be locked"));
            }
            let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(buffer);
            let usable = !base.is_null() && stride >= row_bytes;
            if usable {
                for (row, source) in bgra.chunks_exact(row_bytes).take(rows).enumerate() {
                    std::ptr::copy_nonoverlapping(source.as_ptr(), base.add(row * stride), row_bytes);
                }
            }
            CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags(0));
            if usable { Ok(()) } else { Err(write_error("a pixel buffer has no usable memory")) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrates_follow_mediabunny() {
        // 1080p medium: 3 Mbit/s x 0.3 x e^(2.5538 x 0.5), rounded up to the kilobit.
        assert_eq!(average_bitrate(Quality::Medium, Size::new(1920, 1080)), 3_227_000);
        assert_eq!(average_bitrate(Quality::High, Size::new(1920, 1080)), 6_111_000);
        assert_eq!(average_bitrate(Quality::VeryHigh, Size::new(1920, 1080)), 11_571_000);
    }

    #[test]
    fn bitrates_scale_with_pixels_not_frame_rate() {
        let hd = average_bitrate(Quality::High, Size::new(1280, 720));
        let full = average_bitrate(Quality::High, Size::new(1920, 1080));
        assert!(hd < full, "{hd} >= {full}");
        let ratio = full as f64 / hd as f64;
        let expected = (1920.0_f64 * 1080.0 / (1280.0 * 720.0)).powf(0.95);
        assert!((ratio - expected).abs() < 0.01, "{ratio}");
    }

    /// Finds the `colr` box in an MP4 and returns its `nclx` primaries, transfer and matrix.
    fn colour_tags(bytes: &[u8]) -> Option<(u16, u16, u16)> {
        let at = bytes.windows(4).position(|w| w == b"colr")? + 4;
        if bytes.get(at..at + 4)? != b"nclx" {
            return None;
        }
        let value = |i: usize| Some(u16::from_be_bytes([*bytes.get(at + 4 + 2 * i)?, *bytes.get(at + 5 + 2 * i)?]));
        Some((value(0)?, value(1)?, value(2)?))
    }

    #[test]
    fn mp4_is_tagged_srgb() {
        let path = std::env::temp_dir().join(format!("export-colour-{}.mp4", std::process::id()));
        let size = Size::new(64, 64);
        let mut encoder = AvFoundationEncoder::open(&path, Format::Mp4, Quality::Medium, size, 30.0)
            .expect("the encoder opens");
        for _ in 0..3 {
            encoder.push(&vec![0u8; 64 * 64 * 4]).expect("a frame is pushed");
        }
        encoder.finish().expect("the file is written");
        let bytes = std::fs::read(&path).expect("the file is readable");
        std::fs::remove_file(&path).ok();
        // ISO/IEC 23001-8 code points: 1 is BT.709 (primaries, matrix), 13 the sRGB transfer.
        assert_eq!(colour_tags(&bytes), Some((1, 13, 1)));
    }
}
