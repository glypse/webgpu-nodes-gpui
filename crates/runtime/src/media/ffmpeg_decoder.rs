//! Video decoding with FFmpeg, built statically by `ffmpeg-next`: the video
//! stream's frames, converted to BGRA by swscale and turned upright as the
//! stream's display matrix says.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::format::context::Input;
use ffmpeg::format::stream::Stream;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::packet::side_data::Type as SideDataType;
use shader_graph::assets::VideoInfo;

use super::video::{DecodedFrame, VideoDecoder};
use super::MediaError;
use crate::Size;

/// Packets read to estimate a video's frame rate, as the original's importer does.
const PACKETS_FOR_FRAME_RATE: usize = 600;
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
        let (stream, time_base, turn, duration) = {
            let stream = input.streams().best(ffmpeg::media::Type::Video).ok_or_else(|| video_error("the file has no video track"))?;
            let time_base = f64::from(stream.time_base());
            let duration = duration_seconds(&input, &stream, time_base).ok_or_else(|| video_error("the file's duration isn't known"))?;
            (stream.index(), time_base, clockwise_degrees(&stream), duration)
        };
        let frame_rate = frame_rate(path, stream)?;
        let decoder = {
            let input = ffmpeg::format::input(path).map_err(ff)?;
            let stream = input.stream(stream).ok_or_else(|| video_error("the video track is gone"))?;
            ffmpeg::codec::context::Context::from_parameters(stream.parameters()).map_err(ff)?.decoder().video().map_err(ff)?
        };
        let (width, height) = (decoder.width(), decoder.height());
        let (width, height) = if turn_swaps_sides(turn) { (height, width) } else { (width, height) };
        let info = VideoInfo { width, height, duration, frame_rate };
        if width == 0 || height == 0 {
            return Err(video_error("the video's size isn't one a frame can have"));
        }
        Ok(Self { input, stream, time_base, decoder, scaler: None, turn, info, flushing: false, next_time: 0.0 })
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
                context: Scaler::get(source.0, width, height, Pixel::BGRA, width, height, Flags::BILINEAR).map_err(ff)?,
                source,
            },
        };
        let mut bgra = ffmpeg::util::frame::Video::empty();
        let converted = scaler.context.run(frame, &mut bgra);
        self.scaler = Some(scaler);
        converted.map_err(ff)?;
        let tight = tight_rows(&bgra, Size::new(width, height))?;
        let (bgra, size) = turn_clockwise(tight, Size::new(width, height), self.turn);

        let time = match frame.timestamp() {
            Some(timestamp) => timestamp as f64 * self.time_base,
            None => self.next_time,
        };
        let duration = if self.info.frame_rate > 0.0 { 1.0 / self.info.frame_rate } else { FALLBACK_FRAME_SECONDS };
        self.next_time = time + duration;
        Ok(DecodedFrame { time, duration, size, bgra })
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

/// The average frame rate of the stream's first packets: their count over
/// their span. Zero when there are too few to tell.
fn frame_rate(path: &Path, stream: usize) -> Result<f64, MediaError> {
    let mut input = ffmpeg::format::input(path).map_err(ff)?;
    let time_base = input.stream(stream).map(|s| f64::from(s.time_base())).ok_or_else(|| video_error("the video track is gone"))?;
    let (mut first, mut end, mut count, mut read) = (f64::INFINITY, f64::NEG_INFINITY, 0usize, 0usize);
    while read < PACKETS_FOR_FRAME_RATE {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => {
                log::warn!("[media] the video ends early: {error}");
                break;
            }
        }
        if packet.stream() != stream {
            continue;
        }
        read += 1;
        // Containers can carry empty packets (priming, an end marker) that aren't frames.
        if let Some(pts) = packet.pts().filter(|_| packet.duration() > 0) {
            let start = pts as f64 * time_base;
            first = first.min(start);
            end = end.max(start + packet.duration() as f64 * time_base);
            count += 1;
        }
    }
    let span = end - first;
    Ok(if count < 2 || span.is_nan() || span <= 0.0 { 0.0 } else { count as f64 / span })
}

/// The stream's display rotation as clockwise degrees, in quarter turns.
/// The sign follows FFmpeg's own command line, which turns frames the same way.
fn clockwise_degrees(stream: &Stream) -> u32 {
    let Some(matrix) = stream.side_data().find(|data| data.kind() == SideDataType::DisplayMatrix) else {
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

/// The frame's pixels as BGRA, rows tight and top first; the frame may pad its rows.
fn tight_rows(frame: &ffmpeg::util::frame::Video, size: Size) -> Result<Vec<u8>, MediaError> {
    let row = size.width as usize * 4;
    let stride = frame.stride(0);
    let data = frame.data(0);
    if stride < row || data.len() < stride * (size.height as usize).saturating_sub(1) + row {
        return Err(video_error("a frame has no usable pixels"));
    }
    let mut out = Vec::with_capacity(row * size.height as usize);
    for y in 0..size.height as usize {
        out.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Ok(out)
}

/// `bgra` (of `size`) turned `degrees` clockwise, a multiple of 90.
fn turn_clockwise(bgra: Vec<u8>, size: Size, degrees: u32) -> (Vec<u8>, Size) {
    if degrees.is_multiple_of(360) {
        return (bgra, size);
    }
    let (w, h) = (size.width as usize, size.height as usize);
    let (out_w, out_h) = if turn_swaps_sides(degrees) { (h, w) } else { (w, h) };
    let mut out = vec![0u8; bgra.len()];
    for oy in 0..out_h {
        for ox in 0..out_w {
            let (sx, sy) = match degrees % 360 {
                90 => (oy, h - 1 - ox),
                180 => (w - 1 - ox, h - 1 - oy),
                _ => (w - 1 - oy, ox),
            };
            let from = (sy * w + sx) * 4;
            let to = (oy * out_w + ox) * 4;
            if let (Some(source), Some(target)) = (bgra.get(from..from + 4), out.get_mut(to..to + 4)) {
                target.copy_from_slice(source);
            }
        }
    }
    (out, Size::new(out_w as u32, out_h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_pixels_clockwise() {
        // A 2 wide, 3 tall image whose pixels are numbered in reading order.
        let pixels: Vec<u8> = (0..6u8).flat_map(|n| [n, n, n, 255]).collect();
        let size = Size::new(2, 3);
        let column = |bgra: &[u8]| bgra.as_chunks::<4>().0.iter().map(|p| p[0]).collect::<Vec<_>>();

        let (quarter, quarter_size) = turn_clockwise(pixels.clone(), size, 90);
        assert_eq!((quarter_size.width, quarter_size.height), (3, 2));
        assert_eq!(column(&quarter), vec![4, 2, 0, 5, 3, 1]);

        let (half, half_size) = turn_clockwise(pixels.clone(), size, 180);
        assert_eq!((half_size.width, half_size.height), (2, 3));
        assert_eq!(column(&half), vec![5, 4, 3, 2, 1, 0]);

        let (three_quarters, _) = turn_clockwise(pixels.clone(), size, 270);
        assert_eq!(column(&three_quarters), vec![1, 3, 5, 0, 2, 4]);

        let (unturned, unturned_size) = turn_clockwise(pixels.clone(), size, 0);
        assert_eq!((unturned_size.width, unturned_size.height), (2, 3));
        assert_eq!(unturned, pixels);
    }
}
