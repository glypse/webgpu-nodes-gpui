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

/// Bits per pixel per frame of each quality preset. These are our choices: the
/// original's `Quality` presets are opaque to us, so they need checking against
/// its output before being relied on.
pub fn bits_per_pixel(quality: Quality) -> f64 {
    match quality {
        Quality::Medium => 0.05,
        Quality::High => 0.1,
        Quality::VeryHigh => 0.2,
    }
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
        AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey, AVVideoHeightKey,
        AVVideoMaxKeyFrameIntervalKey, AVVideoWidthKey,
    };
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
        kCVPixelFormatType_32BGRA, kCVReturnSuccess,
    };
    use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL};

    use super::{VideoEncoder, bits_per_pixel, timescale};
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

            // Quality is set as a bitrate proportional to the frame's pixels per second, so every size gets the same quality.
            let bitrate = f64::from(size.width) * f64::from(size.height) * fps * bits_per_pixel(quality);
            let compression = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    key(unsafe { AVVideoAverageBitRateKey }, "average bitrate")?,
                    key(unsafe { AVVideoMaxKeyFrameIntervalKey }, "keyframe interval")?,
                ],
                &[
                    &*NSNumber::new_i64(bitrate as i64) as &AnyObject,
                    &*NSNumber::new_i64(fps.round().max(1.0) as i64) as &AnyObject,
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
                ],
                &[
                    codec as &AnyObject,
                    &*NSNumber::new_i64(i64::from(size.width)) as &AnyObject,
                    &*NSNumber::new_i64(i64::from(size.height)) as &AnyObject,
                    &*compression as &AnyObject,
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
