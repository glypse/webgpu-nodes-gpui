//! Lets someone who can't see the app measure it: spans on the hot path go to a
//! Chrome-format trace (open it in Perfetto) and to a summary printed at exit.
//! Nothing is recorded unless a probe flag asks for it.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::{LookupSpan, Registry};

const GPU_TARGET: &str = "probe::gpu";

static STATS: OnceLock<Stats> = OnceLock::new();

/// The trace being written, and where; taken by [`finish`]. gpui exits the process
/// without unwinding `main`, so the trace is finished from the app's quit hook.
static TRACE: Mutex<Option<(tracing_chrome::FlushGuard, PathBuf)>> = Mutex::new(None);
static FINISHED: AtomicBool = AtomicBool::new(false);

/// Starts recording, writing a Chrome trace to `trace` when given. Call once,
/// before any span is entered that should be counted.
pub fn install(trace: Option<&Path>) -> Result<()> {
    let stats = STATS.get_or_init(Stats::default);
    let chrome = match trace {
        Some(path) => {
            let (layer, flush) = tracing_chrome::ChromeLayerBuilder::new().file(path).build();
            if let Ok(mut slot) = TRACE.lock() {
                *slot = Some((flush, path.to_owned()));
            }
            Some(layer)
        }
        None => None,
    };
    let subscriber = Registry::default().with(StatsLayer(stats)).with(chrome);
    tracing::subscriber::set_global_default(subscriber).context("installing the probe")
}

/// Writes the trace out and prints the summary. Runs once, when the app quits.
pub fn finish() {
    if FINISHED.swap(true, Ordering::Relaxed) {
        return;
    }
    let trace = TRACE.lock().ok().and_then(|mut slot| slot.take());
    let path = trace.as_ref().map(|(_, path)| path.clone());
    drop(trace);
    if let Some(report) = report() {
        println!("{report}");
    }
    if let Some(path) = path {
        println!("trace: {}", path.display());
    }
}

/// Frames presented since [`install`]; zero when nothing is recorded.
pub fn frames() -> u64 {
    STATS.get().map_or(0, |s| s.frames.load(Ordering::Relaxed))
}

/// The summary of what was recorded, or `None` when nothing was.
pub fn report() -> Option<String> {
    let stats = STATS.get()?;
    let recorded = stats.recorded.lock().ok()?;
    Some(recorded.summary())
}

/// Writes a window's pixels to `path` as a PNG: the one titled `title` if given, else the largest.
/// Captures the window even when other windows cover it.
pub fn save_window(path: &Path, title: Option<&str>) -> Result<()> {
    let pid = std::process::id();
    let mut best: Option<(u64, xcap::Window)> = None;
    for window in xcap::Window::all().context("listing windows")? {
        let (Ok(owner), Ok(minimized)) = (window.pid(), window.is_minimized()) else {
            continue;
        };
        if owner != pid || minimized {
            continue;
        }
        if let Some(title) = title
            && !window.title().is_ok_and(|t| t == title)
        {
            continue;
        }
        let area = u64::from(window.width().unwrap_or(0)) * u64::from(window.height().unwrap_or(0));
        if best.as_ref().is_none_or(|(largest, _)| area > *largest) {
            best = Some((area, window));
        }
    }
    let (_, window) = best.context("this app has no window to capture")?;
    let image = window.capture_image().context("capturing the window")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    image.save(path).with_context(|| format!("writing {}", path.display()))
}

#[derive(Default)]
struct Stats {
    frames: AtomicU64,
    recorded: Mutex<Recorded>,
}

#[derive(Default)]
struct Recorded {
    first_frame: Option<Instant>,
    last_frame: Option<Instant>,
    frame_starts: Vec<Instant>,
    /// Busy time of each span kind, in seconds.
    spans: BTreeMap<String, Vec<f64>>,
    gpu: Vec<f64>,
}

/// What a span is recorded as: its name, or for a view's render, the view.
struct Open {
    key: String,
    entered: Option<Instant>,
    busy: Duration,
    first_entered: Option<Instant>,
}

struct StatsLayer(&'static Stats);

impl<S> Layer<S> for StatsLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let name = attrs.metadata().name();
        let mut view = ViewName(None);
        attrs.record(&mut view);
        let key = match view.0 {
            Some(view) => format!("{name} {view}"),
            None => name.to_owned(),
        };
        span.extensions_mut().insert(Open {
            key,
            entered: None,
            busy: Duration::ZERO,
            first_entered: None,
        });
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        let Some(open) = extensions.get_mut::<Open>() else { return };
        let now = Instant::now();
        open.entered = Some(now);
        open.first_entered.get_or_insert(now);
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        let Some(open) = extensions.get_mut::<Open>() else { return };
        if let Some(entered) = open.entered.take() {
            open.busy += entered.elapsed();
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let Some(open) = span.extensions_mut().remove::<Open>() else { return };
        let busy = open.busy.as_secs_f64();
        let Ok(mut recorded) = self.0.recorded.lock() else { return };
        if open.key == "present" {
            self.0.frames.fetch_add(1, Ordering::Relaxed);
            if let Some(first) = open.first_entered {
                recorded.first_frame.get_or_insert(first);
                recorded.last_frame = Some(first);
                recorded.frame_starts.push(first);
            }
        }
        recorded.spans.entry(open.key).or_default().push(busy);
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != GPU_TARGET {
            return;
        }
        let mut seconds = GpuSeconds(None);
        event.record(&mut seconds);
        if let (Some(seconds), Ok(mut recorded)) = (seconds.0, self.0.recorded.lock()) {
            recorded.gpu.push(seconds);
        }
    }
}

struct ViewName(Option<String>);

impl Visit for ViewName {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "view" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

struct GpuSeconds(Option<f64>);

impl Visit for GpuSeconds {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "seconds" {
            self.0 = Some(value);
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl Recorded {
    fn summary(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let frames = self.frame_starts.len();
        let wall = match (self.first_frame, self.last_frame) {
            (Some(first), Some(last)) => (last - first).as_secs_f64(),
            _ => 0.0,
        };
        let _ = writeln!(out, "probe summary");
        let _ = writeln!(out, "  frames presented: {frames}");
        if frames >= 2 && wall > 0.0 {
            let intervals: Vec<f64> = self
                .frame_starts
                .windows(2)
                .map(|w| (w[1] - w[0]).as_secs_f64() * 1e3)
                .collect();
            let fps = (frames - 1) as f64 / wall;
            let stats = Dist::of(intervals);
            let _ = writeln!(out, "  fps: {fps:.1} over {wall:.2} s");
            let _ = writeln!(
                out,
                "  frame time (ms): p50 {:.2}, p95 {:.2}, max {:.2}",
                stats.p50, stats.p95, stats.max
            );
        }
        if let Some(gpu) = (!self.gpu.is_empty()).then(|| Dist::of(self.gpu.iter().map(|s| s * 1e3).collect())) {
            let _ = writeln!(
                out,
                "  gpu time (ms, {} frames): mean {:.3}, p50 {:.3}, p95 {:.3}, max {:.3}",
                gpu.count, gpu.mean, gpu.p50, gpu.p95, gpu.max
            );
        }
        let _ = writeln!(out, "  spans, busy time (ms), by total:");
        let _ = writeln!(
            out,
            "    {:<36} {:>7} {:>10} {:>9} {:>9} {:>9} {:>9} {:>7}",
            "span", "count", "total", "mean", "p50", "p95", "max", "of wall"
        );
        let mut rows: Vec<(String, Dist)> = self
            .spans
            .iter()
            .map(|(name, busy)| (name.clone(), Dist::of(busy.iter().map(|s| s * 1e3).collect())))
            .collect();
        rows.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));
        for (name, d) in rows {
            let share = if wall > 0.0 { format!("{:.1}%", d.total / (wall * 1e3) * 100.0) } else { "-".into() };
            let _ = writeln!(
                out,
                "    {:<36} {:>7} {:>10.2} {:>9.3} {:>9.3} {:>9.3} {:>9.3} {:>7}",
                name, d.count, d.total, d.mean, d.p50, d.p95, d.max, share
            );
        }
        out
    }
}

struct Dist {
    count: usize,
    total: f64,
    mean: f64,
    p50: f64,
    p95: f64,
    max: f64,
}

impl Dist {
    fn of(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        let count = values.len();
        let total: f64 = values.iter().sum();
        let percentile = |p: f64| {
            if values.is_empty() {
                return 0.0;
            }
            let rank = (p * count as f64).ceil() as usize;
            values[rank.clamp(1, count) - 1]
        };
        Self {
            count,
            total,
            mean: if count > 0 { total / count as f64 } else { 0.0 },
            p50: percentile(0.5),
            p95: percentile(0.95),
            max: values.last().copied().unwrap_or(0.0),
        }
    }
}
