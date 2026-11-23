//! Video decoding on the real device: a generated video's frames come out at
//! the times asked for, loop and hold as their playback says, and import
//! records its metadata.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ffmpeg::codec::Id;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::util::rational::Rational;
use ffmpeg_next as ffmpeg;
use runtime::{MediaLibrary, MediaSource, MediaStatus, SystemVideoProbe};
use shader_graph::assets::{
    Asset, AssetKind, ImportFile, VideoEnd, VideoPlayback, asset_hash, describe_asset,
};
use shader_graph::passes::Size;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const FPS: f64 = 4.0;
const FPS_RATIONAL: i32 = 4;
/// The colours of the generated video, two frames each: its frame k is `COLOURS[k / 2]`, shown at k / FPS.
/// BGRA, as the encoder takes them.
const COLOURS: [[u8; 4]; 4] = [
    [0, 0, 255, 255],
    [0, 255, 0, 255],
    [255, 0, 0, 255],
    [255, 255, 255, 255],
];
const FRAMES: usize = 8;
const TOLERANCE: i32 = 12;
const TIMEOUT: Duration = Duration::from_secs(20);

/// Encodes the test video with FFmpeg's MPEG-4 encoder in an MP4, one solid colour per two frames.
fn generated_video() -> Vec<u8> {
    encoded_video(Id::MPEG4, Pixel::YUV420P, "mp4")
}

/// The test video as PNG frames with alpha in a QuickTime file.
fn generated_png_mov() -> Vec<u8> {
    encoded_video(Id::PNG, Pixel::RGBA, "mov")
}

fn encoded_video(codec: Id, pixel: Pixel, extension: &str) -> Vec<u8> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let name = format!(
        "runtime-video-test-{}-{}.{extension}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::env::temp_dir().join(name);
    write_video(&path, codec, pixel);
    let bytes = std::fs::read(&path).expect("reads the written video");
    let _ = std::fs::remove_file(&path);
    bytes
}

fn write_video(path: &std::path::Path, id: Id, pixel: Pixel) {
    ffmpeg::init().expect("FFmpeg starts");
    let mut output = ffmpeg::format::output(&path).expect("opens the output");
    let codec = ffmpeg::encoder::find(id).expect("FFmpeg has the encoder");
    let encoder_time_base = Rational::new(1, FPS_RATIONAL);
    let global_header = output
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let mut stream = output.add_stream(codec).expect("adds a stream");
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .expect("a video encoder");
    encoder.set_width(WIDTH);
    encoder.set_height(HEIGHT);
    encoder.set_format(pixel);
    encoder.set_time_base(encoder_time_base);
    encoder.set_frame_rate(Some(Rational::new(FPS_RATIONAL, 1)));
    if global_header {
        encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    let mut encoder = encoder.open_as(codec).expect("opens the encoder");
    stream.set_parameters(&encoder);
    output.write_header().expect("writes the header");
    let stream_time_base = output.stream(0).expect("the stream").time_base();

    let mut scaler = Scaler::get(
        Pixel::BGRA,
        WIDTH,
        HEIGHT,
        pixel,
        WIDTH,
        HEIGHT,
        Flags::BILINEAR,
    )
    .expect("converts to the encoder's format");
    for index in 0..FRAMES {
        let colour = COLOURS[index / 2];
        let mut bgra = ffmpeg::util::frame::Video::new(Pixel::BGRA, WIDTH, HEIGHT);
        let stride = bgra.stride(0);
        for row in bgra.data_mut(0).chunks_exact_mut(stride) {
            for pixel in row[..WIDTH as usize * 4].as_chunks_mut::<4>().0.iter_mut() {
                pixel.copy_from_slice(&colour);
            }
        }
        let mut yuv = ffmpeg::util::frame::Video::empty();
        scaler.run(&bgra, &mut yuv).expect("converts the frame");
        yuv.set_pts(Some(index as i64));
        encoder.send_frame(&yuv).expect("sends the frame");
        write_packets(
            &mut encoder,
            &mut output,
            encoder_time_base,
            stream_time_base,
        );
    }
    encoder.send_eof().expect("ends the video");
    write_packets(
        &mut encoder,
        &mut output,
        encoder_time_base,
        stream_time_base,
    );
    output.write_trailer().expect("writes the trailer");
}

fn write_packets(
    encoder: &mut ffmpeg::encoder::Video,
    output: &mut ffmpeg::format::context::Output,
    from: Rational,
    to: Rational,
) {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        // Muxers drop packets without a duration, the last frame's included.
        packet.set_duration(1);
        packet.rescale_ts(from, to);
        packet.write_interleaved(output).expect("writes a packet");
    }
}

fn source(bytes: &[u8]) -> MediaSource {
    MediaSource {
        kind: AssetKind::Video,
        hash: asset_hash(bytes),
        bytes: Arc::new(bytes.to_vec()),
    }
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("a GPU adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .expect("a device")
}

/// The BGRA pixel at the centre of a video texture.
fn centre_pixel(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> [u8; 4] {
    let row = WIDTH * 4;
    assert_eq!(
        row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "the test size keeps rows aligned"
    );
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("centre"),
        size: u64::from(row * HEIGHT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the copy finishes");
    receiver.recv().expect("the buffer maps").expect("maps");
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .expect("the mapped range");
    let at = ((HEIGHT / 2) * row + (WIDTH / 2) * 4) as usize;
    let pixel = [data[at], data[at + 1], data[at + 2], data[at + 3]];
    drop(data);
    buffer.unmap();
    pixel
}

fn close(pixel: [u8; 4], colour: [u8; 4]) -> bool {
    pixel
        .iter()
        .zip(colour)
        .all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() <= TOLERANCE)
}

/// The library and the device it draws on, with a wake-up for each decoded frame.
struct Harness {
    library: MediaLibrary,
    woken: mpsc::Receiver<()>,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Harness {
    fn new() -> Self {
        let (device, queue) = device();
        let (wake, woken) = mpsc::channel();
        let mut library = MediaLibrary::new(&device, &queue);
        library.set_waker(move || {
            let _ = wake.send(());
        });
        Self {
            library,
            woken,
            device,
            queue,
        }
    }

    /// Polls the library until its clip shows `colour` at project time `time`, waiting for decoded frames.
    fn shows(&mut self, playback: &VideoPlayback, view: &str, time: f64, colour: [u8; 4]) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.library.poll();
            let shown = self.library.video_texture("clip", view, playback, time);
            if shown.is_some_and(|texture| {
                close(centre_pixel(&self.device, &self.queue, &texture), colour)
            }) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            self.woken
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("the frame at {time}s never showed {colour:?}"));
        }
    }
}

#[test]
fn a_video_shows_the_frame_for_each_time_and_loops() {
    let bytes = generated_video();
    let mut harness = Harness::new();
    harness.library.sync(&std::collections::BTreeMap::from([(
        "clip".to_owned(),
        source(&bytes),
    )]));
    let deadline = Instant::now() + TIMEOUT;
    while harness.library.status("clip") == MediaStatus::Loading {
        harness.library.poll();
        harness
            .woken
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("the video opens");
    }
    assert_eq!(
        harness.library.status("clip"),
        MediaStatus::Ready(Size::new(WIDTH, HEIGHT))
    );

    let playback = VideoPlayback {
        start: 0.0,
        speed: 1.0,
        end: VideoEnd::Loop,
    };
    // Forward, frame by frame, then back to the start (a seek), then past the end (looping back to the start).
    for (time, colour) in [
        (0.1, COLOURS[0]),
        (0.6, COLOURS[1]),
        (1.1, COLOURS[2]),
        (1.6, COLOURS[3]),
    ] {
        harness.shows(&playback, "view", time, colour);
    }
    harness.shows(&playback, "view", 0.1, COLOURS[0]);
    harness.shows(&playback, "view", 2.1, COLOURS[0]);
    harness.shows(&playback, "view", 2.6, COLOURS[1]);
}

#[test]
fn a_png_video_in_a_mov_shows_its_frames() {
    let bytes = generated_png_mov();
    let mut harness = Harness::new();
    harness.library.sync(&std::collections::BTreeMap::from([(
        "clip".to_owned(),
        source(&bytes),
    )]));
    let playback = VideoPlayback {
        start: 0.0,
        speed: 1.0,
        end: VideoEnd::Loop,
    };
    for (time, colour) in [(0.1, COLOURS[0]), (1.1, COLOURS[2]), (1.6, COLOURS[3])] {
        harness.shows(&playback, "view", time, colour);
    }
}

#[test]
fn a_video_is_transparent_before_its_start_and_after_its_end_when_it_says_so() {
    let bytes = generated_video();
    let mut harness = Harness::new();
    harness.library.sync(&std::collections::BTreeMap::from([(
        "clip".to_owned(),
        source(&bytes),
    )]));
    let playback = VideoPlayback {
        start: 1.0,
        speed: 1.0,
        end: VideoEnd::Transparent,
    };
    harness.shows(&playback, "view", 1.1, COLOURS[0]);
    assert!(
        harness
            .library
            .video_texture("clip", "view", &playback, 0.5)
            .is_none(),
        "before its start"
    );
    assert!(
        harness
            .library
            .video_texture("clip", "view", &playback, 3.5)
            .is_none(),
        "past its end"
    );
}

#[test]
fn importing_a_video_records_its_size_length_and_frame_rate() {
    let bytes = generated_video();
    let file = ImportFile {
        name: "Clip.mp4",
        mime: "video/mp4",
        bytes: &bytes,
    };
    let Ok(Asset::Video(video)) = describe_asset("v1", file, &SystemVideoProbe) else {
        panic!("a video is described");
    };
    assert_eq!(video.file.name, "Clip");
    assert_eq!(video.file.file, "assets/videos/v1.mp4");
    assert_eq!((video.file.width, video.file.height), (WIDTH, HEIGHT));
    assert_eq!(video.file.hash, asset_hash(&bytes));
    assert!(
        (video.duration - FRAMES as f64 / FPS).abs() < 0.1,
        "{}",
        video.duration
    );
    assert!((video.frame_rate - FPS).abs() < 0.5, "{}", video.frame_rate);
}
