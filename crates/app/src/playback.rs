use std::sync::Arc;
use std::time::Instant;

use gpui::{Context, EventEmitter, RenderImage, SharedString, Window};
use runtime::{
    Frame, PixelFormat, Player, Program, ProgramError, ProgramSource, RefreshRateMeter,
    RenderOptions, Renderer, Size, Timing,
};

/// Seconds' worth of samples before the display's refresh rate is judged.
const REFRESH_SAMPLES: usize = 20;
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
    image: Option<Arc<RenderImage>>,
    /// The WGSL of the latest program the graph generated, working or not.
    pub code: SharedString,
    pub failure: Option<Failure>,
    pub refresh_rate: Option<f64>,
    refresh_meter: RefreshRateMeter,
    epoch: Instant,
    scheduled: bool,
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
        Self {
            renderer,
            program: None,
            player: Player::new(DEFAULT_TIMING),
            resolution: DEFAULT_RESOLUTION,
            image: None,
            code: SharedString::default(),
            failure,
            refresh_rate: None,
            refresh_meter: RefreshRateMeter::new(REFRESH_SAMPLES),
            epoch: Instant::now(),
            scheduled: false,
        }
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
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
        generated: Result<ProgramSource, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match generated {
            Ok(source) => {
                self.code = source.code.clone().into();
                if let Some(renderer) = &mut self.renderer {
                    match renderer.compile(&source) {
                        Ok(program) => {
                            self.program = Some(program);
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
                this.animation_frame(window, cx);
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
        let mut to_draw = None;
        self.player.animation_frame(timestamp, |inputs| {
            to_draw = Some(inputs);
            false
        });
        if let Some(inputs) = to_draw {
            self.draw(inputs, window);
            cx.emit(PlaybackEvent::Changed);
            cx.notify();
        }
        self.schedule(window, cx);
    }

    fn draw(&mut self, inputs: runtime::FrameInputs, window: &mut Window) {
        let (Some(renderer), Some(program)) = (&mut self.renderer, &self.program) else {
            return;
        };
        let rendered = renderer.render(
            program,
            inputs,
            RenderOptions {
                size: self.resolution,
                format: PixelFormat::Bgra8,
                premultiply: true,
            },
        );
        let frame = match rendered {
            Ok(frame) => frame,
            Err(error) => {
                self.failure = Some(Failure::text(error.to_string()));
                return;
            }
        };
        if let Some(seconds) = frame.gpu_time
            && self.player.playing()
        {
            self.player.meter.record_gpu_time(seconds);
        }
        // GPUI's images are BGRA bytes in an image buffer.
        let Some(buffer) =
            image::RgbaImage::from_raw(frame.size.width, frame.size.height, frame.pixels)
        else {
            return;
        };
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer
        )]));
        if let Some(old) = self.image.replace(image) {
            window.drop_image(old).ok();
        }
    }
}
