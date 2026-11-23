use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, EntityId, EventEmitter, RenderImage, SharedString, Task, Window};
use shader_graph::assets::{AssetKind, asset_hash};
use shader_graph::codegen::{RenderPlan, ResourceDecl};
use shader_graph::graph::MAX_CANVAS_SIZE;

use crate::project::AssetFiles;
use runtime::{
    CompileQueue, Compiled, Frame, MediaSource, MediaStatus, PixelFormat, Player, Presented,
    Program, ProgramError, RefreshRateMeter, RenderOptions, Renderer, Size, Timing,
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
/// A fixed canvas is drawn once, and every preview shows that frame. While following the
/// previews, each distinct size of a preview area is drawn: areas of the same size share one
/// render, and all renders of a frame use the same time and uniforms and go to the GPU together.
///
/// Drawing happens on the window's animation frames, only while playing or
/// when something changed (a seek, a new program, a new size).
pub struct Playback {
    renderer: Option<Renderer>,
    program: Option<Program>,
    /// Compiles the programs generated, off the UI thread.
    compiler: Option<CompileQueue>,
    /// Woken by the compile thread when a compile finishes. Taken by `watch_compiles`, which
    /// spawns the foreground task that drains the queue.
    compile_ready: Option<async_channel::Receiver<()>>,
    compile_watch: Option<Task<()>>,
    /// Compiles with an id up to this one are out of date: a newer source replaced them.
    discarded: u64,
    /// The uniform values of the latest program, which a program takes when it arrives.
    uniforms: Vec<(String, shader_graph::UniformValue)>,
    pub player: Player,
    /// The canvas size from the project, which frames render at unless following a preview area.
    pub resolution: Size,
    follow: bool,
    /// Each preview area's size in device pixels, by its entity.
    panes: HashMap<EntityId, Size>,
    /// The preview area whose size exports and the canvas readouts use: the one last used, else the first laid out.
    followed: Option<EntityId>,
    /// The latest frame drawn at each size, as the window shows it.
    frames: HashMap<Size, Shown>,
    /// The frame each area showed last, which it keeps until one at its new size is drawn.
    #[cfg(target_os = "macos")]
    held: HashMap<EntityId, (Size, Shown)>,
    stats: FrameStats,
    /// Frames sent to the GPU so far: every window showing the playback shows these, none draws its own.
    submitted: u64,
    /// The WGSL of the latest program the graph generated, working or not.
    pub code: SharedString,
    pub failure: Option<Failure>,
    pub refresh_rate: Option<f64>,
    refresh_meter: RefreshRateMeter,
    epoch: Instant,
    scheduled: bool,
    /// The files of the project's assets, which programs reading them are given.
    files: AssetFiles,
    /// The hash of each file given, by asset id, computed once per file.
    hashes: HashMap<String, (Arc<Vec<u8>>, String)>,
    media_watching: bool,
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
        let (compiler, compile_ready) = match renderer.as_ref() {
            Some(renderer) => {
                // Capacity one: a wake-up that finds one already queued adds nothing, since
                // the drain takes every finished compile.
                let (ready, compile_ready) = async_channel::bounded(1);
                let compiler = CompileQueue::new(renderer.compiler(), move || {
                    let _ = ready.try_send(());
                });
                (Some(compiler), Some(compile_ready))
            }
            None => (None, None),
        };
        Self {
            renderer,
            program: None,
            compiler,
            compile_ready,
            compile_watch: None,
            discarded: 0,
            uniforms: Vec::new(),
            player,
            resolution: DEFAULT_RESOLUTION,
            follow: false,
            panes: HashMap::new(),
            followed: None,
            frames: HashMap::new(),
            #[cfg(target_os = "macos")]
            held: HashMap::new(),
            stats: FrameStats::default(),
            submitted: 0,
            code: SharedString::default(),
            failure,
            refresh_rate: None,
            refresh_meter: RefreshRateMeter::new(REFRESH_SAMPLES),
            epoch: Instant::now(),
            scheduled: false,
            files: AssetFiles::new(),
            hashes: HashMap::new(),
            media_watching: false,
        }
    }

    /// How the renderer holds an asset's file. Only the files programs read reach the renderer.
    pub fn media_status(&self, asset: &str) -> Option<MediaStatus> {
        self.renderer.as_ref().map(|r| r.media_status(asset))
    }

    pub fn asset_files(&self) -> &AssetFiles {
        &self.files
    }

    /// Takes the files of the project's assets, for the programs reading them.
    pub fn set_asset_files(
        &mut self,
        files: AssetFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.files = files;
        self.hashes.clear();
        self.watch_media(window, cx);
        self.sync_media();
        self.player.invalidate();
        self.changed(window, cx);
    }

    /// Gives the renderer the assets' files that the program reads, and drops
    /// the images no program reads any more. Decoding happens off the main
    /// thread: `media_ready` shows each one as it arrives.
    fn sync_media(&mut self) {
        let (Some(renderer), Some(program)) = (&mut self.renderer, &self.program) else {
            return;
        };
        let sources = media_sources(&self.files, &mut self.hashes, program.plan());
        renderer.sync_media(&sources);
    }

    /// The media a program of `plan` reads, for an export to draw with.
    pub fn export_media(&mut self, plan: &RenderPlan) -> BTreeMap<String, MediaSource> {
        media_sources(&self.files, &mut self.hashes, plan)
    }

    /// Starts polling the renderer for decoded images, once, when the renderer
    /// first has a reason to read them.
    fn watch_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.media_watching {
            return;
        }
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        self.media_watching = true;
        let (wake, woken) = async_channel::bounded::<()>(1);
        renderer.set_media_waker(move || {
            let _ = wake.try_send(());
        });
        cx.spawn_in(window, async move |this, cx| {
            while woken.recv().await.is_ok() {
                let updated = guard::guard("media ready", || {
                    this.update_in(cx, |p, window, cx| p.media_ready(window, cx))
                });
                if updated.is_none_or(|updated| updated.is_err()) {
                    break;
                }
            }
        })
        .detach();
    }

    fn media_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renderer.as_mut().is_some_and(|r| r.poll_media()) {
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    #[cfg(test)]
    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    /// The frame a preview area or output window shows, and the size it was drawn at.
    pub fn pane_frame(&self, area: EntityId) -> Option<(Size, Shown)> {
        let size = self.size_of(area);
        if let Some(shown) = self.frames.get(&size) {
            return Some((size, shown.clone()));
        }
        #[cfg(target_os = "macos")]
        if let Some(held) = self.held.get(&area) {
            return Some(held.clone());
        }
        self.frames
            .iter()
            .next()
            .map(|(size, shown)| (*size, shown.clone()))
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

    /// Takes a newly generated program: compiles it off the UI thread and shows it once it is
    /// ready, keeping the last working one until then.
    pub fn set_source(
        &mut self,
        generated: Result<shader_graph::codegen::Program, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _span = tracing::info_span!("set_source").entered();
        match generated {
            Ok(source) => {
                self.code = source.program.code.clone().into();
                self.uniforms = source.uniforms;
                if let Some(compiler) = &mut self.compiler {
                    compiler.submit(*source.plan);
                    self.watch_compiles(window, cx);
                }
            }
            Err(message) => {
                if let Some(compiler) = &self.compiler {
                    self.discarded = compiler.latest();
                }
                self.failure = Some(Failure::text(message));
            }
        }
        self.changed(window, cx);
    }

    /// Starts draining finished compiles as the compile thread reports them. The task lives as
    /// long as the playback; it is spawned on the first source.
    fn watch_compiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.compile_watch.is_some() {
            return;
        }
        let Some(ready) = self.compile_ready.take() else {
            return;
        };
        self.compile_watch = Some(cx.spawn_in(window, async move |this, cx| {
            while ready.recv().await.is_ok() {
                let updated = guard::guard("compiles ready", || {
                    this.update_in(cx, |p, window, cx| p.compiles_ready(window, cx))
                });
                if !matches!(updated, Some(Ok(()))) {
                    break;
                }
            }
        }));
    }

    fn compiles_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        while let Some(done) = self.compiler.as_mut().and_then(CompileQueue::try_next) {
            changed |= self.take_compiled(done);
        }
        if changed {
            self.changed(window, cx);
        }
    }

    /// Waits for the program of the latest source, which an export draws with.
    fn finish_compiling(&mut self) {
        while let Some(done) = self.compiler.as_mut().and_then(CompileQueue::wait_next) {
            self.take_compiled(done);
        }
    }

    /// Whether the latest source is still compiling.
    pub fn compiling(&self) -> bool {
        self.compiler.as_ref().is_some_and(CompileQueue::busy)
    }

    /// Shows a program that has compiled, or its error. A source newer than the compile only
    /// has its program shown early: whatever it says about the latest source is for the latest.
    fn take_compiled(&mut self, done: Compiled) -> bool {
        if done.id <= self.discarded {
            return false;
        }
        let latest = !self.compiling();
        match done.result {
            Ok(program) => {
                if let Some(renderer) = &self.renderer {
                    renderer.set_uniforms(&program, &uniform_values(&self.uniforms));
                }
                self.program = Some(program);
                self.sync_media();
                if latest {
                    self.failure = None;
                }
                self.player.invalidate();
                true
            }
            Err(error) if latest => {
                self.failure = Some(Failure::from_program(&error));
                true
            }
            Err(_) => false,
        }
    }

    /// Writes new uniform values into the running program, without compiling it again.
    pub fn set_uniforms(
        &mut self,
        uniforms: &[(String, shader_graph::UniformValue)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (name, value) in uniforms {
            match self.uniforms.iter_mut().find(|(n, _)| n == name) {
                Some((_, held)) => *held = value.clone(),
                None => self.uniforms.push((name.clone(), value.clone())),
            }
        }
        if let (Some(renderer), Some(program)) = (&self.renderer, &self.program) {
            renderer.set_uniforms(program, &uniform_values(uniforms));
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    /// Takes the canvas settings: frames render at the preview areas' sizes while `follow`.
    pub fn set_canvas(
        &mut self,
        follow: bool,
        resolution: Size,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if follow == self.follow && resolution == self.resolution {
            return;
        }
        let before = self.render_sizes();
        self.follow = follow;
        self.resolution = resolution;
        if self.render_sizes() != before {
            self.player.invalidate();
        }
        self.changed(window, cx);
    }

    pub fn following(&self) -> bool {
        self.follow
    }

    /// The size frames render at for exports and the canvas readouts: the followed preview area's, while following it.
    pub fn render_size(&self) -> Size {
        let pane = self
            .followed
            .and_then(|area| self.panes.get(&area))
            .copied();
        canvas_size(self.follow, pane, self.resolution)
    }

    /// The size frames render at for a preview area: its own, while following.
    pub fn size_of(&self, area: EntityId) -> Size {
        canvas_size(self.follow, self.panes.get(&area).copied(), self.resolution)
    }

    /// The distinct sizes each frame is drawn at.
    pub fn render_sizes(&self) -> Vec<Size> {
        if !self.follow || self.panes.is_empty() {
            return vec![self.render_size()];
        }
        let mut sizes: Vec<Size> = Vec::new();
        for size in self
            .followed
            .iter()
            .filter_map(|a| self.panes.get(a))
            .chain(self.panes.values())
            .copied()
        {
            if !sizes.contains(&size) {
                sizes.push(size);
            }
        }
        sizes
    }

    /// Reports a preview area's size in device pixels, which frames are drawn at while `follow`.
    pub fn set_pane(
        &mut self,
        area: EntityId,
        size: Size,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.render_sizes();
        if self.panes.insert(area, size) == Some(size) {
            return;
        }
        self.followed.get_or_insert(area);
        if self.follow && self.render_sizes() != before {
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    /// Makes `area` the one the canvas readouts follow: the one last used.
    pub fn follow_pane(&mut self, area: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if self.followed == Some(area) || !self.panes.contains_key(&area) {
            return;
        }
        let before = self.render_size();
        self.followed = Some(area);
        if self.follow && self.render_size() != before {
            self.player.invalidate();
            self.changed(window, cx);
        }
    }

    /// Forgets a preview area that has gone, with the render only it used.
    pub fn forget_pane(&mut self, area: EntityId, cx: &mut Context<Self>) {
        let before = self.render_sizes();
        if self.panes.remove(&area).is_none() {
            return;
        }
        #[cfg(target_os = "macos")]
        self.held.remove(&area);
        if self.followed == Some(area) {
            self.followed = self.panes.keys().next().copied();
        }
        if self.follow && self.render_sizes() != before {
            self.player.invalidate();
        }
        cx.notify();
    }

    pub fn set_timing(&mut self, timing: Timing, window: &mut Window, cx: &mut Context<Self>) {
        self.player.set_timing(timing);
        self.changed(window, cx);
    }

    pub fn playing(&self) -> bool {
        self.player.playing()
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
        self.finish_compiling();
        let size = self.render_size();
        let inputs = self.player.clock.inputs();
        let (renderer, program) = self
            .renderer
            .as_mut()
            .zip(self.program.as_ref())
            .ok_or_else(|| anyhow::anyhow!("there is no program to draw"))?;
        renderer.render(
            program,
            inputs,
            RenderOptions {
                size,
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
        let _tick = tracing::info_span!("playback tick").entered();
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
        let _span = tracing::info_span!("submit").entered();
        let sizes = self.render_sizes();
        let (Some(renderer), Some(program)) = (&mut self.renderer, &self.program) else {
            return false;
        };
        let started = Instant::now();
        #[cfg(target_os = "macos")]
        let submitted = renderer.submit_surfaces(program, inputs, &sizes);
        #[cfg(not(target_os = "macos"))]
        let submitted = renderer.submit(
            program,
            inputs,
            RenderOptions {
                size: self.render_size(),
                format: PixelFormat::Bgra8,
                premultiply: true,
            },
        );
        self.stats.submitted(started.elapsed());
        match submitted {
            Ok(()) => {
                self.submitted += 1;
                true
            }
            Err(error) => {
                self.failure = Some(Failure::text(error.to_string()));
                false
            }
        }
    }

    fn present(&mut self, presented: Presented, window: &mut Window) {
        let _span = tracing::info_span!("present").entered();
        let started = Instant::now();
        let (shown, gpu_time) = match presented {
            #[cfg(target_os = "macos")]
            Presented::Surfaces { surfaces, gpu_time } => (
                surfaces
                    .into_iter()
                    .map(|(size, surface)| (size, Shown::Surface(surface)))
                    .collect::<HashMap<_, _>>(),
                gpu_time,
            ),
            Presented::Pixels(frame) => {
                let gpu_time = frame.gpu_time;
                let size = frame.size;
                match image_of(frame) {
                    Some(image) => (HashMap::from([(size, Shown::Image(image))]), gpu_time),
                    None => {
                        log::error!("a frame's pixels don't match its size");
                        return;
                    }
                }
            }
        };
        #[cfg(target_os = "macos")]
        {
            let sizes: Vec<(EntityId, Size)> = self
                .panes
                .keys()
                .map(|area| (*area, self.size_of(*area)))
                .collect();
            for (area, size) in sizes {
                if let Some(frame) = shown.get(&size) {
                    self.held.insert(area, (size, frame.clone()));
                }
            }
        }
        for (_, old) in std::mem::replace(&mut self.frames, shown) {
            if let Shown::Image(old) = old
                && let Err(error) = window.drop_image(old)
            {
                log::warn!("dropping the previous frame failed: {error:#}");
            }
        }
        if let Some(seconds) = gpu_time {
            tracing::debug!(target: "probe::gpu", seconds);
        }
        if let Some(seconds) = gpu_time
            && self.player.playing()
        {
            self.player.meter.record_gpu_time(seconds);
        }
        self.stats.presented(started.elapsed(), gpu_time);
    }
}

/// The media `plan` reads, from the project's files. Each file is hashed once: its hash is kept until the file changes.
fn media_sources(
    files: &AssetFiles,
    hashes: &mut HashMap<String, (Arc<Vec<u8>>, String)>,
    plan: &RenderPlan,
) -> BTreeMap<String, MediaSource> {
    let mut sources = BTreeMap::new();
    for resource in &plan.resources {
        let ResourceDecl::Media { asset, video, .. } = resource else {
            continue;
        };
        let Some(bytes) = files.get(asset) else {
            continue;
        };
        let kind = if video.is_some() {
            AssetKind::Video
        } else {
            AssetKind::Image
        };
        let hash = match hashes.get(asset) {
            Some((known, hash)) if Arc::ptr_eq(known, bytes) => hash.clone(),
            _ => {
                let hash = asset_hash(bytes);
                hashes.insert(asset.clone(), (bytes.clone(), hash.clone()));
                hash
            }
        };
        sources.insert(
            asset.clone(),
            MediaSource {
                kind,
                hash,
                bytes: bytes.clone(),
            },
        );
    }
    sources
}

fn uniform_values(uniforms: &[(String, shader_graph::UniformValue)]) -> BTreeMap<String, Vec<f64>> {
    uniforms
        .iter()
        .map(|(name, v)| (name.clone(), v.0.iter().map(|c| f64::from(*c)).collect()))
        .collect()
}

/// The size frames render at: the followed pane's while following it, else `stored`.
fn canvas_size(follow: bool, pane: Option<Size>, stored: Size) -> Size {
    match (follow, pane) {
        (true, Some(pane)) => pane,
        _ => stored,
    }
}

/// A preview area's logical size as device pixels: whole pixels, within the canvas limit.
/// Empty areas (hidden or collapsed) have none, so they keep their last size.
pub fn device_size(width: f32, height: f32, scale: f32) -> Option<Size> {
    // `min` would return the cap for NaN, so the comparison is written to send NaN to 0.
    let side = |logical: f32| {
        let pixels = (logical * scale).round();
        if pixels > 0.0 {
            pixels.min(MAX_CANVAS_SIZE as f32) as u32
        } else {
            0
        }
    };
    let size = Size::new(side(width), side(height));
    (size.width > 0 && size.height > 0).then_some(size)
}

/// The frame as an image GPUI can draw: its BGRA bytes, premultiplied as the renderer drew them.
fn image_of(frame: Frame) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(frame.size.width, frame.size.height, frame.pixels)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buffer)
    ])))
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
        let gpu_ms = (self.gpu_count > 0).then(|| self.gpu_total * 1e3 / f64::from(self.gpu_count));
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

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, Entity, IntoElement, Render, TestAppContext};
    use shader_graph::graph::GraphModel;

    use super::*;

    struct Harness(Entity<Playback>);

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    fn scene() -> shader_graph::codegen::Program {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/default.json");
        let loaded = crate::project::read(&path).expect("the default scene loads");
        GraphModel::new(loaded.project)
            .generate()
            .expect("the default scene generates")
    }

    fn set_source(
        cx: &mut gpui::VisualTestContext,
        playback: &Entity<Playback>,
        source: Result<shader_graph::codegen::Program, String>,
    ) {
        cx.update(|window, cx| playback.update(cx, |p, cx| p.set_source(source, window, cx)));
    }

    /// Waits for the compile thread, then lets the drain task run on the test executor.
    fn wait_for_compile(cx: &mut gpui::VisualTestContext, playback: &Entity<Playback>) {
        cx.executor().allow_parking();
        for _ in 0..2000 {
            cx.run_until_parked();
            if !playback.read_with(cx, |p, _| p.compiling()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the compile never finished");
    }

    #[gpui::test]
    fn a_source_is_compiled_in_the_background_and_then_shown(cx: &mut TestAppContext) {
        let (harness, cx) = cx.add_window_view(|_, cx| Harness(cx.new(|_| Playback::new())));
        let playback = harness.read_with(cx, |h, _| h.0.clone());
        set_source(cx, &playback, Ok(scene()));
        assert!(
            playback.read_with(cx, |p, _| !p.code.is_empty()),
            "the code is known at once"
        );
        wait_for_compile(cx, &playback);
        playback.read_with(cx, |p, _| {
            assert!(p.program.is_some(), "the compiled program was swapped in");
            assert!(p.failure.is_none());
        });
    }

    #[gpui::test]
    fn a_program_that_fails_to_compile_keeps_the_last_one_and_reports_the_error(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = cx.add_window_view(|_, cx| Harness(cx.new(|_| Playback::new())));
        let playback = harness.read_with(cx, |h, _| h.0.clone());
        set_source(cx, &playback, Ok(scene()));
        wait_for_compile(cx, &playback);
        let good = playback.read_with(cx, |p, _| p.program.as_ref().map(|p| p.id()));

        let mut broken = scene();
        for pass in &mut broken.plan.passes {
            pass.program.code.push_str("\nfn broken( {");
        }
        set_source(cx, &playback, Ok(broken));
        wait_for_compile(cx, &playback);
        playback.read_with(cx, |p, _| {
            assert!(p.failure.is_some(), "the error is reported");
            assert_eq!(
                p.program.as_ref().map(|p| p.id()),
                good,
                "the last working program stays"
            );
        });

        set_source(cx, &playback, Ok(scene()));
        wait_for_compile(cx, &playback);
        playback.read_with(cx, |p, _| {
            assert!(p.failure.is_none());
            assert_ne!(p.program.as_ref().map(|p| p.id()), good);
        });
    }

    #[gpui::test]
    fn a_generation_error_is_not_cleared_by_an_older_compile(cx: &mut TestAppContext) {
        let (harness, cx) = cx.add_window_view(|_, cx| Harness(cx.new(|_| Playback::new())));
        let playback = harness.read_with(cx, |h, _| h.0.clone());
        set_source(cx, &playback, Ok(scene()));
        set_source(cx, &playback, Err("no output".into()));
        wait_for_compile(cx, &playback);
        playback.read_with(cx, |p, _| {
            assert_eq!(
                p.failure.as_ref().map(|f| f.message.to_string()).as_deref(),
                Some("no output")
            );
            assert!(p.program.is_none());
        });
    }

    #[gpui::test]
    fn an_export_waits_for_the_program_of_the_latest_source(cx: &mut TestAppContext) {
        let (harness, cx) = cx.add_window_view(|_, cx| Harness(cx.new(|_| Playback::new())));
        let playback = harness.read_with(cx, |h, _| h.0.clone());
        set_source(cx, &playback, Ok(scene()));
        let frame = playback
            .update(cx, |p, _| p.export_frame())
            .expect("a frame");
        assert!(!frame.pixels.is_empty());
    }

    #[test]
    fn pane_sizes_are_whole_device_pixels() {
        assert_eq!(device_size(100.4, 50.6, 2.0), Some(Size::new(201, 101)));
    }

    #[test]
    fn empty_panes_have_no_size() {
        assert_eq!(device_size(0.0, 300.0, 2.0), None);
        assert_eq!(device_size(300.0, -1.0, 1.0), None);
        assert_eq!(device_size(f32::NAN, 300.0, 1.0), None);
    }

    #[test]
    fn pane_sizes_are_capped_at_the_canvas_limit() {
        assert_eq!(
            device_size(10_000.0, 10.0, 1.0),
            Some(Size::new(MAX_CANVAS_SIZE, 10))
        );
    }

    #[test]
    fn the_canvas_follows_the_pane_only_while_following() {
        let stored = Size::new(1920, 1080);
        let pane = Some(Size::new(800, 600));
        assert_eq!(canvas_size(true, pane, stored), Size::new(800, 600));
        assert_eq!(canvas_size(false, pane, stored), stored);
        assert_eq!(canvas_size(true, None, stored), stored);
    }
}
