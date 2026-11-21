//! The export video dialog: the original's VideoExportDialog. Choosing the
//! file starts an export on the export crate's thread; the dialog polls its
//! progress until the file is written, failed or cancelled.

use std::path::PathBuf;
use std::time::Duration;

use export::{Export, ExportError, Format, Quality, Scene, Settings};
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_component::dialog::Dialog;
use gpui_component::progress::Progress as ProgressBar;
use gpui_component::{Disableable as _, Selectable as _, WindowExt as _};
use guard::SafeListener as _;
use shader_graph::codegen::Program;
use shader_graph::graph::{ColorSpace, MAX_FPS};
use theme::{ActiveTheme as _, NumberField};

use crate::editor::Editor;
use crate::playback::Playback;

/// How often the dialog reads the export's progress.
const POLL: Duration = Duration::from_millis(100);
const WIDTH: f32 = 448.0;
const FORMATS: [Format; 2] = [Format::Mp4, Format::WebM];
const QUALITIES: [Quality; 3] = [Quality::Medium, Quality::High, Quality::VeryHigh];

/// Opens the dialog on the window. Each opening starts from the project's frame rate.
pub fn open(editor: Entity<Editor>, playback: Entity<Playback>, window: &mut Window, cx: &mut App) -> Entity<ExportForm> {
    let fps = editor.read(cx).settings().timing.fps;
    let form = cx.new(|cx| ExportForm::new(editor, playback, fps, cx));
    let shown = form.clone();
    window.open_dialog(cx, move |dialog: Dialog, _, _| {
        dialog.title("Export video").w(px(WIDTH)).child(shown.clone())
    });
    form
}

/// The dialog's contents. Dropping it cancels a running export, which is how closing the dialog stops it.
pub struct ExportForm {
    editor: Entity<Editor>,
    playback: Entity<Playback>,
    format: Format,
    quality: Quality,
    start: Entity<NumberField>,
    duration: Entity<NumberField>,
    fps: Entity<NumberField>,
    running: Option<Running>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

struct Running {
    export: Export,
    done: u64,
    total: u64,
}

impl ExportForm {
    fn new(editor: Entity<Editor>, playback: Entity<Playback>, fps: f64, cx: &mut Context<Self>) -> Self {
        let start = cx.new(|cx| NumberField::new(0.0, cx).label("Start").min(0.0));
        let duration = cx.new(|cx| NumberField::new(5.0, cx).label("Duration").min(0.0));
        let fps = cx.new(|cx| NumberField::new(fps, cx).label("FPS").step(1.0).min(1.0).max(MAX_FPS));
        let subscriptions = vec![
            cx.observe(&start, |_, _, cx| cx.notify()),
            cx.observe(&duration, |_, _, cx| cx.notify()),
            cx.observe(&fps, |_, _, cx| cx.notify()),
        ];
        Self {
            editor,
            playback,
            format: Format::Mp4,
            quality: Quality::High,
            start,
            duration,
            fps,
            running: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn settings(&self, cx: &App) -> Settings {
        Settings {
            format: self.format,
            quality: self.quality,
            start: self.start.read(cx).value(),
            duration: self.duration.read(cx).value(),
            fps: self.fps.read(cx).value(),
        }
    }

    fn set_editable(&self, editable: bool, cx: &mut Context<Self>) {
        for field in [&self.start, &self.duration, &self.fps] {
            field.update(cx, |field, cx| field.set_read_only(!editable, cx));
        }
    }

    /// Asks where to write the file, then starts the export there.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        self.error = None;
        let program = match self.editor.read(cx).shader() {
            Ok(program) => program.clone(),
            Err(message) => {
                self.error = Some(message.clone().into());
                cx.notify();
                return;
            }
        };
        let directory = std::env::var_os("HOME").map_or_else(std::env::temp_dir, Into::into);
        let suggested = format!("webgpu-nodes.{}", self.format.extension());
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else { return };
            this.update_in(cx, |form, window, cx| {
                guard::guard("start export", || form.begin(program, path, window, cx));
            })
            .ok();
        })
        .detach();
    }

    fn begin(&mut self, program: Program, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let size = self.playback.read(cx).render_size();
        let media = self.playback.update(cx, |playback, _| playback.export_media(&program.plan));
        let scene = Scene { program, media, size };
        match Export::start(scene, settings, path) {
            Ok(export) => {
                let total = export.progress().total;
                self.running = Some(Running { export, done: 0, total });
                self.set_editable(false, cx);
                cx.spawn_in(window, async move |this, cx| {
                    loop {
                        cx.background_executor().timer(POLL).await;
                        let Ok(finished) = this.update_in(cx, |form, _, cx| form.poll(cx)) else { return };
                        let Some(export) = finished else { continue };
                        let result = cx.background_executor().spawn(async move { export.wait() }).await;
                        this.update_in(cx, |form, window, cx| {
                            guard::guard("finish export", || form.finish(result, window, cx));
                        })
                        .ok();
                        return;
                    }
                })
                .detach();
            }
            Err(error) => self.failed(&error, cx),
        }
        cx.notify();
    }

    /// Reads the export's progress. Returns the export once it has finished.
    fn poll(&mut self, cx: &mut Context<Self>) -> Option<Export> {
        let running = self.running.as_mut()?;
        if !running.export.is_finished() {
            let progress = running.export.progress();
            running.done = progress.done;
            running.total = progress.total;
            cx.notify();
            return None;
        }
        self.running.take().map(|running| running.export)
    }

    fn finish(&mut self, result: Result<PathBuf, ExportError>, window: &mut Window, cx: &mut Context<Self>) {
        self.running = None;
        match result {
            Ok(path) => {
                log::info!("exported video to {}", path.display());
                window.close_dialog(cx);
            }
            // Only a cancel gets here, and cancelling closes the dialog.
            Err(ExportError::Cancelled) => {}
            Err(error) => {
                self.failed(&error, cx);
                self.set_editable(true, cx);
            }
        }
        cx.notify();
    }

    fn failed(&mut self, error: &ExportError, cx: &mut Context<Self>) {
        log::error!("video export failed: {error:#}");
        self.error = Some(error.to_string().into());
        cx.notify();
    }

    fn choose_format(&mut self, format: Format, cx: &mut Context<Self>) {
        self.format = format;
        cx.notify();
    }

    fn choose_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.quality = quality;
        cx.notify();
    }

    fn choice_group<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        items: &[(T, &'static str, bool)],
        selected: T,
        cx: &Context<Self>,
        choose: fn(&mut ExportForm, T, &mut Context<ExportForm>),
    ) -> ButtonGroup {
        let weak = cx.entity().downgrade();
        let running = self.running.is_some();
        let options: Vec<T> = items.iter().map(|(value, _, _)| *value).collect();
        let buttons = items.iter().enumerate().map(|(index, (value, label, enabled))| {
            Button::new((id, index))
                .label(*label)
                .selected(*value == selected)
                .disabled(running || !enabled)
        });
        ButtonGroup::new(id).children(buttons).on_click(move |indices: &Vec<usize>, _, cx| {
            let Some(value) = indices.first().and_then(|&i| options.get(i).copied()) else { return };
            weak.update(cx, |form, cx| choose(form, value, cx)).ok();
        })
    }

    fn body(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted = cx.theme().colors.muted_foreground;
        let destructive = cx.theme().colors.destructive;
        let running = self.running.is_some();

        let size = self.playback.read(cx).render_size();
        let settings = self.settings(cx);
        let encoded = export::video_size(size);
        let frames = export::frame_count(&settings);
        let p3 = self.editor.read(cx).settings().color_space == ColorSpace::DisplayP3;

        let formats = FORMATS.map(|format| (format, format.label(), format == Format::Mp4));
        let qualities = QUALITIES.map(|q| (q, q.label(), true));

        let description = div()
            .flex()
            .flex_col()
            .gap_1()
            .text_sm()
            .text_color(muted)
            .child(format!(
                "{} × {}, {frames} {}, in sRGB{}.",
                encoded.width,
                encoded.height,
                if frames == 1 { "frame" } else { "frames" },
                if p3 { " (colours past it are clipped)" } else { "" },
            ))
            .when(encoded != size, |d| {
                d.child(format!(
                    "Encoders need even sizes, so the canvas ({} × {}) loses its last row or column.",
                    size.width, size.height,
                ))
            });

        let progress = self.running.as_ref().map(|running| {
            let fraction = if running.total == 0 { 0.0 } else { running.done as f32 / running.total as f32 };
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(ProgressBar::new("export-progress").value(fraction * 100.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Frame {} of {}", running.done, running.total)),
                )
        });

        let export = Button::new("export-video-start")
            .label("Export")
            .primary()
            .disabled(running)
            .on_click(cx.safe_listener(|this, _: &ClickEvent, window, cx| this.start(window, cx)));
        let cancel = Button::new("export-video-cancel")
            .label("Cancel")
            .ghost()
            .on_click(|_: &ClickEvent, window, cx| window.close_dialog(cx));

        // WebM is listed as the original's dialog does, but can't be encoded yet.
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(description)
            .child(field(
                "Format",
                Some("WebM export isn't available yet."),
                muted,
                self.choice_group("export-format", &formats, self.format, cx, ExportForm::choose_format),
            ))
            .child(field(
                "Quality",
                None,
                muted,
                self.choice_group("export-quality", &qualities, self.quality, cx, ExportForm::choose_quality),
            ))
            .child(field(
                "Range",
                Some("In seconds, rendered on the frame rate's grid."),
                muted,
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.start.clone()))
                    .child(div().flex_1().child(self.duration.clone())),
            ))
            .child(field("Frame rate", None, muted, div().w(px(160.0)).child(self.fps.clone())))
            .children(progress)
            .children(self.error.clone().map(|error| div().text_xs().text_color(destructive).child(error)))
            .child(div().flex().justify_end().gap_2().child(cancel).child(export))
    }
}

fn field(
    label: &'static str,
    note: Option<&'static str>,
    muted: gpui::Hsla,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
        .children(note.map(|note| div().text_xs().text_color(muted).child(note)))
        .child(control)
}

impl Render for ExportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("export video", || self.body(cx))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Render, TestAppContext, VisualTestContext};
    use gpui_component::Root;
    use shader_graph::graph::ProjectSettings;

    use super::*;

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|_| Host);
            Root::new(gpui::AnyView::from(host), window, cx)
        });
        cx.clone()
    }

    #[gpui::test]
    fn the_dialog_opens_at_the_project_frame_rate_and_clamps_its_fields(cx: &mut TestAppContext) {
        let mut cx = window(cx);
        let (editor, playback) = cx.update(|window, cx| {
            (cx.new(|cx| Editor::new(window, cx)), cx.new(|_| Playback::new()))
        });
        let form = cx.update(|window, cx| open(editor, playback, window, cx));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)), "the dialog opens");

        let fps = form.read_with(&cx, |form, cx| form.fps.read(cx).value());
        assert_eq!(fps, ProjectSettings::default().timing.fps, "each opening starts at the project's frame rate");

        form.update(&mut cx, |form, cx| {
            form.start.update(cx, |field, cx| field.set_value(-3.0, cx));
            form.fps.update(cx, |field, cx| field.set_value(10_000.0, cx));
            form.duration.update(cx, |field, cx| field.set_value(0.0, cx));
        });
        let settings = form.read_with(&cx, |form, cx| form.settings(cx));
        assert_eq!(settings.start, 0.0, "the start can't be negative");
        assert_eq!(settings.fps, MAX_FPS, "the frame rate is capped");
        assert_eq!(settings.duration, 0.0);
        assert_eq!(export::frame_count(&settings), 1, "a zero duration still exports a frame");
    }
}
