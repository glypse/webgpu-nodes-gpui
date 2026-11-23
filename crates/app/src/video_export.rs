//! The video export's output settings, kept with the project, and the export
//! job the Output section runs: frames are rendered and encoded on a thread of
//! the export crate's own, and the job polls it for progress.

use crate::properties::MAX_SECONDS;
use std::path::PathBuf;
use std::time::Duration;

use export::{Codec, Container, Depth, DnxhrProfile, Export, ExportError, ProResProfile, Scene, Settings};
use gpui::{ClickEvent, Context, EventEmitter, FontWeight, IntoElement, ParentElement as _, Render, Styled as _, div};
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::progress::Progress as ProgressBar;
use theme::ActiveTheme as _;
use guard::SafeListener as _;
use serde_json::{Value, json};
use shader_graph::graph::{GraphProject, MAX_FPS};

/// The key of the output settings in the project's JSON, beside the layout and settings.
const KEY: &str = "export";
/// How often the job reads the export's progress.
const POLL: Duration = Duration::from_millis(100);
/// Seconds, the range a new project exports.
const DEFAULT_DURATION: f64 = 5.0;

const CONTAINERS: &[(&str, Container)] = &[("mov", Container::Mov), ("mkv", Container::Matroska)];

const CODECS: &[(&str, Codec)] = &[
    ("prores-proxy", Codec::ProRes(ProResProfile::Proxy)),
    ("prores-lt", Codec::ProRes(ProResProfile::Lt)),
    ("prores-422", Codec::ProRes(ProResProfile::Standard)),
    ("prores-422-hq", Codec::ProRes(ProResProfile::Hq)),
    ("prores-4444", Codec::ProRes(ProResProfile::P4444)),
    ("prores-4444-xq", Codec::ProRes(ProResProfile::P4444Xq)),
    ("dnxhr-lb", Codec::Dnxhr(DnxhrProfile::Lb)),
    ("dnxhr-sq", Codec::Dnxhr(DnxhrProfile::Sq)),
    ("dnxhr-hq", Codec::Dnxhr(DnxhrProfile::Hq)),
    ("dnxhr-hqx", Codec::Dnxhr(DnxhrProfile::Hqx)),
    ("dnxhr-444", Codec::Dnxhr(DnxhrProfile::P444)),
    ("ffv1-8", Codec::Ffv1(Depth::Bits8)),
    ("ffv1-16", Codec::Ffv1(Depth::Bits16)),
    ("png", Codec::Png),
    ("qtrle", Codec::QuickTimeAnimation),
];

/// What a video export writes. Stored with the project, so the file a project exports to is the one it was set up for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputSettings {
    pub container: Container,
    pub codec: Codec,
    /// Only kept where the codec has an alpha channel.
    pub alpha: bool,
    /// Seconds of the first frame.
    pub start: f64,
    /// Seconds.
    pub duration: f64,
    pub fps: f64,
}

impl OutputSettings {
    /// A new project's output: the default codec in MOV, over the first seconds at the project's frame rate.
    pub fn new(fps: f64) -> Self {
        Self {
            container: Container::Mov,
            codec: Codec::DEFAULT,
            alpha: false,
            start: 0.0,
            duration: DEFAULT_DURATION,
            fps,
        }
    }

    pub fn to_settings(self) -> Settings {
        Settings {
            container: self.container,
            codec: self.codec,
            alpha: self.alpha && self.codec.supports_alpha(),
            start: self.start,
            duration: self.duration,
            fps: self.fps,
        }
    }

    /// A codec the container can't hold falls back to the default codec.
    pub fn choose_container(&mut self, container: Container) {
        self.container = container;
        if !self.codec.supports_container(container) {
            self.codec = Codec::DEFAULT;
        }
    }

    pub fn choose_codec(&mut self, codec: Codec) {
        self.codec = codec;
    }

    /// Reads what a project stored. Anything missing or out of range takes its default; a value the
    /// project can't hold (a codec in a container it doesn't fit) falls back to the default codec.
    pub fn from_json(value: &Value, fps: f64) -> Option<Self> {
        let object = value.as_object()?;
        let container = named(CONTAINERS, object.get("container")).unwrap_or(Container::Mov);
        let codec = named(CODECS, object.get("codec"))
            .filter(|codec| codec.supports_container(container))
            .unwrap_or(Codec::DEFAULT);
        let alpha = object.get("alpha").and_then(Value::as_bool).unwrap_or(false) && codec.supports_alpha();
        let seconds = |key: &str, default: f64| {
            object
                .get(key)
                .and_then(Value::as_f64)
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map_or(default, |s| s.min(MAX_SECONDS))
        };
        let fps = object
            .get("fps")
            .and_then(Value::as_f64)
            .filter(|f| f.is_finite() && (1.0..=MAX_FPS).contains(f))
            .unwrap_or(fps);
        Some(Self {
            container,
            codec,
            alpha,
            start: seconds("start", 0.0),
            duration: seconds("duration", DEFAULT_DURATION),
            fps,
        })
    }

    pub fn to_json(self) -> Value {
        json!({
            "container": name_of(CONTAINERS, self.container),
            "codec": name_of(CODECS, self.codec),
            "alpha": self.alpha,
            "start": self.start,
            "duration": self.duration,
            "fps": self.fps,
        })
    }
}

fn named<T: Copy>(table: &[(&str, T)], key: Option<&Value>) -> Option<T> {
    let key = key?.as_str()?;
    table.iter().find(|(name, _)| *name == key).map(|(_, value)| *value)
}

fn name_of<T: Copy + PartialEq>(table: &[(&'static str, T)], value: T) -> &'static str {
    table.iter().find(|(_, v)| *v == value).map_or("", |(name, _)| name)
}

/// What a project saved under the output key, to read back with `from_stored`.
pub fn stored(project: &GraphProject) -> Option<Value> {
    project.extra.get(KEY).cloned()
}

/// The output settings a project saved, or the defaults at `fps` when there are none.
pub fn from_stored(stored: Option<&Value>, fps: f64) -> OutputSettings {
    stored.and_then(|value| OutputSettings::from_json(value, fps)).unwrap_or_else(|| OutputSettings::new(fps))
}

pub fn store(project: &mut GraphProject, output: &OutputSettings) {
    project.extra.insert(KEY.to_owned(), output.to_json());
}

/// A running export. Dropping it cancels the export, so closing its project stops the encode.
struct Running {
    export: Option<Export>,
    progress: export::Progress,
}

impl Running {
    /// The export once it has finished, for its result; `None` while it runs.
    fn poll(&mut self) -> Option<Export> {
        let export = self.export.as_ref()?;
        if export.is_finished() {
            return self.export.take();
        }
        self.progress = export.progress();
        None
    }

    fn cancel(&self) {
        if let Some(export) = &self.export {
            export.cancel();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub enum JobEvent {
    /// Once the file is written, the export failed, or it was cancelled.
    Finished(Result<PathBuf, ExportError>),
}

/// Runs one export at a time and shows its progress, with a cancel button, while it runs.
#[derive(Default)]
pub struct ExportJob {
    running: Option<Running>,
}

impl EventEmitter<JobEvent> for ExportJob {}

impl ExportJob {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Starts encoding `scene` into `path`. Errors are for the settings or the file, before any frame is drawn.
    pub fn start(
        &mut self,
        scene: Scene,
        settings: Settings,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), ExportError> {
        let export = Export::start(scene, settings, path)?;
        self.running = Some(Running { progress: export.progress(), export: Some(export) });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Some(Ok(finished)) = guard::guard("export poll", || this.update(cx, |job, cx| job.poll(cx))) else {
                    return;
                };
                let Some(export) = finished else { continue };
                let result = cx.background_executor().spawn(async move { export.wait() }).await;
                guard::guard("export finished", || {
                    this.update(cx, |job, cx| {
                        job.running = None;
                        cx.emit(JobEvent::Finished(result));
                        cx.notify();
                    })
                    .ok();
                });
                return;
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    fn poll(&mut self, cx: &mut Context<Self>) -> Option<Export> {
        let finished = self.running.as_mut()?.poll();
        if finished.is_none() {
            cx.notify();
        }
        finished
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(running) = &self.running {
            running.cancel();
        }
        cx.notify();
    }
}

impl Render for ExportJob {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("export progress", || {
            let Some(running) = &self.running else { return div().into_any_element() };
            let muted = cx.theme().colors.muted_foreground;
            let progress = running.progress;
            let fraction = if progress.total == 0 { 0.0 } else { progress.done as f32 / progress.total as f32 };
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(ProgressBar::new("export-progress").value(fraction * 100.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted)
                                .child(format!("Frame {} of {}", progress.done, progress.total)),
                        )
                        .child(
                            Button::new("export-cancel")
                                .label("Cancel")
                                .ghost()
                                .small()
                                .on_click(cx.safe_listener(|this, _: &ClickEvent, _, cx| this.cancel(cx))),
                        ),
                )
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_output_reads_back_the_same() {
        let output = OutputSettings {
            container: Container::Matroska,
            codec: Codec::Ffv1(Depth::Bits16),
            alpha: true,
            start: 1.5,
            duration: 2.25,
            fps: 24.0,
        };
        assert_eq!(OutputSettings::from_json(&output.to_json(), 60.0), Some(output));
        let prores = OutputSettings {
            container: Container::Mov,
            codec: Codec::ProRes(ProResProfile::P4444Xq),
            alpha: true,
            ..output
        };
        assert_eq!(OutputSettings::from_json(&prores.to_json(), 60.0), Some(prores));
    }

    #[test]
    fn a_project_stores_and_loads_its_output() {
        let mut project = GraphProject::default();
        assert_eq!(from_stored(stored(&project).as_ref(), 25.0), OutputSettings::new(25.0), "no output means the defaults");
        let output = OutputSettings { start: 3.0, ..OutputSettings::new(25.0) };
        store(&mut project, &output);
        assert_eq!(from_stored(stored(&project).as_ref(), 60.0), output);
        let json = project.to_json();
        let reloaded = GraphProject::from_json(&json).expect("loads");
        assert_eq!(from_stored(stored(&reloaded).as_ref(), 60.0), output, "the output survives a save");
    }

    #[test]
    fn unusable_values_fall_back_to_defaults() {
        let read = |value: Value| OutputSettings::from_json(&value, 30.0).expect("an object");
        assert_eq!(read(json!({})), OutputSettings::new(30.0));
        let odd = read(json!({ "container": "avi", "codec": "h264", "start": -2, "duration": "long", "fps": 10000 }));
        assert_eq!(odd, OutputSettings::new(30.0));
        // QuickTime Animation has no Matroska form, so the default codec takes its place.
        let wrong = read(json!({ "container": "mkv", "codec": "qtrle" }));
        assert_eq!(wrong.codec, Codec::DEFAULT);
        assert_eq!(OutputSettings::from_json(&json!("nope"), 30.0), None);
    }

    #[test]
    fn alpha_is_only_kept_where_the_codec_has_it() {
        let output = read_alpha("prores-422-hq");
        assert!(!output.alpha);
        assert!(read_alpha("png").alpha);
        assert!(!output.to_settings().alpha);
    }

    fn read_alpha(codec: &str) -> OutputSettings {
        OutputSettings::from_json(&json!({ "codec": codec, "alpha": true }), 30.0).expect("an object")
    }
}
