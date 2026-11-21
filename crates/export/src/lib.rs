//! Renders a shader graph frame by frame at a fixed timestep and encodes the
//! frames as an MP4 on a thread of its own. Encoding uses AVFoundation, so the
//! crate is macOS only.

#![cfg(target_os = "macos")]

mod encode;
mod run;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use runtime::{MediaSource, Size};
use shader_graph::codegen::Program;
use thiserror::Error;

/// The container and codec the file is written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Mp4,
    WebM,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::WebM => "WebM",
        }
    }
}

/// The quality presets of the original's export dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Medium,
    High,
    VeryHigh,
}

impl Quality {
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub format: Format,
    pub quality: Quality,
    /// Seconds of the first frame.
    pub start: f64,
    /// Seconds.
    pub duration: f64,
    pub fps: f64,
}

/// What to draw: the program with its uniforms and media, at the canvas size.
pub struct Scene {
    pub program: Program,
    pub media: BTreeMap<String, MediaSource>,
    pub size: Size,
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("the export settings are invalid: {0}")]
    Invalid(String),
    #[error("no GPU for the export: {0:#}")]
    Gpu(anyhow::Error),
    #[error("the program doesn't compile: {0}")]
    Program(String),
    #[error("a frame couldn't be rendered: {0:#}")]
    Render(anyhow::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("encoding failed: {0}")]
    Encode(String),
    #[error("writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the export was cancelled")]
    Cancelled,
    #[error("the export stopped unexpectedly")]
    Panicked,
}

/// Frames done of the total, the total being known once the export starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

#[derive(Default)]
struct Shared {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

/// A running export. Dropping it does not stop the export: call `cancel` first.
pub struct Export {
    shared: Arc<Shared>,
    thread: JoinHandle<Result<PathBuf, ExportError>>,
}

impl Export {
    /// Validates the settings and starts encoding `scene` into `path` on a thread of its own.
    pub fn start(scene: Scene, settings: Settings, path: PathBuf) -> Result<Self, ExportError> {
        validate(&settings)?;
        let total = frame_count(&settings) as u64;
        let shared = Arc::new(Shared::default());
        shared.total.store(total, Ordering::Relaxed);
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("video-export".into())
            .spawn(move || run::run(scene, settings, path, &thread_shared))
            .map_err(|source| ExportError::Io {
                path: PathBuf::new(),
                source,
            })?;
        Ok(Self { shared, thread })
    }

    pub fn progress(&self) -> Progress {
        Progress {
            done: self.shared.done.load(Ordering::Relaxed),
            total: self.shared.total.load(Ordering::Relaxed),
        }
    }

    /// Asks the export to stop; it finishes its current frame, then removes the partial file.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits for the export to finish and returns the written file's path.
    pub fn wait(self) -> Result<PathBuf, ExportError> {
        self.thread.join().unwrap_or(Err(ExportError::Panicked))
    }
}

fn validate(settings: &Settings) -> Result<(), ExportError> {
    let invalid = |why: &str| Err(ExportError::Invalid(why.to_owned()));
    if settings.format != Format::Mp4 {
        return Err(ExportError::Unsupported(format!("{} export isn't available yet", settings.format.label())));
    }
    if !(settings.fps.is_finite() && settings.fps > 0.0) {
        return invalid("the frame rate must be above zero");
    }
    if !(settings.duration.is_finite() && settings.duration > 0.0) {
        return invalid("the duration must be above zero");
    }
    if !(settings.start.is_finite() && settings.start >= 0.0) {
        return invalid("the start can't be negative");
    }
    Ok(())
}

/// Encoders subsample chroma by two, so frames need even sides.
pub fn video_size(size: Size) -> Size {
    Size::new(size.width.saturating_sub(size.width % 2).max(2), size.height.saturating_sub(size.height % 2).max(2))
}

/// The number of frames exported: at least one.
pub fn frame_count(settings: &Settings) -> usize {
    (settings.duration * settings.fps).round().max(1.0) as usize
}

/// The index of the frame at `settings.start`, on the grid of `settings.fps`.
pub fn first_frame(settings: &Settings) -> u64 {
    (settings.start * settings.fps).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(start: f64, duration: f64, fps: f64) -> Settings {
        Settings {
            format: Format::Mp4,
            quality: Quality::High,
            start,
            duration,
            fps,
        }
    }

    #[test]
    fn frames_are_counted_on_the_fps_grid() {
        assert_eq!(frame_count(&settings(0.0, 2.0, 30.0)), 60);
        assert_eq!(frame_count(&settings(0.0, 1.0 / 60.0, 30.0)), 1, "at least one frame");
        assert_eq!(frame_count(&settings(0.0, 1.0, 29.97)), 30);
    }

    #[test]
    fn the_first_frame_is_the_start_on_the_grid() {
        assert_eq!(first_frame(&settings(1.5, 1.0, 60.0)), 90);
        assert_eq!(first_frame(&settings(0.0, 1.0, 60.0)), 0);
    }

    #[test]
    fn sides_are_made_even() {
        assert_eq!(video_size(Size::new(513, 1)), Size::new(512, 2));
        assert_eq!(video_size(Size::new(640, 480)), Size::new(640, 480));
    }

    #[test]
    fn bad_settings_are_refused() {
        assert!(validate(&settings(0.0, 0.0, 30.0)).is_err());
        assert!(validate(&settings(-1.0, 1.0, 30.0)).is_err());
        assert!(validate(&settings(0.0, 1.0, f64::NAN)).is_err());
        assert!(validate(&settings(0.0, 1.0, 30.0)).is_ok());
    }

    #[test]
    fn webm_is_refused_until_it_is_supported() {
        let webm = Settings { format: Format::WebM, ..settings(0.0, 1.0, 30.0) };
        assert!(matches!(validate(&webm), Err(ExportError::Unsupported(_))));
    }
}
