//! The Properties area: the window's project settings by section, edited live.
//! Canvas, Time and Colour are saved with the project's settings; Output is the
//! video export, saved beside them and run from its button.

use std::path::PathBuf;

use export::{Codec, Container, Depth, DnxhrProfile, ProResProfile, Scene};
use gpui::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::{Disableable as _, Selectable as _, Sizable as _};
use guard::SafeListener as _;
use runtime::Size;
use shader_graph::codegen::Program;
use shader_graph::graph::{ColorSpace, MAX_CANVAS_SIZE, MAX_FPS, ProjectSettings};
use theme::{ActiveTheme as _, Icon, IconName, NumberEvent, NumberField};

use crate::editor::Editor;
use crate::playback::Playback;
use crate::video_export::{ExportJob, JobEvent, OutputSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Canvas,
    Time,
    Colour,
    Output,
}

struct SectionInfo {
    section: Section,
    icon: IconName,
    label: &'static str,
    description: &'static str,
}

const SECTIONS: &[SectionInfo] = &[
    SectionInfo {
        section: Section::Canvas,
        icon: IconName::ArrowsOut,
        label: "Canvas",
        description: "The resolution frames render and export at.",
    },
    SectionInfo {
        section: Section::Time,
        icon: IconName::Timer,
        label: "Time",
        description: "How time advances while playing and exporting.",
    },
    SectionInfo {
        section: Section::Colour,
        icon: IconName::Palette,
        label: "Colour",
        description: "The colour space frames are shown and exported in.",
    },
    SectionInfo {
        section: Section::Output,
        icon: IconName::DownloadSimple,
        label: "Output",
        description: "The video export: its file type, codec, range and frame rate.",
    },
];

const RESOLUTION_PRESETS: &[(&str, u32, u32)] = &[
    ("HD", 1280, 720),
    ("Full HD", 1920, 1080),
    ("4K UHD", 3840, 2160),
    ("Square", 1080, 1080),
    ("Portrait", 1080, 1350),
    ("Vertical", 1080, 1920),
];

const FRAME_RATES: &[u32] = &[24, 25, 30, 50, 60, 120];

pub struct PropertiesPanel {
    editor: Entity<Editor>,
    playback: Entity<Playback>,
    job: Entity<ExportJob>,
    section: Section,
    /// The size frames render at, as last shown: the preview's while following it.
    shown: Size,
    focus: FocusHandle,
    width: Entity<NumberField>,
    height: Entity<NumberField>,
    fps: Entity<NumberField>,
    start: Entity<NumberField>,
    duration: Entity<NumberField>,
    export_fps: Entity<NumberField>,
    output: OutputSettings,
    error: Option<SharedString>,
    done: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for PropertiesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A ratio like 16:9, or a decimal for ones that don't reduce to small numbers.
fn aspect(width: u32, height: u32) -> String {
    let divisor = gcd(width, height).max(1);
    let (w, h) = (width / divisor, height / divisor);
    if w <= 32 && h <= 32 { format!("{w}:{h}") } else { format!("{:.2}", f64::from(width) / f64::from(height.max(1))) }
}

impl PropertiesPanel {
    /// `output` is what the project saved, or its defaults; `job` runs exports for the Output section.
    pub fn new(
        editor: Entity<Editor>,
        playback: Entity<Playback>,
        output: OutputSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = editor.read(cx).settings();
        let shown = playback.read(cx).render_size();
        let side = |value: u32, label: &'static str, cx: &mut Context<NumberField>| {
            NumberField::new(f64::from(value), cx)
                .label(label)
                .integer()
                .min(1.0)
                .max(f64::from(MAX_CANVAS_SIZE))
        };
        let width = cx.new(|cx| side(settings.canvas.width, "Width", cx));
        let height = cx.new(|cx| side(settings.canvas.height, "Height", cx));
        let fps = cx.new(|cx| {
            NumberField::new(settings.timing.fps, cx).label("FPS").step(1.0).min(1.0).max(MAX_FPS)
        });
        let start = cx.new(|cx| NumberField::new(output.start, cx).label("Start").min(0.0));
        let duration = cx.new(|cx| NumberField::new(output.duration, cx).label("Duration").min(0.0));
        let export_fps = cx.new(|cx| {
            NumberField::new(output.fps, cx).label("FPS").step(1.0).min(1.0).max(MAX_FPS)
        });
        let job = cx.new(|_| ExportJob::default());
        let subscriptions = vec![
            cx.observe(&playback, |this, playback, cx| {
                if playback.read(cx).render_size() != this.shown {
                    cx.notify();
                }
            }),
            cx.subscribe(&width, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("canvas width", || this.edit_project(cx, |s| s.canvas.width = *v as u32));
            }),
            cx.subscribe(&height, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("canvas height", || this.edit_project(cx, |s| s.canvas.height = *v as u32));
            }),
            cx.subscribe(&fps, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("frame rate", || this.edit_project(cx, |s| s.timing.fps = *v));
            }),
            cx.subscribe(&start, |this, _, NumberEvent::Change(v), cx| {
                this.output.start = *v;
                cx.notify();
            }),
            cx.subscribe(&duration, |this, _, NumberEvent::Change(v), cx| {
                this.output.duration = *v;
                cx.notify();
            }),
            cx.subscribe(&export_fps, |this, _, NumberEvent::Change(v), cx| {
                this.output.fps = *v;
                cx.notify();
            }),
            cx.subscribe(&job, |this, _, event: &JobEvent, cx| {
                guard::guard("export finished", || this.finished(event, cx));
            }),
        ];
        Self {
            editor,
            playback,
            job,
            section: Section::Canvas,
            shown,
            focus: cx.focus_handle(),
            width,
            height,
            fps,
            start,
            duration,
            export_fps,
            output,
            error: None,
            done: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    pub fn output(&self) -> OutputSettings {
        self.output
    }

    /// Replaces the output settings, as a project that was opened has them.
    pub fn set_output(&mut self, output: OutputSettings, cx: &mut Context<Self>) {
        self.output = output;
        for (field, value) in [(&self.start, output.start), (&self.duration, output.duration), (&self.export_fps, output.fps)] {
            field.update(cx, |f, cx| f.set_value(value, cx));
        }
        cx.notify();
    }

    fn edit_project(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ProjectSettings)) {
        let mut settings = self.editor.read(cx).settings();
        f(&mut settings);
        self.editor.update(cx, |e, cx| e.update_settings(settings, cx));
        cx.notify();
    }

    fn canvas_page(&mut self, settings: &ProjectSettings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let canvas = settings.canvas;
        let follow = canvas.follow_preview;
        self.shown = self.playback.read(cx).render_size();
        let (width, height) = if follow {
            (self.shown.width, self.shown.height)
        } else {
            (canvas.width, canvas.height)
        };
        for (field, value) in [(&self.width, width), (&self.height, height)] {
            field.update(cx, |f, cx| {
                f.set_value(f64::from(value), cx);
                f.set_read_only(follow, cx);
            });
        }
        let size = field(
            "Size",
            Some(
                if follow {
                    "Frames render at the size of the preview or output window used last, filling it."
                } else {
                    "Frames render at a fixed resolution, fitted into the preview."
                }
                .to_owned(),
            ),
            segmented(
                "canvas-size",
                ["Fixed size", "Follow preview"],
                usize::from(follow),
                cx.listener_for_group(|this, index, cx| {
                    this.edit_project(cx, |s| s.canvas.follow_preview = index == 1);
                }),
            ),
            cx,
        );
        let swap = {
            let (w, h) = (canvas.width, canvas.height);
            Button::new("swap-size")
                .ghost()
                .small()
                .icon(IconName::ArrowsLeftRight)
                .tooltip("Swap width and height")
                .on_click(cx.safe_listener(move |this, _, _, cx| {
                    this.edit_project(cx, |s| {
                        s.canvas.width = h;
                        s.canvas.height = w;
                    });
                }))
        };
        let resolution = field(
            "Resolution",
            Some(format!("In pixels, {}.", aspect(width, height))),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(128.0)).child(self.width.clone()))
                .child(swap)
                .child(div().w(px(128.0)).child(self.height.clone())),
            cx,
        );
        let presets = {
            let cards: Vec<AnyElement> = RESOLUTION_PRESETS
                .iter()
                .map(|&(label, width, height)| {
                    card(
                        SharedString::from(format!("preset-{label}")),
                        width == canvas.width && height == canvas.height,
                        label,
                        &format!("{width} × {height}"),
                        cx,
                        move |this, cx| {
                            this.edit_project(cx, |s| {
                                s.canvas.width = width;
                                s.canvas.height = height;
                            });
                        },
                    )
                })
                .collect();
            field("Presets", None, div().grid().grid_cols(2).gap_2().children(cards), cx)
        };
        let fixed = div().flex().flex_col().gap_6().when(follow, |d| d.opacity(0.5)).child(resolution).child(presets);
        // Greyed out and inert while the canvas follows the preview.
        vec![
            size,
            if follow { fixed.occlude().into_any_element() } else { fixed.into_any_element() },
        ]
    }

    fn time_page(&mut self, settings: &ProjectSettings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let timing = settings.timing;
        self.fps.update(cx, |f, cx| f.set_value(timing.fps, cx));
        let playback = field(
            "Playback",
            Some(
                if timing.realtime {
                    "Every display refresh draws a frame, and time follows the clock."
                } else {
                    "Frames advance at the frame rate, so each one looks the same however it's reached."
                }
                .to_owned(),
            ),
            segmented(
                "playback",
                ["Real time", "Fixed frame rate"],
                usize::from(!timing.realtime),
                cx.listener_for_group(|this, index, cx| {
                    this.edit_project(cx, |s| s.timing.realtime = index == 0);
                }),
            ),
            cx,
        );
        let current = FRAME_RATES.iter().position(|&r| f64::from(r) == timing.fps);
        // No preset is selected for a rate that isn't one; an out-of-range index selects none.
        let rates = segmented(
            "frame-rates",
            FRAME_RATES.iter().map(|rate| rate.to_string()),
            current.unwrap_or(usize::MAX),
            cx.listener_for_group(|this, index, cx| {
                if let Some(&rate) = FRAME_RATES.get(index) {
                    this.edit_project(cx, |s| s.timing.fps = f64::from(rate));
                }
            }),
        );
        let rate = field(
            "Frame rate",
            Some(
                if timing.realtime {
                    "Used for exports and stepping through frames."
                } else {
                    "Frames per second, for playback, exports and stepping."
                }
                .to_owned(),
            ),
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(div().w(px(128.0)).child(self.fps.clone()))
                .child(rates),
            cx,
        );
        vec![playback, rate]
    }

    fn colour_page(&mut self, settings: &ProjectSettings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let options = [
            (ColorSpace::Srgb, "sRGB", "The standard gamut, shown alike on every display."),
            (
                ColorSpace::DisplayP3,
                "Display P3",
                "A wider gamut: colours past sRGB's (linear values outside 0–1) stay saturated on displays that support it.",
            ),
        ];
        let current = settings.color_space;
        vec![
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(options.into_iter().map(|(space, title, detail)| {
                    card(
                        SharedString::from(format!("space-{title}")),
                        current == space,
                        title,
                        detail,
                        cx,
                        move |this, cx| this.edit_project(cx, |s| s.color_space = space),
                    )
                }))
                .into_any_element(),
        ]
    }

    /// Builds a choice of buttons, one per item, whose selection is saved by `choose`.
    fn choice_group<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        items: &[(T, &'static str, bool)],
        selected: impl Fn(T) -> bool,
        cx: &Context<Self>,
        choose: impl Fn(&mut PropertiesPanel, T, &mut Context<PropertiesPanel>) + 'static,
    ) -> ButtonGroup {
        let weak = cx.entity().downgrade();
        let running = self.job.read(cx).is_running();
        let options: Vec<T> = items.iter().map(|(value, _, _)| *value).collect();
        let buttons = items.iter().enumerate().map(|(index, (value, label, enabled))| {
            Button::new((id, index))
                .label(*label)
                .selected(selected(*value))
                .disabled(running || !enabled)
        });
        ButtonGroup::new(id).children(buttons).on_click(move |indices: &Vec<usize>, _, cx| {
            let Some(value) = indices.first().and_then(|&i| options.get(i).copied()) else { return };
            weak.update(cx, |panel, cx| choose(panel, value, cx)).ok();
        })
    }

    fn output_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().colors.muted_foreground;
        let destructive = cx.theme().colors.destructive;
        let running = self.job.read(cx).is_running();
        let output = self.output;
        let settings = output.to_settings();

        let size = self.playback.read(cx).render_size();
        let encoded = export::video_size(size);
        let frames = export::frame_count(&settings);
        let p3 = self.editor.read(cx).settings().color_space == ColorSpace::DisplayP3;
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
            })
            .when_some(output.codec.min_size().filter(|min| encoded.width < min.width || encoded.height < min.height), |d, min| {
                d.child(format!(
                    "{} needs frames of at least {} × {} pixels.",
                    output.codec.label(),
                    min.width,
                    min.height,
                ))
            });

        let containers = Container::ALL.map(|container| (container, container.label(), true));
        let container_group = self.choice_group(
            "export-container",
            &containers,
            |container| container == output.container,
            cx,
            |panel, container, cx| {
                panel.output.choose_container(container);
                cx.notify();
            },
        );

        let codecs = [
            Codec::ProRes(ProResProfile::Hq),
            Codec::Dnxhr(DnxhrProfile::Hq),
            Codec::Ffv1(Depth::Bits8),
            Codec::Png,
            Codec::QuickTimeAnimation,
        ]
        .map(|codec| (codec, codec.label(), codec.supports_container(output.container)));
        let codec_group = self.choice_group(
            "export-codec",
            &codecs,
            |codec| std::mem::discriminant(&codec) == std::mem::discriminant(&output.codec),
            cx,
            |panel, codec, cx| {
                if std::mem::discriminant(&panel.output.codec) != std::mem::discriminant(&codec) {
                    panel.output.choose_codec(codec);
                    cx.notify();
                }
            },
        );

        let codec_options = match output.codec {
            Codec::ProRes(selected) => {
                let profiles = ProResProfile::ALL.map(|profile| (profile, profile.label(), true));
                Some(field(
                    "Profile",
                    None,
                    self.choice_group(
                        "export-prores-profile",
                        &profiles,
                        move |profile| profile == selected,
                        cx,
                        |panel, profile, cx| {
                            panel.output.choose_codec(Codec::ProRes(profile));
                            cx.notify();
                        },
                    ),
                    cx,
                ))
            }
            Codec::Dnxhr(selected) => {
                let profiles = DnxhrProfile::ALL.map(|profile| (profile, profile.label(), true));
                Some(field(
                    "Profile",
                    None,
                    self.choice_group(
                        "export-dnxhr-profile",
                        &profiles,
                        move |profile| profile == selected,
                        cx,
                        |panel, profile, cx| {
                            panel.output.choose_codec(Codec::Dnxhr(profile));
                            cx.notify();
                        },
                    ),
                    cx,
                ))
            }
            Codec::Ffv1(selected) => {
                let depths = Depth::ALL.map(|depth| (depth, depth.label(), true));
                Some(field(
                    "Colour depth",
                    None,
                    self.choice_group(
                        "export-ffv1-depth",
                        &depths,
                        move |depth| depth == selected,
                        cx,
                        |panel, depth, cx| {
                            panel.output.choose_codec(Codec::Ffv1(depth));
                            cx.notify();
                        },
                    ),
                    cx,
                ))
            }
            Codec::Png | Codec::QuickTimeAnimation => None,
        };

        let alpha = output.codec.supports_alpha().then(|| {
            let weak = cx.entity().downgrade();
            Checkbox::new("export-alpha")
                .label("Keep the alpha channel")
                .checked(output.alpha)
                .disabled(running)
                .on_click(move |checked: &bool, _, cx| {
                    weak.update(cx, |panel, cx| {
                        panel.output.alpha = *checked;
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element()
        });

        let export = Button::new("export-video-start")
            .label("Export video…")
            .primary()
            .disabled(running)
            .on_click(cx.safe_listener(|this, _: &ClickEvent, window, cx| this.start_export(window, cx)));

        [
            description.into_any_element(),
            field("Container", None, container_group, cx),
            field("Codec", None, codec_group, cx),
        ]
        .into_iter()
        .chain(codec_options)
        .chain(alpha)
        .chain([
            field(
                "Range",
                Some("In seconds, rendered on the frame rate's grid.".to_owned()),
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.start.clone()))
                    .child(div().flex_1().child(self.duration.clone())),
                cx,
            ),
            field("Frame rate", None, div().w(px(160.0)).child(self.export_fps.clone()), cx),
            AnyView::from(self.job.clone()).cached(StyleRefinement::default()).into_any_element(),
            div()
                .text_xs()
                .text_color(muted)
                .children(self.done.clone())
                .into_any_element(),
            div()
                .text_xs()
                .text_color(destructive)
                .children(self.error.clone())
                .into_any_element(),
            div().flex().justify_end().child(export).into_any_element(),
        ])
        .collect()
    }

    /// Asks where to write the video, then starts the export there with the output settings.
    pub fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.read(cx).is_running() {
            return;
        }
        self.error = None;
        self.done = None;
        let program = match self.editor.read(cx).shader() {
            Ok(program) => program.clone(),
            Err(message) => {
                self.error = Some(message.clone().into());
                cx.notify();
                return;
            }
        };
        let directory = std::env::var_os("HOME").map_or_else(std::env::temp_dir, Into::into);
        let suggested = format!("webgpu-nodes.{}", self.output.container.extension());
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else { return };
            this.update_in(cx, |panel, _, cx| {
                guard::guard("start export", || panel.begin(program, path, cx));
            })
            .ok();
        })
        .detach();
    }

    fn begin(&mut self, program: Program, path: PathBuf, cx: &mut Context<Self>) {
        let size = self.playback.read(cx).render_size();
        let media = self.playback.update(cx, |playback, _| playback.export_media(&program.plan));
        let scene = Scene { program, media, size };
        let settings = self.output.to_settings();
        match self.job.update(cx, |job, cx| job.start(scene, settings, path, cx)) {
            Ok(()) => self.set_editable(false, cx),
            Err(error) => self.failed(&error, cx),
        }
        cx.notify();
    }

    fn finished(&mut self, event: &JobEvent, cx: &mut Context<Self>) {
        let JobEvent::Finished(result) = event;
        match result {
            Ok(path) => {
                log::info!("exported video to {}", path.display());
                self.done = Some(format!("Exported {}.", crate::project::file_name(path)).into());
            }
            // A cancel is the user's own choice, so it reports nothing.
            Err(export::ExportError::Cancelled) => {}
            Err(error) => self.failed(error, cx),
        }
        self.set_editable(true, cx);
        cx.notify();
    }

    fn failed(&mut self, error: &export::ExportError, cx: &mut Context<Self>) {
        log::error!("video export failed: {error:#}");
        self.error = Some(error.to_string().into());
        cx.notify();
    }

    fn set_editable(&self, editable: bool, cx: &mut Context<Self>) {
        for field in [&self.start, &self.duration, &self.export_fps] {
            field.update(cx, |field, cx| field.set_read_only(!editable, cx));
        }
    }

    fn nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .w(px(136.0))
            .h_full()
            .p_2()
            .bg(c.muted.opacity(0.4))
            .border_r_1()
            .border_color(c.border)
            .children(SECTIONS.iter().map(|info| {
                let section = info.section;
                let current = self.section == section;
                div()
                    .id(SharedString::from(format!("properties-section-{}", info.label)))
                    .px_2()
                    .py(px(6.0))
                    .rounded(theme.radius.md)
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(theme.text.sm)
                    .cursor_pointer()
                    .when(current, |d| d.bg(c.muted).font_weight(FontWeight::MEDIUM))
                    .hover(|s| s.bg(c.muted))
                    .on_click(cx.safe_listener(move |this, _: &ClickEvent, _, cx| {
                        this.section = section;
                        cx.notify();
                    }))
                    .child(Icon::new(info.icon).size(px(16.0)))
                    .child(info.label)
            }))
            .into_any_element()
    }
}

/// A button-like row of choices, one selected, that reports the index of the one clicked.
pub(crate) fn segmented(
    id: &'static str,
    labels: impl IntoIterator<Item = impl Into<SharedString>>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> ButtonGroup {
    let buttons = labels.into_iter().enumerate().map(|(index, label)| {
        Button::new((id, index)).label(label).selected(index == selected)
    });
    segmented_buttons(id, buttons, on_change)
}

pub(crate) fn segmented_with_icons(
    id: &'static str,
    items: impl IntoIterator<Item = (IconName, &'static str)>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> ButtonGroup {
    let buttons = items.into_iter().enumerate().map(|(index, (icon, label))| {
        Button::new((id, index)).icon(icon).label(label).selected(index == selected)
    });
    segmented_buttons(id, buttons, on_change)
}

fn segmented_buttons(
    id: &'static str,
    buttons: impl IntoIterator<Item = Button>,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> ButtonGroup {
    ButtonGroup::new(id).children(buttons).on_click(move |indices: &Vec<usize>, window, cx| {
        if let Some(&index) = indices.first() {
            on_change(index, window, cx);
        }
    })
}

/// A labelled control with an optional note under its label.
pub(crate) fn field(label: &str, description: Option<String>, child: impl IntoElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let text = cx.theme().text;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(text.sm).font_weight(FontWeight::MEDIUM).child(label.to_owned()))
                .children(description.map(|d| div().text_size(text.xs).text_color(c.muted_foreground).child(d))),
        )
        .child(child)
        .into_any_element()
}

pub(crate) fn card<V: 'static>(
    id: impl Into<ElementId>,
    selected: bool,
    title: &str,
    detail: &str,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let theme = cx.theme().clone();
    let c = &theme.colors;
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .items_start()
        .gap(px(2.0))
        .px_3()
        .py_2()
        .rounded(theme.radius.lg)
        .border_1()
        .border_color(if selected { c.primary } else { c.border })
        .when(selected, |d| d.bg(c.muted))
        .cursor_pointer()
        .hover(|s| s.bg(c.muted))
        .on_click(cx.safe_listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
        .child(div().text_size(theme.text.sm).font_weight(FontWeight::MEDIUM).child(title.to_owned()))
        .child(div().text_size(theme.text.xs).text_color(c.muted_foreground).child(detail.to_owned()))
        .into_any_element()
}

/// A change handler for a `segmented` group, run on the panel.
trait GroupListener {
    fn listener_for_group(
        &self,
        f: impl Fn(&mut PropertiesPanel, usize, &mut Context<PropertiesPanel>) + 'static,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static;
}

impl GroupListener for Context<'_, PropertiesPanel> {
    fn listener_for_group(
        &self,
        f: impl Fn(&mut PropertiesPanel, usize, &mut Context<PropertiesPanel>) + 'static,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static {
        let weak = self.entity().downgrade();
        move |index, _, cx| {
            guard::guard("properties", || {
                weak.update(cx, |this, cx| f(this, index, cx)).ok();
            });
        }
    }
}

impl Render for PropertiesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("properties", || {
            let theme = cx.theme().clone();
            let c = &theme.colors;
            let settings = self.editor.read(cx).settings();
            let info = SECTIONS.iter().find(|s| s.section == self.section);
            let (label, description) = info.map_or(("", ""), |s| (s.label, s.description));
            let body = match self.section {
                Section::Canvas => self.canvas_page(&settings, cx),
                Section::Time => self.time_page(&settings, cx),
                Section::Colour => self.colour_page(&settings, cx),
                Section::Output => self.output_page(cx),
            };
            let nav = self.nav(cx);
            div()
                .id("properties-panel")
                .debug_selector(|| "properties-panel".into())
                .track_focus(&self.focus)
                .key_context("Properties")
                .flex()
                .size_full()
                .overflow_hidden()
                .bg(c.card)
                .text_color(c.card_foreground)
                .text_size(theme.text.sm)
                .font_family(theme.font.clone())
                .child(nav)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .py_3()
                                .px_4()
                                .border_b_1()
                                .border_color(c.border)
                                .child(
                                    div()
                                        .text_size(theme.text.base)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(label),
                                )
                                .child(div().text_size(theme.text.xs).text_color(c.muted_foreground).child(description)),
                        )
                        .child(
                            div()
                                .id("properties-body")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .gap_6()
                                .px_4()
                                .py_4()
                                .overflow_y_scroll()
                                .children(body),
                        ),
                )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};
    use shader_graph::graph::ProjectSettings;

    use super::*;
    use crate::preferences;

    fn panel(cx: &mut TestAppContext, output: OutputSettings) -> (Entity<PropertiesPanel>, VisualTestContext) {
        cx.update(theme::init);
        cx.update(preferences::init);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let editor = cx.new(|cx| Editor::new(window, cx));
            let playback = cx.new(|_| Playback::new());
            PropertiesPanel::new(editor, playback, output, cx)
        });
        cx.run_until_parked();
        (panel, cx.clone())
    }

    fn project(panel: &Entity<PropertiesPanel>, cx: &mut VisualTestContext) -> ProjectSettings {
        panel.read_with(cx, |p, cx| p.editor.read(cx).settings())
    }

    /// A field edited by the user emits its change; setting its value in code doesn't.
    fn user_edits(field: &Entity<NumberField>, value: f64, cx: &mut VisualTestContext) {
        field.update(cx, |_, cx| cx.emit(NumberEvent::Change(value)));
    }

    #[gpui::test]
    fn project_edits_reach_the_editor_and_are_announced(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        let editor = panel.read_with(&cx, |p, _| p.editor.clone());
        let announced = Rc::new(RefCell::new(Vec::new()));
        let seen = announced.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &crate::editor::EditorEvent, _| {
                if let crate::editor::EditorEvent::Settings(s) = event {
                    seen.borrow_mut().push(*s);
                }
            })
            .detach();
        });
        panel.update(&mut cx, |p, cx| {
            p.edit_project(cx, |s| {
                s.timing.fps = 30.0;
                s.canvas.width = 640;
                s.color_space = ColorSpace::DisplayP3;
            })
        });
        let edited = project(&panel, &mut cx);
        assert_eq!((edited.timing.fps, edited.canvas.width, edited.color_space), (30.0, 640, ColorSpace::DisplayP3));
        assert_eq!(announced.borrow().len(), 1, "the change is announced once");
    }

    #[gpui::test]
    fn the_canvas_and_frame_rate_fields_edit_the_project(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        let (width, fps) = panel.read_with(&cx, |p, _| (p.width.clone(), p.fps.clone()));
        user_edits(&width, 640.0, &mut cx);
        user_edits(&fps, 30.0, &mut cx);
        let edited = project(&panel, &mut cx);
        assert_eq!((edited.canvas.width, edited.timing.fps), (640, 30.0));
        assert_eq!(
            panel.read_with(&cx, |p, _| p.output().fps),
            60.0,
            "the export's frame rate is its own setting"
        );
    }

    #[gpui::test]
    fn the_output_fields_edit_the_output(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        assert_eq!(panel.read_with(&cx, |p, _| p.output()), OutputSettings::new(60.0), "a new output starts at the project's frame rate");
        let (start, duration, fps) = panel.read_with(&cx, |p, _| (p.start.clone(), p.duration.clone(), p.export_fps.clone()));
        user_edits(&start, 2.5, &mut cx);
        user_edits(&duration, 1.0, &mut cx);
        user_edits(&fps, 24.0, &mut cx);
        let output = panel.read_with(&cx, |p, _| p.output());
        assert_eq!((output.start, output.duration, output.fps), (2.5, 1.0, 24.0));
        assert_eq!(project(&panel, &mut cx).timing.fps, 60.0, "the export's frame rate leaves the project's alone");
    }

    #[gpui::test]
    fn the_codec_choice_keeps_alpha_only_where_it_exists(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        panel.update(&mut cx, |p, cx| {
            p.output.choose_codec(Codec::ProRes(ProResProfile::P4444));
            p.output.alpha = true;
            cx.notify();
        });
        assert!(panel.read_with(&cx, |p, _| p.output().to_settings().alpha), "4444 keeps alpha");
        panel.update(&mut cx, |p, cx| {
            p.output.choose_codec(Codec::ProRes(ProResProfile::Hq));
            cx.notify();
        });
        assert!(!panel.read_with(&cx, |p, _| p.output().to_settings().alpha), "422 has no alpha channel");
    }

    #[gpui::test]
    fn set_output_shows_the_stored_settings(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        let stored = OutputSettings {
            container: Container::Matroska,
            codec: Codec::Png,
            alpha: true,
            start: 2.0,
            duration: 1.5,
            fps: 24.0,
        };
        panel.update(&mut cx, |p, cx| p.set_output(stored, cx));
        assert_eq!(panel.read_with(&cx, |p, _| p.output()), stored);
        let shown = panel.read_with(&cx, |p, cx| {
            (p.start.read(cx).value(), p.duration.read(cx).value(), p.export_fps.read(cx).value())
        });
        assert_eq!(shown, (2.0, 1.5, 24.0));
    }

    #[gpui::test]
    fn every_section_renders(cx: &mut TestAppContext) {
        let (panel, mut cx) = panel(cx, OutputSettings::new(60.0));
        for section in [Section::Canvas, Section::Time, Section::Colour, Section::Output] {
            panel.update(&mut cx, |p, cx| p.show(section, cx));
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        assert!(cx.debug_bounds("properties-panel").is_some());
    }

    #[test]
    fn aspect_ratios_reduce() {
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(aspect(1080, 1080), "1:1");
        assert_eq!(aspect(1280, 853), "1.50");
    }
}
