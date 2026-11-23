//! Renders a small graph on the real GPU and encodes it, then reads each file
//! back with FFmpeg: frame count, size, codec, pixel format and colour tags.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use export::{
    Codec, Container, Depth, DnxhrProfile, Export, ExportError, ProResProfile, Scene, Settings,
};
use ffmpeg::codec::Id;
use ffmpeg::format::Pixel;
use ffmpeg::util::color;
use ffmpeg_next as ffmpeg;
use runtime::Size;
use shader_graph::graph::{Connection, FRAG_OUTPUT, GraphModel, Point};
use shader_graph::types::{InputValue, WgslType};

fn add(model: &mut GraphModel, node_type: &str) -> String {
    model
        .add_node(node_type, Point::default())
        .unwrap_or_else(|| panic!("adds {node_type}"))
}

fn link(model: &mut GraphModel, from: (&str, &str), to: (&str, &str)) {
    let connection = Connection::new(from.0, from.1, to.0, to.1);
    assert!(
        model.is_valid_connection(&connection),
        "invalid link {connection:?}"
    );
    assert!(model.connect(connection));
}

/// A colour that changes with time: `time * 0.5` in every channel.
fn scene(size: Size) -> Scene {
    let mut model = GraphModel::default();
    let time = add(&mut model, "time");
    let half = add(&mut model, "declare");
    model.set_node_type(&half, Some(WgslType::F32));
    model.set_input_value(&half, "x", Some(InputValue::from(0.5)));
    let scaled = add(&mut model, "mul");
    link(&mut model, (&time, "seconds"), (&scaled, "operands"));
    link(&mut model, (&half, "value"), (&scaled, "operands"));
    let color = add(&mut model, "declare");
    model.set_node_type(&color, Some(WgslType::Vec4f));
    link(&mut model, (&scaled, "result"), (&color, "x"));
    let output = model
        .root()
        .nodes
        .iter()
        .find(|node| node.is(FRAG_OUTPUT))
        .expect("an output")
        .id
        .clone();
    link(&mut model, (&color, "value"), (&output, "color"));
    let program = model.generate_hoisted().expect("generates");
    Scene {
        program,
        media: BTreeMap::new(),
        size,
    }
}

fn output_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn settings(
    container: Container,
    codec: Codec,
    alpha: bool,
    duration: f64,
    fps: f64,
    start: f64,
) -> Settings {
    Settings {
        container,
        codec,
        alpha,
        start,
        duration,
        fps,
    }
}

/// What the file's first video stream and its frames say once decoded.
#[derive(Debug)]
struct Readback {
    codec: Id,
    width: u32,
    height: u32,
    pixel: Pixel,
    frames: usize,
    primaries: color::Primaries,
    transfer: color::TransferCharacteristic,
    space: color::Space,
    range: color::Range,
}

fn read_back(path: &PathBuf, name: &str) -> Readback {
    ffmpeg::init().expect("FFmpeg starts");
    let mut input = ffmpeg::format::input(path).expect("the file opens");
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .expect("a video stream");
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .decoder()
        .video()
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let codec = decoder.id();
    let primaries = decoder.color_primaries();
    let transfer = decoder.color_transfer_characteristic();
    let space = decoder.color_space();
    let range = decoder.color_range();

    let mut frames = 0;
    let mut first = None;
    let mut frame = ffmpeg::util::frame::Video::empty();
    let mut count = |frame: &ffmpeg::util::frame::Video| {
        frames += 1;
        first.get_or_insert((frame.width(), frame.height(), frame.format()));
    };
    for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).expect("a packet decodes");
        while decoder.receive_frame(&mut frame).is_ok() {
            count(&frame);
        }
    }
    decoder.send_eof().expect("the decoder flushes");
    while decoder.receive_frame(&mut frame).is_ok() {
        count(&frame);
    }
    let (width, height, pixel) = first.expect("at least one frame");
    Readback {
        codec,
        width,
        height,
        pixel,
        frames,
        primaries,
        transfer,
        space,
        range,
    }
}

fn is_yuv(pixel: Pixel) -> bool {
    !matches!(
        pixel,
        Pixel::BGRZ
            | Pixel::BGRA
            | Pixel::ARGB
            | Pixel::RGB24
            | Pixel::RGBA
            | Pixel::GBRP16LE
            | Pixel::GBRAP16LE
    )
}

fn encode(name: &str, settings: Settings, size: Size) -> Readback {
    let path = output_path(name);
    let _ = std::fs::remove_file(&path);
    let export = Export::start(scene(size), settings, path.clone()).expect("starts");
    let written = export
        .wait()
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(written, path);
    read_back(&path, name)
}

#[test]
fn every_codec_in_every_container_writes_its_frames_at_the_even_size() {
    // The canvas is 33 by 17, odd on both sides, so most codecs get 32 by 16. DNxHR needs
    // frames of at least 256 by 120, so its cases use a canvas of 257 by 121. A 1 second range at 30 fps is 30 frames.
    let small = Size::new(33, 17);
    let large = Size::new(257, 121);
    let cases = [
        (
            Codec::ProRes(ProResProfile::Proxy),
            false,
            Pixel::YUV422P10LE,
            Id::PRORES,
        ),
        (
            Codec::ProRes(ProResProfile::Standard),
            false,
            Pixel::YUV422P10LE,
            Id::PRORES,
        ),
        (
            Codec::ProRes(ProResProfile::Hq),
            false,
            Pixel::YUV422P10LE,
            Id::PRORES,
        ),
        // FFmpeg's prores_ks stores the 4444 profiles at 12 bits whatever the input depth.
        (
            Codec::ProRes(ProResProfile::P4444),
            false,
            Pixel::YUV444P12LE,
            Id::PRORES,
        ),
        (
            Codec::ProRes(ProResProfile::P4444Xq),
            true,
            Pixel::YUVA444P12LE,
            Id::PRORES,
        ),
        (
            Codec::Dnxhr(DnxhrProfile::Lb),
            false,
            Pixel::YUV422P,
            Id::DNXHD,
        ),
        (
            Codec::Dnxhr(DnxhrProfile::Hqx),
            false,
            Pixel::YUV422P10LE,
            Id::DNXHD,
        ),
        (
            Codec::Dnxhr(DnxhrProfile::P444),
            false,
            Pixel::YUV444P10LE,
            Id::DNXHD,
        ),
        (Codec::Ffv1(Depth::Bits8), false, Pixel::BGRZ, Id::FFV1),
        (Codec::Ffv1(Depth::Bits8), true, Pixel::BGRA, Id::FFV1),
        (Codec::Ffv1(Depth::Bits16), false, Pixel::GBRP16LE, Id::FFV1),
        (Codec::Png, false, Pixel::RGB24, Id::PNG),
        (Codec::Png, true, Pixel::RGBA, Id::PNG),
        (Codec::QuickTimeAnimation, false, Pixel::RGB24, Id::QTRLE),
        (Codec::QuickTimeAnimation, true, Pixel::ARGB, Id::QTRLE),
    ];
    for container in Container::ALL {
        for (codec, alpha, pixel, id) in cases {
            if !codec.supports_container(container) {
                continue;
            }
            let canvas = if matches!(codec, Codec::Dnxhr(_)) {
                large
            } else {
                small
            };
            let name = format!("{codec:?}-{alpha}-{container:?}.{}", container.extension());
            let readback = encode(
                &name,
                settings(container, codec, alpha, 1.0, 30.0, 0.5),
                canvas,
            );
            assert_eq!(
                readback.frames, 30,
                "{name}: one frame per frame of the range"
            );
            let expected = if canvas == large {
                (256, 120)
            } else {
                (32, 16)
            };
            assert_eq!((readback.width, readback.height), expected, "{name}");
            assert_eq!(readback.pixel, pixel, "{name}");
            assert_eq!(readback.codec, id, "{name}");
            assert_eq!(
                readback.primaries,
                color::Primaries::BT709,
                "{name}: sRGB is tagged BT.709"
            );
            assert_eq!(
                readback.transfer,
                color::TransferCharacteristic::IEC61966_2_1,
                "{name}: sRGB transfer"
            );
            if is_yuv(pixel) {
                assert_eq!(readback.space, color::Space::BT709, "{name}: YUV matrix");
                assert_eq!(readback.range, color::Range::MPEG, "{name}: limited range");
            } else {
                assert_eq!(
                    readback.space,
                    color::Space::RGB,
                    "{name}: RGB has no matrix"
                );
            }
        }
    }
}

#[test]
fn quicktime_animation_is_refused_in_matroska() {
    let path = output_path("qtrle.mkv");
    let refused = Export::start(
        scene(Size::new(32, 16)),
        settings(
            Container::Matroska,
            Codec::QuickTimeAnimation,
            false,
            1.0,
            30.0,
            0.0,
        ),
        path.clone(),
    );
    assert!(matches!(refused, Err(ExportError::Invalid(_))));
    assert!(!path.exists());
}

#[test]
fn dnxhr_refuses_frames_below_its_minimum_size() {
    let path = output_path("too-small.mov");
    let refused = Export::start(
        scene(Size::new(32, 16)),
        settings(
            Container::Mov,
            Codec::Dnxhr(DnxhrProfile::Sq),
            false,
            1.0,
            30.0,
            0.0,
        ),
        path.clone(),
    );
    assert!(matches!(refused, Err(ExportError::Invalid(_))));
    assert!(!path.exists());
}

#[test]
fn progress_counts_the_frames_and_cancel_removes_the_file() {
    let path = output_path("cancelled.mov");
    let _ = std::fs::remove_file(&path);
    let export = Export::start(
        scene(Size::new(32, 16)),
        settings(Container::Mov, Codec::DEFAULT, false, 60.0, 60.0, 0.0),
        path.clone(),
    )
    .expect("starts");

    let started = Instant::now();
    while export.progress().done == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no frame was encoded"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let progress = export.progress();
    assert_eq!(progress.total, 3600);
    assert!(
        progress.done < progress.total,
        "the export is still running"
    );

    export.cancel();
    assert!(matches!(export.wait(), Err(ExportError::Cancelled)));
    assert!(!path.exists(), "a cancelled export leaves no file");
}
