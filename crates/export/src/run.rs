//! The export's thread: renders each frame of the range at its fixed timestep and hands it to the encoder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use runtime::{
    FrameInputs, MediaSource, MediaStatus, PixelFormat, Presented, RenderOptions, Renderer, Size,
};
use shader_graph::codegen::Program;

use crate::encode::Encoder;
use crate::{ExportError, Scene, Settings, Shared, first_frame, frame_count, video_size};

/// Frames simulated between checks for cancellation.
const SIMULATE_BATCH: u64 = 64;
/// How long to wait between checks on images still decoding.
const MEDIA_POLL: Duration = Duration::from_millis(2);
/// Frames submitted before the oldest is read back and encoded, so the GPU draws
/// one while the previous is encoded. Must not exceed the renderer's readback depth.
const FRAMES_IN_FLIGHT: usize = 2;

pub fn run(
    scene: Scene,
    settings: Settings,
    path: PathBuf,
    shared: &Shared,
) -> Result<PathBuf, ExportError> {
    let outcome = encode(scene, settings, &path, shared);
    if outcome.is_err() {
        // A partial file is never left behind, whatever stopped the export.
        if let Err(source) = std::fs::remove_file(&path)
            && source.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("[export] couldn't remove {}: {source}", path.display());
        }
    }
    outcome.map(|()| path)
}

fn encode(
    scene: Scene,
    settings: Settings,
    path: &Path,
    shared: &Shared,
) -> Result<(), ExportError> {
    let size = video_size(scene.size);
    let total = frame_count(&settings) as u64;
    let first = first_frame(&settings);
    let fps = settings.fps;

    let mut renderer = Renderer::new().map_err(ExportError::Gpu)?;
    let compiled = renderer
        .compile(&scene.program.plan)
        .map_err(|error| ExportError::Program(error.to_string()))?;
    let uniforms: BTreeMap<String, Vec<f64>> = scene
        .program
        .uniforms
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.0.iter().map(|c| f64::from(*c)).collect(),
            )
        })
        .collect();
    renderer.set_uniforms(&compiled, &uniforms);
    renderer.sync_media(&scene.media);
    wait_for_media(&mut renderer, &scene.media, shared)?;

    let mut encoder = Encoder::open(path, &settings, size)?;
    let range = Range {
        size,
        first,
        total,
        fps,
    };
    draw_frames(
        &mut renderer,
        &compiled,
        simulates(&scene.program),
        range,
        &mut encoder,
        shared,
    )?;
    encoder.finish()
}

/// The frames to export: `total` of them, the first at index `first`, at `fps`.
struct Range {
    size: Size,
    first: u64,
    total: u64,
    fps: f64,
}

fn simulates(program: &Program) -> bool {
    program
        .plan
        .passes
        .iter()
        .any(|pass| pass.simulation.is_some())
}

/// Simulations run from the start, so the frames before the export's first are drawn without being kept.
fn draw_frames(
    renderer: &mut Renderer,
    compiled: &runtime::Program,
    simulates: bool,
    range: Range,
    encoder: &mut Encoder,
    shared: &Shared,
) -> Result<(), ExportError> {
    let Range {
        size,
        first,
        total,
        fps,
    } = range;
    let inputs = |frame: u64| FrameInputs {
        time: frame as f64 / fps,
        delta: 1.0 / fps,
        frame,
    };
    if simulates {
        let mut frame = 0;
        while frame < first {
            check_cancel(shared)?;
            let end = (frame + SIMULATE_BATCH).min(first);
            renderer
                .simulate(compiled, (frame..end).map(inputs), size)
                .map_err(ExportError::Render)?;
            frame = end;
        }
    }
    let options = RenderOptions {
        size,
        format: PixelFormat::Bgra8,
        premultiply: false,
    };
    let mut in_flight = 0;
    let mut written = 0;
    for index in 0..total {
        check_cancel(shared)?;
        if in_flight == FRAMES_IN_FLIGHT {
            write_oldest(renderer, encoder, shared, &mut written)?;
            in_flight -= 1;
        }
        renderer
            .submit(compiled, inputs(first + index), options)
            .map_err(ExportError::Render)?;
        in_flight += 1;
    }
    while in_flight > 0 {
        check_cancel(shared)?;
        write_oldest(renderer, encoder, shared, &mut written)?;
        in_flight -= 1;
    }
    Ok(())
}

fn write_oldest(
    renderer: &mut Renderer,
    encoder: &mut Encoder,
    shared: &Shared,
    written: &mut u64,
) -> Result<(), ExportError> {
    let frame = renderer
        .wait_frame()
        .ok_or_else(|| ExportError::Render(anyhow::anyhow!("no frame was in flight")))?
        .map_err(ExportError::Render)?;
    match frame {
        Presented::Pixels(frame) => encoder.push(&frame.pixels)?,
        #[cfg(target_os = "macos")]
        Presented::Surfaces { .. } => {
            return Err(ExportError::Render(anyhow::anyhow!(
                "a readback came out as a surface"
            )));
        }
    }
    *written += 1;
    shared.done.store(*written, Ordering::Relaxed);
    Ok(())
}

fn wait_for_media(
    renderer: &mut Renderer,
    media: &BTreeMap<String, MediaSource>,
    shared: &Shared,
) -> Result<(), ExportError> {
    loop {
        renderer.poll_media();
        if !media
            .keys()
            .any(|asset| renderer.media_status(asset) == MediaStatus::Loading)
        {
            return Ok(());
        }
        check_cancel(shared)?;
        std::thread::sleep(MEDIA_POLL);
    }
}

fn check_cancel(shared: &Shared) -> Result<(), ExportError> {
    if shared.cancel.load(Ordering::Relaxed) {
        Err(ExportError::Cancelled)
    } else {
        Ok(())
    }
}
