//! Renders a small graph on the real GPU and encodes it: the file's container
//! is read back for its frame count and size.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use export::{Export, ExportError, Format, Quality, Scene, Settings};
use runtime::Size;
use shader_graph::graph::{Connection, FRAG_OUTPUT, GraphModel, Point};
use shader_graph::types::{InputValue, WgslType};

fn add(model: &mut GraphModel, node_type: &str) -> String {
    model.add_node(node_type, Point::default()).unwrap_or_else(|| panic!("adds {node_type}"))
}

fn link(model: &mut GraphModel, from: (&str, &str), to: (&str, &str)) {
    let connection = Connection::new(from.0, from.1, to.0, to.1);
    assert!(model.is_valid_connection(&connection), "invalid link {connection:?}");
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

fn settings(duration: f64, fps: f64, start: f64) -> Settings {
    Settings {
        format: Format::Mp4,
        quality: Quality::Medium,
        start,
        duration,
        fps,
    }
}

#[test]
fn a_range_is_encoded_at_the_even_size_with_its_frame_count() {
    let path = output_path("range.mp4");
    let _ = std::fs::remove_file(&path);
    // 33 by 17 is odd on both sides, so the video is 32 by 16.
    let export = Export::start(scene(Size::new(33, 17)), settings(1.0, 30.0, 0.5), path.clone())
        .expect("starts");
    let written = export.wait().expect("encodes");
    assert_eq!(written, path);

    let bytes = std::fs::read(&path).expect("the file exists");
    let mp4 = re_mp4::Mp4::read_bytes(&bytes).expect("an MP4 file");
    let track = mp4
        .tracks()
        .values()
        .find(|track| track.width != 0)
        .expect("a video track");
    assert_eq!((track.width, track.height), (32, 16));
    assert_eq!(track.samples.len(), 30, "one sample per frame of the range");
}

#[test]
fn progress_counts_the_frames_and_cancel_removes_the_file() {
    let path = output_path("cancelled.mp4");
    let _ = std::fs::remove_file(&path);
    let export = Export::start(scene(Size::new(32, 16)), settings(60.0, 60.0, 0.0), path.clone())
        .expect("starts");

    let started = Instant::now();
    while export.progress().done == 0 {
        assert!(started.elapsed() < Duration::from_secs(60), "no frame was encoded");
        std::thread::sleep(Duration::from_millis(1));
    }
    let progress = export.progress();
    assert_eq!(progress.total, 3600);
    assert!(progress.done < progress.total, "the export is still running");

    export.cancel();
    assert!(matches!(export.wait(), Err(ExportError::Cancelled)));
    assert!(!path.exists(), "a cancelled export leaves no file");
}
