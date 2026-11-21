use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, EventEmitter, RenderImage, SharedString, Window};
use shader_graph::assets::AssetKind;
use shader_graph::codegen::ResourceDecl;

use crate::project::AssetFiles;
use runtime::{
    Frame, PixelFormat, Player, Presented, Program, ProgramError, RefreshRateMeter,
    RenderOptions, Renderer, Size, Timing,
};

/// Seconds' worth of samples before the display's refresh rate is judged.
const REFRESH_SAMPLES: usize = 20;
/// How often the preview's frame timing is logged at debug level.
const STATS_PERIOD: Duration = Duration::from_secs(5);
pub const DEFAULT_RESOLUTION: Size = Size::new(512, 512);
pub const DEFAULT_TIMING: Timing = Timing {
    fps: 60.0,
    realtime: true,
};

/// Why the program shown isn't the one asked for.
#[derive(Clone, Debug)]
pub struct Failure {
    pub message: SharedString,
    /// The program's lines around the failure, `(number, text, at fault)`.
    pub snippet: Vec<(u32, SharedString, bool)>,
}

impl Failure {
    pub fn text(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            snippet: Vec::new(),
        }
    }

    fn from_program(error: &ProgramError) -> Self {
        let line = error
            .diagnostics
            .iter()
            .find_map(|d| d.location)
            .map(|(line, _)| line);
        let snippet = line
            .map(|line| {
                let from = line.saturating_sub(2).max(1);
                error
                    .code
                    .lines()
                    .enumerate()
                    .map(|(i, text)| (i as u32 + 1, text))
                    .filter(|(n, _)| (from..=line + 2).contains(n))
                    .map(|(n, text)| (n, SharedString::from(text.to_string()), n == line))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            message: error.to_string().into(),
            snippet,
        }
    }
}

/// A drawn frame, as the window shows it.
#[derive(Clone)]
pub enum Shown {
    #[cfg(target_os = "macos")]
    Surface(runtime::Surface),
    Image(Arc<RenderImage>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// A frame was drawn, or the program or settings changed.
    Changed,
}

/// What the previews show: the generated program, drawn by the player's clock.
///
/// Drawing happens on the window's animation frames, only while playing or
/// when something changed (a seek, a new program, a new size).
pub struct Playback {
    renderer: Option<Renderer>,
    program: Option<Program>,
    pub player: Player,
    pub resolution: Size,
    /// The latest frame drawn, as the window shows it.
    shown: Option<Shown>,
    stats: FrameStats,
    /// The WGSL of the latest program the graph generated, working or not.
    pub code: SharedString,
    pub failure: Option<Failure>,
    pub refresh_rate: Option<f64>,
    refresh_meter: RefreshRateMeter,
    epoch: Instant,
    scheduled: bool,
    /// The files of the project's assets, which programs reading them are given.
    files: AssetFiles,
    /// The files already given to the renderer, to give each once.
    given: HashMap<String, Arc<Vec<u8>>>,
}

impl EventEmitter<PlaybackEvent> for Playback {}

impl Playback {
    pub fn new() -> Self {
        let (renderer, failure) = match Renderer::new() {
            Ok(renderer) => (Some(renderer), None),
            Err(error) => (
                None,
                Some(Failure::text(format!("No GPU for the preview: {error}"))),
            ),
        };
        let player = Player::new(DEFAULT_TIMING);
        Self {
            renderer,
            program: None,
            player,
            resolution: DEFAULT_RESOLUTION,
            shown: None,
            stats: FrameStats::default(),
            code: SharedString::default(),
            failure,
            refresh_rate: None,
            refresh_meter: RefreshRateMeter::new(REFRESH_SAMPLES),
            epoch: Instant::now(),
            scheduled: false,
            files: AssetFiles::new(),
            given: HashMap::new(),
        }
    }

    pub fn asset_files(&self) -> &AssetFiles {
        &self.files
    }

    /// Takes the files of the project's assets, for the programs reading them.
    pub fn set_asset_files(&mut self, files: AssetFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.files = files;
        self.sync_media();
        self.player.invalidate();
        self.changed(window, cx);
    }

    /// Gives the renderer the files the program reads that it doesn't have yet,
    /// and forgets those no longer there. A file that can't be used is logged
    /// and read as a transparent image.
    fn sync_media(&mut self) {
        let (Some(renderer), Some(program)) = (&mut self.renderer, &self.program) else { return };
        let wanted: HashMap<&str, AssetKind> = program
            .plan()
            .resources
            .iter()
            .filter_map(|r| match r {
                ResourceDecl::Media { asset, video, .. } => {
                    Some((asset.as_str(), if video.is_some() { AssetKind::Video } else { AssetKind::Image }))
                }
                _ => None,
            })
            .collect();
        for (asset, kind) in wanted {
            match self.files.get(asset) {
                Some(file) if self.given.get(asset).is_some_and(|g| Arc::ptr_eq(g, file)) => {}
                Some(file) => {
                    self.given.insert(asset.to_owned(), file.clone());
                    if let Err(error) = renderer.set_media(asset, kind, file) {
                        log::error!("the asset {asset} can't be shown: {error}");
                    }
                }
                None => {
                    self.given.remove(asset);
                    renderer.remove_media(asset);
                }
            }
        }
    }

    pub fn shown(&self) -> Option<Shown> {
        self.shown.clone()
    }

    pub fn timing(&self) -> Timing {
        self.player.clock.timing
    }

    /// The rate playback should draw at: the display's in real time, the project's otherwise.
    pub fn target_rate(&self) -> f64 {
        let timing = self.timing();
        match (timing.realtime, self.refresh_rate) {
            (true, Some(rate)) => rate.round(),
            _ => timing.fps,
        }
    }

    /// Takes a newly generated program: shows it once it compiles, keeping the last working one until then.
    pub fn set_source(
        &mut self,
        generated: Result<shader_graph::codegen::Program, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match generated {
            Ok(source) => {
                self.code = source.program.code.clone().into();
                if let Some(renderer) = &mut self.renderer {
                    match renderer.compile(&source.plan) {
                        Ok(program) => {
                            let values = source
                                .uniforms
                                .iter()
                                .map(|(name, v)| {
                                    (name.clone(), v.0.iter().map(|c| f64::from(*c)).collect())
                                })
                                .collect();
                            renderer.set_uniforms(&program, &values);
                            self.program = Some(program);
                            self.sync_media();
                            self.failure = None;
                            self.player.invalidate();
                        }
                        Err(error) => self.failure = Some(Failure::from_program(&error)),
                    }
                }
            }
            Err(message) => self.failure = Some(Failure::text(message)),
        }
        self.changed(window, cx);
    }

    /// Writes new uniform values into the running program, without compiling it again.
    pub fn set_uniforms(
        &mut self,
        uniforms: &[(String, shader_graph::UniformValue)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let (Some(renderer), Some(program)) = (&self.renderer, &self.program) {
            let values = uniforms
                .iter()
                .map(|(name, v)| (name.clone(), v.0.iter().map(|c| f64::from(*c)).collect()))
                .collect();
            renderer.set_uniforms(program, &values);
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    pub fn set_resolution(
        &mut self,
        resolution: Size,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if resolution != self.resolution {
            self.resolution = resolution;
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    pub fn set_timing(&mut self, timing: Timing, window: &mut Window, cx: &mut Context<Self>) {
        self.player.set_timing(timing);
        self.changed(window, cx);
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.player.toggle();
        self.changed(window, cx);
    }

    pub fn seek(&mut self, time: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.player.seek(time.max(0.0));
        self.changed(window, cx);
    }

    pub fn step(&mut self, frames: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.player.step(frames);
        self.changed(window, cx);
    }

    /// Renders the current frame at the canvas size, for export.
    pub fn export_frame(&mut self) -> anyhow::Result<Frame> {
        let (renderer, program) = self
            .renderer
            .as_mut()
            .zip(self.program.as_ref())
            .ok_or_else(|| anyhow::anyhow!("there is no program to draw"))?;
        renderer.render(
            program,
            self.player.clock.inputs(),
            RenderOptions {
                size: self.resolution,
                format: PixelFormat::Rgba8,
                premultiply: false,
            },
        )
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PlaybackEvent::Changed);
        cx.notify();
        self.schedule(window, cx);
    }

    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scheduled || !self.player.wants_animation_frame() {
            return;
        }
        self.scheduled = true;
        let this = cx.entity();
        window.on_next_frame(move |window, cx| {
            this.update(cx, |this, cx| {
                this.scheduled = false;
                guard::guard("animation frame", || this.animation_frame(window, cx));
            });
        });
    }

    fn animation_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let timestamp = self.epoch.elapsed().as_secs_f64();
        if self.player.playing()
            && let Some(rate) = self.refresh_meter.record(timestamp)
        {
            self.refresh_rate = Some(rate);
        }
        let mut changed = false;
        if let Some(result) = self.renderer.as_mut().and_then(|r| r.take_frame()) {
            self.player.finished();
            changed = true;
            match result {
                Ok(presented) => self.present(presented, window),
                Err(error) => self.failure = Some(Failure::text(error.to_string())),
            }
        }
        let drawable = self.renderer.is_some() && self.program.is_some();
        let mut to_draw = None;
        self.player.animation_frame(timestamp, |inputs| {
            to_draw = Some(inputs);
            drawable
        });
        if let Some(inputs) = to_draw
            && drawable
            && !self.submit(inputs)
        {
            self.player.finished();
        }
        if changed {
            cx.emit(PlaybackEvent::Changed);
            cx.notify();
        }
        self.schedule(window, cx);
    }

    /// Starts drawing a frame; it is shown once the GPU has finished with it.
    fn submit(&mut self, inputs: runtime::FrameInputs) -> bool {
        let (Some(renderer), Some(program)) = (&mut self.renderer, &self.program) else {
            return false;
        };
        let started = Instant::now();
        #[cfg(target_os = "macos")]
        let submitted = renderer.submit_surface(program, inputs, self.resolution);
        #[cfg(not(target_os = "macos"))]
        let submitted = renderer.submit(
            program,
            inputs,
            RenderOptions {
                size: self.resolution,
                format: PixelFormat::Bgra8,
                premultiply: true,
            },
        );
        self.stats.submitted(started.elapsed());
        match submitted {
            Ok(()) => true,
            Err(error) => {
                self.failure = Some(Failure::text(error.to_string()));
                false
            }
        }
    }

    fn present(&mut self, presented: Presented, window: &mut Window) {
        let started = Instant::now();
        let (shown, gpu_time) = match presented {
            #[cfg(target_os = "macos")]
            Presented::Surface { surface, gpu_time } => (Shown::Surface(surface), gpu_time),
            Presented::Pixels(frame) => {
                let gpu_time = frame.gpu_time;
                match image_of(frame) {
                    Some(image) => (Shown::Image(image), gpu_time),
                    None => {
                        log::error!("a frame's pixels don't match its size");
                        return;
                    }
                }
            }
        };
        if let Some(Shown::Image(old)) = self.shown.replace(shown)
            && let Err(error) = window.drop_image(old)
        {
            log::warn!("dropping the previous frame failed: {error:#}");
        }
        if let Some(seconds) = gpu_time
            && self.player.playing()
        {
            self.player.meter.record_gpu_time(seconds);
        }
        self.stats.presented(started.elapsed(), gpu_time);
    }
}

/// The frame as an image GPUI can draw: its BGRA bytes, premultiplied as the renderer drew them.
fn image_of(frame: Frame) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(frame.size.width, frame.size.height, frame.pixels)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// Averages the CPU and GPU cost of the preview's frames, logged every `STATS_PERIOD`.
#[derive(Default)]
struct FrameStats {
    since: Option<Instant>,
    submits: u32,
    submit_time: Duration,
    frames: u32,
    present_time: Duration,
    gpu_total: f64,
    gpu_count: u32,
}

impl FrameStats {
    fn submitted(&mut self, submit: Duration) {
        self.since.get_or_insert_with(Instant::now);
        self.submits += 1;
        self.submit_time += submit;
    }

    fn presented(&mut self, present: Duration, gpu: Option<f64>) {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        self.frames += 1;
        self.present_time += present;
        if let Some(seconds) = gpu {
            self.gpu_total += seconds;
            self.gpu_count += 1;
        }
        let elapsed = now.duration_since(since);
        if elapsed < STATS_PERIOD {
            return;
        }
        let frames = f64::from(self.frames);
        let per_frame_ms = |total: Duration| total.as_secs_f64() * 1e3 / frames;
        let gpu_ms = (self.gpu_count > 0)
            .then(|| self.gpu_total * 1e3 / f64::from(self.gpu_count));
        log::debug!(
            "preview: {:.1} fps ({:.2} ms per frame; submit {:.3} ms, present {:.3} ms, gpu {} ms)",
            frames / elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1e3 / frames,
            self.submit_time.as_secs_f64() * 1e3 / f64::from(self.submits.max(1)),
            per_frame_ms(self.present_time),
            gpu_ms.map_or_else(|| "n/a".to_owned(), |ms| format!("{ms:.3}")),
        );
        *self = Self {
            since: Some(now),
            ..Self::default()
        };
    }
}
