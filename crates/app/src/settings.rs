//! The settings dialog: the project's canvas, time and colour settings, and
//! the user's own appearance preferences.

use guard::SafeListener as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, deferred, div, prelude::FluentBuilder as _, px,
};
use runtime::Size;
use shader_graph::graph::{ColorSpace, MAX_CANVAS_SIZE, MAX_FPS, ProjectSettings};
use gpui_component::{Disableable as _, Selectable as _, Sizable as _, button::{Button, ButtonGroup, ButtonVariants as _}};
use theme::{ActiveTheme as _, InputEvent, NumberEvent, NumberField, TextInput};

use crate::editor::Editor;
use crate::playback::Playback;
use crate::preferences::{self, Preferences, ThemePreference};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Canvas,
    Time,
    Color,
    Appearance,
}

struct PageInfo {
    page: Page,
    label: &'static str,
    description: &'static str,
}

struct Group {
    label: &'static str,
    hint: &'static str,
    pages: &'static [PageInfo],
}

/// Pages by group: the project's settings, then the user's own.
const GROUPS: &[Group] = &[
    Group {
        label: "Project",
        hint: "Saved with the project",
        pages: &[
            PageInfo {
                page: Page::Canvas,
                label: "Canvas",
                description: "The resolution frames render and export at.",
            },
            PageInfo {
                page: Page::Time,
                label: "Time",
                description: "How time advances while playing and exporting.",
            },
            PageInfo {
                page: Page::Color,
                label: "Colour",
                description: "The colour space frames are shown and exported in.",
            },
        ],
    },
    Group {
        label: "Preferences",
        hint: "Saved on this computer",
        pages: &[PageInfo {
            page: Page::Appearance,
            label: "Appearance",
            description: "How the editor looks.",
        }],
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    Close,
}

pub struct SettingsDialog {
    editor: Entity<Editor>,
    playback: Entity<Playback>,
    /// The size frames render at, as last shown: the preview's while following it.
    shown: Size,
    page: Page,
    focus: FocusHandle,
    width: Entity<NumberField>,
    height: Entity<NumberField>,
    fps: Entity<NumberField>,
    font: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsDialog {}

impl Focusable for SettingsDialog {
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

impl SettingsDialog {
    pub fn new(
        editor: Entity<Editor>,
        playback: Entity<Playback>,
        window: &mut Window,
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
        let font = cx.new(|cx| {
            let mut input = TextInput::new(cx).placeholder(theme::INTERFACE_FONT);
            input.set_value(cx.global::<Preferences>().font.clone(), cx);
            input
        });
        let subscriptions = vec![
            // Playback changes every frame; only a change of the size needs a redraw.
            cx.observe(&playback, |this, playback, cx| {
                if playback.read(cx).render_size() != this.shown {
                    cx.notify();
                }
            }),
            cx.subscribe(&width, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("canvas width", || this.update(cx, |s| s.canvas.width = *v as u32));
            }),
            cx.subscribe(&height, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("canvas height", || this.update(cx, |s| s.canvas.height = *v as u32));
            }),
            cx.subscribe(&fps, |this, _, NumberEvent::Change(v), cx| {
                guard::guard("frame rate", || this.update(cx, |s| s.timing.fps = *v));
            }),
            cx.subscribe_in(&font, window, |_, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change | InputEvent::Submit | InputEvent::Blur) {
                    let text = input.read(cx).value().trim().to_owned();
                    guard::guard("interface font", || {
                        preferences::update(window, cx, |p| p.font = text);
                    });
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            editor,
            playback,
            shown,
            page: Page::Canvas,
            focus,
            width,
            height,
            fps,
            font,
            _subscriptions: subscriptions,
        }
    }

    fn update(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ProjectSettings)) {
        let mut settings = self.editor.read(cx).settings();
        f(&mut settings);
        self.editor.update(cx, |e, cx| e.update_settings(settings, cx));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Close);
    }

    fn field(&self, label: &str, description: Option<String>, child: impl IntoElement, cx: &App) -> AnyElement {
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
                    .child(div().text_size(text.sm).font_weight(gpui::FontWeight::MEDIUM).child(label.to_owned()))
                    .children(description.map(|d| {
                        div().text_size(text.xs).text_color(c.muted_foreground).child(d)
                    })),
            )
            .child(child)
            .into_any_element()
    }

    fn card(
        &self,
        id: impl Into<ElementId>,
        selected: bool,
        title: &str,
        detail: &str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
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
            .child(div().text_size(theme.text.sm).font_weight(gpui::FontWeight::MEDIUM).child(title.to_owned()))
            .child(div().text_size(theme.text.xs).text_color(c.muted_foreground).child(detail.to_owned()))
            .into_any_element()
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
        let size = self.field(
            "Size",
            Some(
                if follow {
                    "Frames render at the preview's size, filling it."
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
                    this.update(cx, |s| s.canvas.follow_preview = index == 1);
                }),
            ),
            cx,
        );
        let swap = {
            let (w, h) = (canvas.width, canvas.height);
            Button::new("swap-size")
                .ghost()
                .small()
                .label("⇄")
                .tooltip("Swap width and height")
                .on_click(cx.safe_listener(move |this, _, _, cx| {
                    this.update(cx, |s| {
                        s.canvas.width = h;
                        s.canvas.height = w;
                    });
                }))
        };
        let resolution = self.field(
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
                    self.card(
                        SharedString::from(format!("preset-{label}")),
                        width == canvas.width && height == canvas.height,
                        label,
                        &format!("{width} × {height}"),
                        cx,
                        move |this, cx| {
                            this.update(cx, |s| {
                                s.canvas.width = width;
                                s.canvas.height = height;
                            });
                        },
                    )
                })
                .collect();
            self.field("Presets", None, div().grid().grid_cols(3).gap_2().children(cards), cx)
        };
        let mut page = vec![size];
        let fixed = div().flex().flex_col().gap_6().when(follow, |d| d.opacity(0.5)).child(resolution).child(presets);
        // Greyed out and inert while the canvas follows the preview.
        page.push(if follow { fixed.occlude().into_any_element() } else { fixed.into_any_element() });
        page
    }

    fn time_page(&mut self, settings: &ProjectSettings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let timing = settings.timing;
        self.fps.update(cx, |f, cx| f.set_value(timing.fps, cx));
        let playback = self.field(
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
                    this.update(cx, |s| s.timing.realtime = index == 0);
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
                    this.update(cx, |s| s.timing.fps = f64::from(rate));
                }
            }),
        );
        let rate = self.field(
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

    fn color_page(&mut self, settings: &ProjectSettings, cx: &mut Context<Self>) -> Vec<AnyElement> {
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
                    self.card(
                        SharedString::from(format!("space-{title}")),
                        current == space,
                        title,
                        detail,
                        cx,
                        move |this, cx| this.update(cx, |s| s.color_space = space),
                    )
                }))
                .into_any_element(),
        ]
    }

    fn appearance_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = cx.global::<Preferences>().clone();
        let themes = [ThemePreference::Light, ThemePreference::Dark, ThemePreference::System];
        let selected = themes.iter().position(|&t| t == current.theme).unwrap_or(2);
        let theme = self.field(
            "Theme",
            None,
            segmented("theme", ["Light", "Dark", "System"], selected, move |index, window, cx| {
                if let Some(&choice) = themes.get(index) {
                    guard::guard("theme", || preferences::update(window, cx, |p| p.theme = choice));
                }
            }),
            cx,
        );
        let font = self.field(
            "Interface font",
            Some("A font family, e.g. “Inter”. Leave empty for the default.".to_owned()),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(320.0))
                        .px_1()
                        .border_1()
                        .border_color(cx.theme().colors.input)
                        .rounded(cx.theme().radius.md)
                        .child(self.font.clone()),
                )
                .child(
                    Button::new("reset-font")
                        .ghost()
                        .small()
                        .label("Reset")
                        .disabled(current.font.is_empty())
                        .on_click(cx.safe_listener(|this, _, window, cx| {
                            this.font.update(cx, |f, cx| f.set_value(String::new(), cx));
                            preferences::update(window, cx, |p| p.font.clear());
                        })),
                ),
            cx,
        );
        vec![theme, font]
    }

    fn nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_4()
            .w(px(208.0))
            .h_full()
            .p_3()
            .bg(c.muted.opacity(0.4))
            .border_r_1()
            .border_color(c.border)
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .text_size(theme.text.base)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .children(GROUPS.iter().map(|group| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .px_2()
                            .pb_1()
                            .text_size(theme.text.xs)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(c.muted_foreground)
                            .child(group.label),
                    )
                    .children(group.pages.iter().map(|info| {
                        let page = info.page;
                        let current = self.page == page;
                        div()
                            .id(SharedString::from(format!("settings-page-{}", info.label)))
                            .px_2()
                            .py(px(6.0))
                            .rounded(theme.radius.md)
                            .text_size(theme.text.sm)
                            .cursor_pointer()
                            .when(current, |d| d.bg(c.muted).font_weight(gpui::FontWeight::MEDIUM))
                            .hover(|s| s.bg(c.muted))
                            .on_click(cx.safe_listener(move |this, _: &ClickEvent, _, cx| {
                                this.page = page;
                                cx.notify();
                            }))
                            .child(info.label)
                    }))
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_size(px(11.0))
                            .text_color(c.muted_foreground.opacity(0.7))
                            .child(group.hint),
                    )
            }))
            .into_any_element()
    }
}

/// A row of buttons, one selected, that reports the index of the one clicked.
fn segmented(
    id: &'static str,
    labels: impl IntoIterator<Item = impl Into<SharedString>>,
    selected: usize,
    on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> ButtonGroup {
    let buttons = labels.into_iter().enumerate().map(|(index, label)| {
        Button::new((id, index)).label(label).selected(index == selected)
    });
    ButtonGroup::new(id).children(buttons).on_click(move |indices: &Vec<usize>, window, cx| {
        if let Some(&index) = indices.first() {
            on_change(index, window, cx);
        }
    })
}

/// A change handler for a `segmented` group, run on the dialog.
trait GroupListener {
    fn listener_for_group(
        &self,
        f: impl Fn(&mut SettingsDialog, usize, &mut Context<SettingsDialog>) + 'static,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static;
}

impl GroupListener for Context<'_, SettingsDialog> {
    fn listener_for_group(
        &self,
        f: impl Fn(&mut SettingsDialog, usize, &mut Context<SettingsDialog>) + 'static,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static {
        let weak = self.entity().downgrade();
        move |index, _, cx| {
            guard::guard("settings", || {
                weak.update(cx, |this, cx| f(this, index, cx)).ok();
            });
        }
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("settings", || {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let settings = self.editor.read(cx).settings();
        let info = GROUPS
            .iter()
            .flat_map(|g| g.pages)
            .find(|p| p.page == self.page)
            .map_or(("", ""), |p| (p.label, p.description));
        let body = match self.page {
            Page::Canvas => self.canvas_page(&settings, cx),
            Page::Time => self.time_page(&settings, cx),
            Page::Color => self.color_page(&settings, cx),
            Page::Appearance => self.appearance_page(cx),
        };
        let nav = self.nav(cx);
        let card = div()
            .id("settings-card")
            .track_focus(&self.focus)
            .key_context("SettingsDialog")
            .occlude()
            .flex()
            .w(px(768.0))
            .h(px(576.0))
            .max_w_full()
            .max_h_full()
            .overflow_hidden()
            .rounded(theme.radius.xl)
            .border_1()
            .border_color(c.foreground.opacity(0.1))
            .bg(c.card)
            .text_color(c.card_foreground)
            .text_size(theme.text.sm)
            .font_family(theme.font.clone())
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.safe_listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
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
                            .items_start()
                            .justify_between()
                            .gap_1()
                            .py_4()
                            .pl_4()
                            .pr_3()
                            .border_b_1()
                            .border_color(c.border)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(theme.text.base)
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(info.0),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme.text.sm)
                                            .text_color(c.muted_foreground)
                                            .child(info.1),
                                    ),
                            )
                            .child(
                                Button::new("close-settings")
                                    .ghost()
                                    .small()
                                    .icon(theme::IconName::X)
                                    .tooltip("Close")
                                    .on_click(cx.safe_listener(|this, _, _, cx| this.close(cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-body")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap_6()
                            .px_4()
                            .py_5()
                            .overflow_y_scroll()
                            .children(body),
                    ),
            );
        deferred(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p_8()
                .bg(gpui::black().opacity(0.5))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(|this, _, _, cx| this.close(cx)),
                )
                .child(card),
        )
        .with_priority(10)
    })
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::editor::EditorEvent;

    #[gpui::test]
    fn changes_reach_the_editor_and_are_announced(cx: &mut TestAppContext) {
        cx.update(theme::init);
        cx.update(preferences::init);
        let editor = std::rc::Rc::new(std::cell::RefCell::new(None));
        let shared = editor.clone();
        let (dialog, cx) = cx.add_window_view(move |window, cx| {
            let e = cx.new(|cx| Editor::new(window, cx));
            *shared.borrow_mut() = Some(e.clone());
            let playback = cx.new(|_| Playback::new());
            SettingsDialog::new(e, playback, window, cx)
        });
        let editor = editor.borrow().clone().expect("editor");
        let announced = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = announced.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
                if let EditorEvent::Settings(s) = event {
                    seen.borrow_mut().push(*s);
                }
            })
            .detach();
        });
        dialog.update(cx, |d, cx| {
            d.update(cx, |s| {
                s.timing.fps = 30.0;
                s.canvas.width = 640;
            })
        });
        let settings = editor.read_with(cx, |e, _| e.settings());
        assert_eq!(settings.timing.fps, 30.0);
        assert_eq!(settings.canvas.width, 640);
        assert_eq!(announced.borrow().len(), 1);
        dialog.update(cx, |d, cx| d.update(cx, |s| s.canvas.width = 0));
        assert_eq!(editor.read_with(cx, |e, _| e.settings().canvas.width), 640);
    }

    #[gpui::test]
    fn every_page_renders(cx: &mut TestAppContext) {
        cx.update(theme::init);
        cx.update(preferences::init);
        let (dialog, cx) = cx.add_window_view(|window, cx| {
            let e = cx.new(|cx| Editor::new(window, cx));
            let playback = cx.new(|_| Playback::new());
            SettingsDialog::new(e, playback, window, cx)
        });
        for page in [Page::Canvas, Page::Time, Page::Color, Page::Appearance] {
            dialog.update(cx, |d, cx| {
                d.page = page;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
    }

    #[test]
    fn aspect_ratios_reduce() {
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(aspect(1080, 1080), "1:1");
        assert_eq!(aspect(1280, 853), "1.50");
    }
}
