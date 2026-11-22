//! Encodes BGRA frames with FFmpeg's built-in encoders into a MOV or Matroska file.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use ffmpeg::codec::{self, Id};
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::util::color;
use ffmpeg::util::frame::Video as Frame;
use runtime::Size;

use crate::{Codec, Depth, DnxhrProfile, ExportError, Settings};

/// The pixel format a codec stores, and whether it is YUV (tagged BT.709) or RGB.
#[derive(Clone, Copy)]
struct Format {
    pixel: Pixel,
    yuv: bool,
}

fn format(settings: &Settings) -> Format {
    let alpha = settings.alpha;
    let (pixel, yuv) = match settings.codec {
        Codec::ProRes(profile) if profile.is_4444() => {
            (if alpha { Pixel::YUVA444P10LE } else { Pixel::YUV444P10LE }, true)
        }
        Codec::ProRes(_) => (Pixel::YUV422P10LE, true),
        Codec::Dnxhr(DnxhrProfile::Hqx) => (Pixel::YUV422P10LE, true),
        Codec::Dnxhr(DnxhrProfile::P444) => (Pixel::YUV444P10LE, true),
        Codec::Dnxhr(_) => (Pixel::YUV422P, true),
        Codec::Ffv1(depth) => match (depth, alpha) {
            // FFV1 in FFmpeg takes 8-bit RGB only packed.
            (Depth::Bits8, false) => (Pixel::BGRZ, false),
            (Depth::Bits8, true) => (Pixel::BGRA, false),
            (Depth::Bits16, false) => (Pixel::GBRP16LE, false),
            (Depth::Bits16, true) => (Pixel::GBRAP16LE, false),
        },
        Codec::Png => (if alpha { Pixel::RGBA } else { Pixel::RGB24 }, false),
        Codec::QuickTimeAnimation => (if alpha { Pixel::ARGB } else { Pixel::RGB24 }, false),
    };
    Format { pixel, yuv }
}

fn encoder_for(codec: Codec) -> Option<ffmpeg::Codec> {
    match codec {
        Codec::ProRes(_) => ffmpeg::encoder::find_by_name("prores_ks"),
        Codec::Dnxhr(_) => ffmpeg::encoder::find(Id::DNXHD),
        Codec::Ffv1(_) => ffmpeg::encoder::find(Id::FFV1),
        Codec::Png => ffmpeg::encoder::find(Id::PNG),
        Codec::QuickTimeAnimation => ffmpeg::encoder::find(Id::QTRLE),
    }
}

fn encoder_options(codec: Codec) -> ffmpeg::Dictionary<'static> {
    let mut options = ffmpeg::Dictionary::new();
    match codec {
        Codec::ProRes(profile) => options.set("profile", profile.ffmpeg_name()),
        Codec::Dnxhr(profile) => options.set("profile", profile.ffmpeg_name()),
        _ => {}
    }
    options
}

fn ff(error: ffmpeg::Error) -> ExportError {
    ExportError::Encode(error.to_string())
}

/// Writes frames of BGRA pixels, top row first, one frame apart.
pub struct Encoder {
    output: ffmpeg::format::context::Output,
    encoder: ffmpeg::encoder::Video,
    stream: usize,
    encoder_time_base: ffmpeg::Rational,
    stream_time_base: ffmpeg::Rational,
    scaler: Scaler,
    format: Format,
    size: Size,
    next_pts: i64,
}

impl Encoder {
    /// Opens `path` for frames of `size` (which must have even sides) at `settings.fps`.
    pub fn open(path: &Path, settings: &Settings, size: Size) -> Result<Self, ExportError> {
        ffmpeg::init().map_err(ff)?;
        let codec = encoder_for(settings.codec)
            .ok_or_else(|| ExportError::Encode(format!("FFmpeg has no {} encoder", settings.codec.label())))?;
        let format = format(settings);
        let mut output = ffmpeg::format::output_as(path, settings.container.muxer()).map_err(ff)?;
        let global_header = output.format().flags().contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

        let index = {
            let stream = output.add_stream(codec).map_err(ff)?;
            stream.index()
        };
        let mut context = codec::context::Context::new_with_codec(codec).encoder().video().map_err(ff)?;
        context.set_width(size.width);
        context.set_height(size.height);
        context.set_format(format.pixel);
        // Frame n is at n / fps: the time base is 1 / fps, with fractional rates kept exact in thousandths.
        let frame_rate = ffmpeg::Rational::new((settings.fps * 1000.0).round() as i32, 1000);
        context.set_frame_rate(Some(frame_rate));
        context.set_time_base(ffmpeg::Rational::new(1000, (settings.fps * 1000.0).round() as i32));
        if global_header {
            context.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        tag_colour(&mut context, format.yuv);

        let encoder = context.open_as_with(codec, encoder_options(settings.codec)).map_err(ff)?;
        if let Some(mut stream) = output.stream_mut(index) {
            stream.set_parameters(&encoder);
        }
        output.write_header().map_err(ff)?;
        let stream_time_base = output
            .stream(index)
            .map(|stream| stream.time_base())
            .ok_or_else(|| ExportError::Encode("the file has no video stream".into()))?;

        let mut scaler = Scaler::get(
            Pixel::BGRA,
            size.width,
            size.height,
            format.pixel,
            size.width,
            size.height,
            Flags::BILINEAR,
        )
        .map_err(ff)?;
        if format.yuv {
            use ffmpeg::ffi::{SWS_CS_DEFAULT, SWS_CS_ITU709, sws_getCoefficients, sws_setColorspaceDetails};
            // SAFETY: the scaler is valid for the call, and the coefficient tables are static FFmpeg data.
            let applied = unsafe {
                sws_setColorspaceDetails(
                    scaler.as_mut_ptr(),
                    sws_getCoefficients(SWS_CS_DEFAULT),
                    1,
                    sws_getCoefficients(SWS_CS_ITU709),
                    0,
                    0,
                    1 << 16,
                    1 << 16,
                )
            };
            if applied < 0 {
                return Err(ExportError::Encode("the scaler can't convert to BT.709".into()));
            }
        }

        Ok(Self {
            output,
            encoder,
            stream: index,
            encoder_time_base: ffmpeg::Rational::new(1000, (settings.fps * 1000.0).round() as i32),
            stream_time_base,
            scaler,
            format,
            size,
            next_pts: 0,
        })
    }

    pub fn push(&mut self, bgra: &[u8]) -> Result<(), ExportError> {
        let Size { width, height } = self.size;
        let row_bytes = width as usize * 4;
        if bgra.len() != row_bytes * height as usize {
            return Err(ExportError::Encode("a frame's pixels don't match the export's size".into()));
        }
        let mut source = Frame::new(Pixel::BGRA, width, height);
        let stride = source.stride(0);
        for (line, destination) in bgra.chunks_exact(row_bytes).zip(source.data_mut(0).chunks_mut(stride)) {
            if let Some(destination) = destination.get_mut(..row_bytes) {
                destination.copy_from_slice(line);
            }
        }
        // A new frame each time: the encoder may still hold the last one.
        let mut converted = Frame::new(self.format.pixel, width, height);
        self.scaler.run(&source, &mut converted).map_err(ff)?;
        converted.set_pts(Some(self.next_pts));
        self.next_pts += 1;
        self.encoder.send_frame(&converted).map_err(ff)?;
        self.receive()
    }

    /// Flushes the encoder and writes the file's trailer. A dropped encoder leaves an unfinished file.
    pub fn finish(mut self) -> Result<(), ExportError> {
        self.encoder.send_eof().map_err(ff)?;
        self.receive()?;
        self.output.write_trailer().map_err(ff)
    }

    fn receive(&mut self) -> Result<(), ExportError> {
        let mut packet = ffmpeg::Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(self.stream);
                    // The MOV muxer drops a final packet that has no duration.
                    packet.set_duration(1);
                    packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
                    packet.write_interleaved(&mut self.output).map_err(ff)?;
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
                Err(ffmpeg::Error::Eof) => return Ok(()),
                Err(error) => return Err(ff(error)),
            }
        }
    }
}

/// Frames are sRGB: BT.709 primaries with the sRGB transfer. YUV uses
/// the BT.709 matrix and limited range; RGB has no matrix and full range.
fn tag_colour(context: &mut codec::encoder::video::Video, yuv: bool) {
    if yuv {
        context.set_colorspace(color::Space::BT709);
        context.set_color_range(color::Range::MPEG);
    } else {
        context.set_colorspace(color::Space::RGB);
        context.set_color_range(color::Range::JPEG);
    }
    context.set_color_primaries(color::Primaries::BT709);
    context.set_color_transfer_characteristic(color::TransferCharacteristic::IEC61966_2_1);
}
