//! Renders a shader graph frame by frame at a fixed timestep and encodes the
//! frames with FFmpeg's built-in encoders, on a thread of its own.

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

/// The file's container. Which codecs it takes is given by `Codec::supports_container`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Mov,
    Matroska,
}

impl Container {
    pub const ALL: [Self; 2] = [Self::Mov, Self::Matroska];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mov => "mov",
            Self::Matroska => "mkv",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mov => "QuickTime",
            Self::Matroska => "Matroska",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::Mov => "mov",
            Self::Matroska => "matroska",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProResProfile {
    Proxy,
    Lt,
    Standard,
    Hq,
    P4444,
    P4444Xq,
}

impl ProResProfile {
    pub const ALL: [Self; 6] = [Self::Proxy, Self::Lt, Self::Standard, Self::Hq, Self::P4444, Self::P4444Xq];

    pub fn label(self) -> &'static str {
        match self {
            Self::Proxy => "Proxy",
            Self::Lt => "LT",
            Self::Standard => "422",
            Self::Hq => "422 HQ",
            Self::P4444 => "4444",
            Self::P4444Xq => "4444 XQ",
        }
    }

    /// The 4444 profiles are the only ones with a fourth, alpha, plane.
    pub fn is_4444(self) -> bool {
        matches!(self, Self::P4444 | Self::P4444Xq)
    }

    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Lt => "lt",
            Self::Standard => "standard",
            Self::Hq => "hq",
            Self::P4444 => "4444",
            Self::P4444Xq => "4444xq",
        }
    }
}

/// The DNxHR profiles. DNxHD itself only takes fixed frame sizes and rates, so it isn't offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DnxhrProfile {
    Lb,
    Sq,
    Hq,
    Hqx,
    P444,
}

impl DnxhrProfile {
    pub const ALL: [Self; 5] = [Self::Lb, Self::Sq, Self::Hq, Self::Hqx, Self::P444];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lb => "LB",
            Self::Sq => "SQ",
            Self::Hq => "HQ",
            Self::Hqx => "HQX",
            Self::P444 => "444",
        }
    }

    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Lb => "dnxhr_lb",
            Self::Sq => "dnxhr_sq",
            Self::Hq => "dnxhr_hq",
            Self::Hqx => "dnxhr_hqx",
            Self::P444 => "dnxhr_444",
        }
    }
}

/// The sample depth of a lossless codec. Frames arrive at 8 bits, so 16 keeps the same values in more room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Bits8,
    Bits16,
}

impl Depth {
    pub const ALL: [Self; 2] = [Self::Bits8, Self::Bits16];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bits8 => "8-bit",
            Self::Bits16 => "16-bit",
        }
    }
}

/// A video codec, with the options that change its stream. Alpha is set separately in `Settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    ProRes(ProResProfile),
    Dnxhr(DnxhrProfile),
    Ffv1(Depth),
    Png,
    QuickTimeAnimation,
}

impl Codec {
    pub const DEFAULT: Self = Self::ProRes(ProResProfile::Hq);

    pub fn label(self) -> &'static str {
        match self {
            Self::ProRes(_) => "ProRes",
            Self::Dnxhr(_) => "DNxHR",
            Self::Ffv1(_) => "FFV1",
            Self::Png => "PNG",
            Self::QuickTimeAnimation => "QuickTime Animation",
        }
    }

    /// The smallest frame the encoder takes, if it has one.
    pub fn min_size(self) -> Option<Size> {
        match self {
            Self::Dnxhr(_) => Some(Size::new(256, 120)),
            _ => None,
        }
    }

    /// Matroska doesn't keep the bit depth QuickTime Animation's decoder needs, so only MOV takes it.
    pub fn supports_container(self, container: Container) -> bool {
        match self {
            Self::QuickTimeAnimation => container == Container::Mov,
            Self::ProRes(_) | Self::Dnxhr(_) | Self::Ffv1(_) | Self::Png => true,
        }
    }

    pub fn supports_alpha(self) -> bool {
        match self {
            Self::ProRes(profile) => profile.is_4444(),
            Self::Dnxhr(_) => false,
            Self::Ffv1(_) | Self::Png | Self::QuickTimeAnimation => true,
        }
    }
}

/// What to write. `alpha` keeps the frames' alpha channel and is only valid where `Codec::supports_alpha` is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub container: Container,
    pub codec: Codec,
    pub alpha: bool,
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
        let size = video_size(scene.size);
        if let Some(min) = settings.codec.min_size()
            && (size.width < min.width || size.height < min.height)
        {
            return Err(ExportError::Invalid(format!(
                "{} needs frames of at least {} × {} pixels, and the canvas is {} × {}",
                settings.codec.label(),
                min.width,
                min.height,
                size.width,
                size.height,
            )));
        }
        let total = frame_count(&settings) as u64;
        let shared = Arc::new(Shared::default());
        shared.total.store(total, Ordering::Relaxed);
        let thread_shared = shared.clone();
        let failed_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("video-export".into())
            .spawn(move || run::run(scene, settings, path, &thread_shared))
            .map_err(|source| ExportError::Io {
                path: failed_path,
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
    if !settings.codec.supports_container(settings.container) {
        return invalid(&format!("{} can't be written to {}", settings.codec.label(), settings.container.label()));
    }
    if settings.alpha && !settings.codec.supports_alpha() {
        return invalid(&format!("{} has no alpha channel", settings.codec.label()));
    }
    if !(settings.fps.is_finite() && settings.fps > 0.0) {
        return invalid("the frame rate must be above zero");
    }
    // The time base has 1/1000 s ticks, so a rate under 0.0005 would need a zero denominator.
    if (settings.fps * 1000.0).round() < 1.0 {
        return invalid("the frame rate is too low");
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
            container: Container::Mov,
            codec: Codec::DEFAULT,
            alpha: false,
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
        assert!(validate(&settings(0.0, -1.0, 30.0)).is_err());
        assert!(validate(&settings(0.0, 0.0, 30.0)).is_err());
        assert!(validate(&settings(0.0, 1.0, 0.0004)).is_err());
        assert!(validate(&settings(0.0, 1.0, 0.001)).is_ok());
        assert!(validate(&settings(-1.0, 1.0, 30.0)).is_err());
        assert!(validate(&settings(0.0, 1.0, f64::NAN)).is_err());
        assert!(validate(&settings(0.0, 1.0, 30.0)).is_ok());
    }

    #[test]
    fn alpha_is_refused_where_the_codec_has_none() {
        let prores_422 = settings(0.0, 1.0, 30.0);
        assert!(validate(&Settings { alpha: true, ..prores_422 }).is_err());
        let prores_4444 = Settings { codec: Codec::ProRes(ProResProfile::P4444), ..prores_422 };
        assert!(validate(&Settings { alpha: true, ..prores_4444 }).is_ok());
        let dnxhr = Settings { codec: Codec::Dnxhr(DnxhrProfile::Hq), ..prores_422 };
        assert!(validate(&Settings { alpha: true, ..dnxhr }).is_err());
    }
}
