//! Video decoding with FFmpeg, built statically by `ffmpeg-next`: the video
//! stream's frames, converted to BGRA by swscale and turned upright as the
//! stream's display matrix says.

use std::path::Path;

use ffmpeg::format::Pixel;
use ffmpeg::format::context::Input;
use ffmpeg::format::stream::Stream;
use ffmpeg::packet::side_data::Type as SideDataType;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next as ffmpeg;
use shader_graph::assets::VideoInfo;

use super::MediaError;
use super::video::{DecodedFrame, VideoDecoder};
use crate::Size;

/// The spacing assumed for a frame when the video's frame rate isn't known.
const FALLBACK_FRAME_SECONDS: f64 = 1.0 / 30.0;

/// Decodes the best video stream of a file, reading frames in presentation order.
pub struct FfmpegDecoder {
    input: Input,
    stream: usize,
    time_base: f64,
    decoder: ffmpeg::decoder::Video,
    scaler: Option<ScalerState>,
    /// Clockwise quarter turns, in degrees, that the frames are shown with.
    turn: u32,
    info: VideoInfo,
    /// Set once the decoder has been told the input ended, so it drains its last frames.
    flushing: bool,
    /// Where the next frame is expected when the container gives it no timestamp.
    next_time: f64,
}

struct ScalerState {
    context: Scaler,
    source: (Pixel, u32, u32),
}

impl FfmpegDecoder {
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        ffmpeg::init().map_err(ff)?;
        let input = ffmpeg::format::input(path).map_err(ff)?;
        let (stream, time_base, turn, duration, frame_rate, parameters) = {
            let stream = input
                .streams()
                .best(ffmpeg::media::Type::Video)
                .ok_or_else(|| video_error("the file has no video track"))?;
            let time_base = f64::from(stream.time_base());
            let duration = duration_seconds(&input, &stream, time_base)
                .ok_or_else(|| video_error("the file's duration isn't known"))?;
            (
                stream.index(),
                time_base,
                clockwise_degrees(&stream),
                duration,
                frame_rate(&stream),
                stream.parameters(),
            )
        };
        let decoder = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(ff)?
            .decoder()
            .video()
            .map_err(ff)?;
        let (width, height) = (decoder.width(), decoder.height());
        let (width, height) = if turn_swaps_sides(turn) {
            (height, width)
        } else {
            (width, height)
        };
        let info = VideoInfo {
            width,
            height,
            duration,
            frame_rate,
        };
        if width == 0 || height == 0 {
            return Err(video_error("the video's size isn't one a frame can have"));
        }
        Ok(Self {
            input,
            stream,
            time_base,
            decoder,
            scaler: None,
            turn,
            info,
            flushing: false,
            next_time: 0.0,
        })
    }

    /// The next packet of the video stream, or `None` at the end of the file.
    fn next_packet(&mut self) -> Result<Option<ffmpeg::Packet>, MediaError> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => return Ok(Some(packet)),
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(error) => return Err(ff(error)),
            }
        }
    }

    fn convert(&mut self, frame: &ffmpeg::util::frame::Video) -> Result<DecodedFrame, MediaError> {
        let (width, height) = (frame.width(), frame.height());
        let source = (frame.format(), width, height);
        let mut scaler = match self.scaler.take() {
            Some(state) if state.source == source => state,
            _ => ScalerState {
                context: Scaler::get(
                    source.0,
                    width,
                    height,
                    Pixel::BGRA,
                    width,
                    height,
                    Flags::BILINEAR,
                )
                .map_err(ff)?,
                source,
            },
        };
        let mut bgra = ffmpeg::util::frame::Video::empty();
        let converted = scaler.context.run(frame, &mut bgra);
        self.scaler = Some(scaler);
        converted.map_err(ff)?;
        let (bgra, size) = turned_pixels(
            bgra.data(0),
            bgra.stride(0),
            Size::new(width, height),
            self.turn,
        )?;

        let time = match frame.timestamp() {
            Some(timestamp) => timestamp as f64 * self.time_base,
            None => self.next_time,
        };
        let duration = if self.info.frame_rate > 0.0 {
            1.0 / self.info.frame_rate
        } else {
            FALLBACK_FRAME_SECONDS
        };
        self.next_time = time + duration;
        Ok(DecodedFrame {
            time,
            duration,
            size,
            bgra,
        })
    }
}

impl VideoDecoder for FfmpegDecoder {
    fn info(&self) -> VideoInfo {
        self.info
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        let target = (time.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        self.input.seek(target, ..target).map_err(ff)?;
        self.decoder.flush();
        self.flushing = false;
        self.next_time = time;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, MediaError> {
        loop {
            let mut frame = ffmpeg::util::frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => return self.convert(&frame).map(Some),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
                Err(error) => return Err(ff(error)),
            }
            if self.flushing {
                return Ok(None);
            }
            match self.next_packet()? {
                Some(packet) => self.decoder.send_packet(&packet).map_err(ff)?,
                None => {
                    self.decoder.send_eof().map_err(ff)?;
                    self.flushing = true;
                }
            }
        }
    }
}

fn ff(error: ffmpeg::Error) -> MediaError {
    MediaError::Video(error.to_string())
}

fn video_error(message: impl Into<String>) -> MediaError {
    MediaError::Video(message.into())
}

fn duration_seconds(input: &Input, stream: &Stream, time_base: f64) -> Option<f64> {
    let container = input.duration();
    if container != ffmpeg::ffi::AV_NOPTS_VALUE && container > 0 {
        return Some(container as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE));
    }
    let own = stream.duration();
    (own != ffmpeg::ffi::AV_NOPTS_VALUE && own > 0).then_some(own as f64 * time_base)
}

/// The stream's average frame rate, else its nominal one; zero when neither is known.
fn frame_rate(stream: &Stream) -> f64 {
    [stream.avg_frame_rate(), stream.rate()]
        .into_iter()
        .map(f64::from)
        .find(|rate| rate.is_finite() && *rate > 0.0)
        .unwrap_or(0.0)
}

/// The stream's display rotation as clockwise degrees, in quarter turns.
/// The sign follows FFmpeg's own command line, which turns frames the same way.
fn clockwise_degrees(stream: &Stream) -> u32 {
    let Some(matrix) = stream
        .side_data()
        .find(|data| data.kind() == SideDataType::DisplayMatrix)
    else {
        return 0;
    };
    let mut values = [0i32; 9];
    for (value, bytes) in values.iter_mut().zip(matrix.data().as_chunks::<4>().0) {
        *value = i32::from_ne_bytes(*bytes);
    }
    // SAFETY: `values` holds the nine 32-bit entries of a display matrix, which the function reads.
    let counter_clockwise = unsafe { ffmpeg::ffi::av_display_rotation_get(values.as_ptr()) };
    let quarters = (-counter_clockwise / 90.0).round() as i64;
    quarters.rem_euclid(4) as u32 * 90
}

fn turn_swaps_sides(degrees: u32) -> bool {
    degrees == 90 || degrees == 270
}

/// A frame's BGRA pixels (`data` with rows `stride` bytes apart, `size` of
/// them) turned `degrees` clockwise, a multiple of 90, into one tight buffer,
/// rows top first.
fn turned_pixels(
    data: &[u8],
    stride: usize,
    size: Size,
    degrees: u32,
) -> Result<(Vec<u8>, Size), MediaError> {
    let (w, h) = (size.width as usize, size.height as usize);
    let row = w * 4;
    if stride < row || data.len() < stride * h.saturating_sub(1) + row {
        return Err(video_error("a frame has no usable pixels"));
    }
    if degrees.is_multiple_of(360) {
        let mut out = Vec::with_capacity(row * h);
        for line in data.chunks(stride).take(h) {
            if let Some(pixels) = line.get(..row) {
                out.extend_from_slice(pixels);
            }
        }
        return Ok((out, size));
    }
    let (out_w, out_h) = if turn_swaps_sides(degrees) {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; row * h];
    for (index, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (ox, oy) = (index % out_w, index / out_w);
        let (sx, sy) = match degrees % 360 {
            90 => (oy, h - 1 - ox),
            180 => (w - 1 - ox, h - 1 - oy),
            _ => (w - 1 - oy, ox),
        };
        if let Some(source) = data.get(sy * stride + sx * 4..sy * stride + sx * 4 + 4) {
            pixel.copy_from_slice(source);
        }
    }
    Ok((out, Size::new(out_w as u32, out_h as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_pixels_clockwise() {
        // A 2 wide, 3 tall image whose pixels are numbered in reading order.
        let pixels: Vec<u8> = (0..6u8).flat_map(|n| [n, n, n, 255]).collect();
        let size = Size::new(2, 3);
        let column = |bgra: &[u8]| {
            bgra.as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>()
        };

        let (quarter, quarter_size) = turned_pixels(&pixels, 8, size, 90).expect("turns");
        assert_eq!((quarter_size.width, quarter_size.height), (3, 2));
        assert_eq!(column(&quarter), vec![4, 2, 0, 5, 3, 1]);

        let (half, half_size) = turned_pixels(&pixels, 8, size, 180).expect("turns");
        assert_eq!((half_size.width, half_size.height), (2, 3));
        assert_eq!(column(&half), vec![5, 4, 3, 2, 1, 0]);

        let (three_quarters, _) = turned_pixels(&pixels, 8, size, 270).expect("turns");
        assert_eq!(column(&three_quarters), vec![1, 3, 5, 0, 2, 4]);

        let (unturned, unturned_size) = turned_pixels(&pixels, 8, size, 0).expect("turns");
        assert_eq!((unturned_size.width, unturned_size.height), (2, 3));
        assert_eq!(unturned, pixels);
    }

    #[test]
    fn reads_rows_padded_to_a_stride() {
        // Each 2-pixel row is followed by 4 bytes of padding that must not reach the output.
        let padded: Vec<u8> = (0..3u8)
            .flat_map(|row| {
                (0..2u8)
                    .flat_map(move |x| [row * 2 + x, 0, 0, 255])
                    .chain([9, 9, 9, 9])
            })
            .collect();
        let size = Size::new(2, 3);
        let (tight, _) = turned_pixels(&padded, 12, size, 0).expect("turns");
        assert_eq!(tight.len(), 24);
        assert!(
            tight
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .all(|(i, p)| p[0] == i as u8)
        );
        let (quarter, _) = turned_pixels(&padded, 12, size, 90).expect("turns");
        assert_eq!(
            quarter
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            vec![4, 2, 0, 5, 3, 1]
        );
    }
}
