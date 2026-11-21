use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point,
    Render, ScrollEvent, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    canvas, div, img, point, prelude::FluentBuilder as _, px, size,
};
use guard::SafeListener as _;
use runtime::Size;
use theme::{ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName, Menu, MenuItem};

use crate::playback::{Playback, Shown, device_size};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 32.0;
const ZOOM_STEP: f32 = 1.2;
const WHEEL_SPEED: f32 = 0.002;
const PERCENTAGES: [f32; 4] = [0.5, 1.0, 2.0, 4.0];
const RESOLUTIONS: [(u32, u32); 6] = [
    (256, 256),
    (512, 512),
    (1024, 1024),
    (2048, 2048),
    (1280, 720),
    (1920, 1080),
];

/// A zoom (device pixels per canvas pixel) and the canvas centre's offset
/// from the pane's, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Manual {
    zoom: f32,
    x: f32,
    y: f32,
}

/// Where the canvas sits in a preview area. Until the user zooms or pans, the
/// canvas is fitted into the pane, following its resizes.
pub struct PreviewArea {
    playback: Entity<Playback>,
    manual: Option<Manual>,
    pane: Bounds<Pixels>,
    scale: f32,
    panning: Option<Point<Pixels>>,
    following: bool,
    _observe: Subscription,
    /// Forgets this area's size in the playback once it is released.
    _released: Option<Subscription>,
}

impl PreviewArea {
    pub fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&playback, |this, playback, cx| {
            let following = playback.read(cx).following();
            // The canvas fills the pane while following, so it starts fitted to it.
            if following && !this.following {
                this.manual = None;
            }
            this.following = following;
            cx.notify();
        });
        Self {
            playback,
            manual: None,
            pane: Bounds::default(),
            scale: 1.0,
            panning: None,
            following: false,
            _observe: observe,
            _released: None,
        }
    }

    pub fn zoom(&self, cx: &App) -> f32 {
        self.manual.map_or_else(|| self.fit_zoom(cx), |m| m.zoom)
    }

    fn resolution(&self, cx: &App) -> Size {
        self.playback.read(cx).render_size()
    }

    fn fit_zoom(&self, cx: &App) -> f32 {
        let res = self.resolution(cx);
        if res.width == 0 || res.height == 0 {
            return 1.0;
        }
        let pane: gpui::Size<f32> = size(self.pane.size.width.into(), self.pane.size.height.into());
        ((pane.width * self.scale) / res.width as f32)
            .min((pane.height * self.scale) / res.height as f32)
    }

    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.manual = None;
        cx.notify();
    }

    pub fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.manual = Some(Manual {
            zoom: 1.0,
            x: 0.0,
            y: 0.0,
        });
        cx.notify();
    }

    /// Zooms to `zoom`, keeping the canvas point under `anchor` (pane pixels) in place.
    pub fn zoom_to(&mut self, zoom: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let next = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let current = self.zoom(cx);
        let scale = next / current;
        let pane = (
            f32::from(self.pane.size.width),
            f32::from(self.pane.size.height),
        );
        let anchor = anchor.map_or((pane.0 / 2.0, pane.1 / 2.0), |a| (a.x.into(), a.y.into()));
        let offset = self.manual.map_or((0.0, 0.0), |m| (m.x, m.y));
        let centre = (pane.0 / 2.0 + offset.0, pane.1 / 2.0 + offset.1);
        self.manual = Some(Manual {
            zoom: next,
            x: anchor.0 - (anchor.0 - centre.0) * scale - pane.0 / 2.0,
            y: anchor.1 - (anchor.1 - centre.1) * scale - pane.1 / 2.0,
        });
        cx.notify();
    }

    pub fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.zoom_to(self.zoom(cx) * factor, anchor, cx);
    }

    fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let current = self.manual.unwrap_or(Manual {
            zoom: self.zoom(cx),
            x: 0.0,
            y: 0.0,
        });
        self.manual = Some(Manual {
            x: current.x + dx,
            y: current.y + dy,
            ..current
        });
        cx.notify();
    }

    /// The canvas's rectangle in the pane, on whole device pixels so no edge is drawn half-covered.
    fn rect(&self, cx: &App) -> Bounds<Pixels> {
        let res = self.resolution(cx);
        let zoom = self.zoom(cx);
        let snap = |v: f32| (v * self.scale).round() / self.scale;
        let (width, height) = (
            res.width as f32 * zoom / self.scale,
            res.height as f32 * zoom / self.scale,
        );
        let offset = self.manual.map_or((0.0, 0.0), |m| (m.x, m.y));
        let pane = (
            f32::from(self.pane.size.width),
            f32::from(self.pane.size.height),
        );
        Bounds::new(
            point(
                px(snap(pane.0 / 2.0 + offset.0 - width / 2.0)),
                px(snap(pane.1 / 2.0 + offset.1 - height / 2.0)),
            ),
            size(px(snap(width)), px(snap(height))),
        )
    }
}

impl Render for PreviewArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("PreviewArea", || {
            self.scale = window.scale_factor();
            let theme = cx.theme().clone();
            let playback = self.playback.read(cx);
            let shown = playback.shown();
            let failure = playback.failure.clone();
            let rect = self.rect(cx);
            let view = cx.entity();
            let id = view.entity_id();
            let playback = self.playback.clone();
            div()
                .id("preview-pane")
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(gpui::black())
                .cursor(if self.panning.is_some() {
                    gpui::CursorStyle::ClosedHand
                } else {
                    gpui::CursorStyle::OpenHand
                })
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            guard::guard("preview layout", || {
                                view.update(cx, |this, cx| {
                                    if this.pane != bounds {
                                        this.pane = bounds;
                                        cx.notify();
                                    }
                                });
                                let size = device_size(
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                    window.scale_factor(),
                                );
                                if let Some(size) = size {
                                    playback.update(cx, |p, cx| p.set_pane(id, size, window, cx));
                                }
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .children(shown.map(|shown| {
                    match shown {
                        #[cfg(target_os = "macos")]
                        Shown::Surface(frame) => gpui::surface(frame)
                            .object_fit(gpui::ObjectFit::Fill)
                            .absolute()
                            .left(rect.origin.x)
                            .top(rect.origin.y)
                            .w(rect.size.width)
                            .h(rect.size.height)
                            .into_any_element(),
                        Shown::Image(image) => img(image)
                            .absolute()
                            .left(rect.origin.x)
                            .top(rect.origin.y)
                            .w(rect.size.width)
                            .h(rect.size.height)
                            .into_any_element(),
                    }
                }))
                .on_scroll(
                    cx.safe_listener(move |this, event: &ScrollEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(20.0)).y;
                        let boost = if event.modifiers.control { 10.0 } else { 1.0 };
                        let factor = 2f32.powf(f32::from(delta) * WHEEL_SPEED * boost);
                        let anchor = event.position - this.pane.origin;
                        this.zoom_by(factor, Some(anchor), cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.safe_listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.panning = Some(event.position);
                        this.playback.update(cx, |p, cx| p.follow_pane(id, window, cx));
                        cx.notify();
                    }),
                )
                .on_mouse_move(cx.safe_listener(|this, event: &MouseMoveEvent, _, cx| {
                    let Some(last) = this.panning else {
                        return;
                    };
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.panning = None;
                        return;
                    }
                    this.pan_by(
                        (event.position.x - last.x).into(),
                        (event.position.y - last.y).into(),
                        cx,
                    );
                    this.panning = Some(event.position);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.safe_listener(|this, _, _, cx| {
                        this.panning = None;
                        cx.notify();
                    }),
                )
                .on_click(cx.safe_listener(|this, event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        let factor = if event.modifiers().shift { 0.5 } else { 2.0 };
                        let anchor = event.position() - this.pane.origin;
                        this.zoom_by(factor, Some(anchor), cx);
                    }
                }))
                .child(
                    div().absolute().left_2().bottom_2().child(
                        Button::new("fit-view")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Icon)
                            .icon(IconName::ArrowsIn)
                            .tooltip("Fit view (0)")
                            .on_click(cx.safe_listener(|this, _, _, cx| this.fit(cx))),
                    ),
                )
                .children(failure.map(|failure| {
                    let c = &theme.colors;
                    div()
                        .absolute()
                        .left(px(56.0))
                        .right_2()
                        .bottom_2()
                        .p_3()
                        .rounded(theme.radius.lg)
                        .bg(c.background.opacity(0.85))
                        .border_1()
                        .border_color(c.border)
                        .shadow_lg()
                        .text_size(theme.text.xs)
                        .text_color(c.foreground)
                        .child(
                            div()
                                .mb_1()
                                .text_size(theme.text.sm)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(c.destructive)
                                .child("Shader error"),
                        )
                        .child(failure.message)
                        .when(!failure.snippet.is_empty(), |d| {
                            d.child(
                                div()
                                    .mt_2()
                                    .p_2()
                                    .rounded(theme.radius.sm)
                                    .bg(c.muted)
                                    .font_family(theme.mono_font.clone())
                                    .children(failure.snippet.into_iter().map(
                                        |(number, text, at_fault)| {
                                            div()
                                                .whitespace_nowrap()
                                                .when(at_fault, |d| d.text_color(c.destructive))
                                                .child(format!(
                                                    "{number:>4} {} {text}",
                                                    if at_fault { ">" } else { " " }
                                                ))
                                        },
                                    )),
                            )
                        })
                }))
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Resolution,
    Zoom,
    Export,
}

/// The preview area's header controls: canvas size, zoom and export.
pub struct PreviewToolbar {
    area: Entity<PreviewArea>,
    playback: Entity<Playback>,
    open: Option<(Open, Point<Pixels>)>,
    _observe: [Subscription; 2],
}

impl PreviewToolbar {
    pub fn new(
        area: Entity<PreviewArea>,
        playback: Entity<Playback>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = [
            cx.observe(&area, |_, _, cx| cx.notify()),
            cx.observe(&playback, |_, _, cx| cx.notify()),
        ];
        Self {
            area,
            playback,
            open: None,
            _observe: observe,
        }
    }

    fn menu(&self, kind: Open, at: Point<Pixels>, cx: &mut Context<Self>) -> Menu {
        let this = cx.entity();
        let area = self.area.clone();
        let playback = self.playback.clone();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| {
                this.open = None;
                cx.notify();
            });
        };
        let menu = Menu::new("preview-menu").at(at).on_dismiss(dismiss);
        match kind {
            Open::Resolution => {
                // While following, the stored size has no effect, so it can't be picked.
                let following = playback.read(cx).following();
                RESOLUTIONS.iter().fold(menu, |menu, &(width, height)| {
                    let playback = playback.clone();
                    let current = playback.read(cx).resolution == Size::new(width, height);
                    menu.item(
                        MenuItem::new(format!("{width} × {height}"))
                            .checked(current)
                            .disabled(following)
                            .on_select(move |window, cx| {
                                playback.update(cx, |p, cx| {
                                    p.set_resolution(Size::new(width, height), window, cx)
                                });
                            }),
                    )
                })
            }
            Open::Zoom => {
                let act = |f: fn(&mut PreviewArea, &mut Context<PreviewArea>)| {
                    let area = area.clone();
                    move |_: &mut Window, cx: &mut App| area.update(cx, f)
                };
                let menu = menu
                    .item(
                        MenuItem::new("Zoom in")
                            .shortcut("+")
                            .on_select(act(|a, cx| a.zoom_by(ZOOM_STEP, None, cx))),
                    )
                    .item(
                        MenuItem::new("Zoom out")
                            .shortcut("−")
                            .on_select(act(|a, cx| a.zoom_by(1.0 / ZOOM_STEP, None, cx))),
                    )
                    .separator()
                    .item(
                        MenuItem::new("Fit view")
                            .shortcut("0")
                            .on_select(act(PreviewArea::fit)),
                    )
                    .item(
                        MenuItem::new("Actual size")
                            .shortcut("1")
                            .on_select(act(PreviewArea::actual_size)),
                    )
                    .separator();
                PERCENTAGES.iter().fold(menu, |menu, &zoom| {
                    let area = area.clone();
                    menu.item(MenuItem::new(format!("{}%", zoom * 100.0)).on_select(
                        move |_, cx| {
                            area.update(cx, |a, cx| a.zoom_to(zoom, None, cx));
                        },
                    ))
                })
            }
            Open::Export => menu.item(MenuItem::new("Export frame as PNG").on_select(
                move |window, cx| {
                    export_png(&playback, window, cx);
                },
            )),
        }
    }
}

fn export_png(playback: &Entity<Playback>, _: &mut Window, cx: &mut App) {
    let frame = playback.update(cx, |p, _| p.export_frame().and_then(|f| f.to_png()));
    let png = match frame {
        Ok(png) => png,
        Err(error) => {
            log::error!("export failed: {error:#}");
            return;
        }
    };
    let directory = std::env::var_os("HOME").map_or_else(std::env::temp_dir, Into::into);
    let path = cx.prompt_for_new_path(&directory, Some("frame.png"));
    cx.spawn(async move |_| {
        if let Ok(Ok(Some(path))) = path.await
            && let Err(error) = std::fs::write(&path, png)
        {
            log::error!("export to {} failed: {error:#}", path.display());
        }
    })
    .detach();
}

impl Render for PreviewToolbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("PreviewToolbar", || {
            let resolution = self.playback.read(cx).render_size();
            let zoom = self.area.read(cx).zoom(cx);
            let this = cx.entity();
            let open = move |kind: Open| {
                let this = this.clone();
                move |event: &ClickEvent, _: &mut Window, cx: &mut App| {
                    this.update(cx, |this, cx| {
                        this.open = Some((kind, event.position() + point(px(0.0), px(8.0))));
                        cx.notify();
                    });
                }
            };
            let is_open = |kind| self.open.is_some_and(|(k, _)| k == kind);
            let menu: Option<AnyElement> = self
                .open
                .map(|(kind, at)| self.menu(kind, at, cx).into_any_element());
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_end()
                .gap_1()
                .child(
                    Button::new("resolution")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Xs)
                        .label(format!("{} × {}", resolution.width, resolution.height))
                        .selected(is_open(Open::Resolution))
                        .tooltip("Canvas size")
                        .on_click(open(Open::Resolution)),
                )
                .child(
                    Button::new("zoom")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Xs)
                        .label(format!("{}%", (zoom * 100.0).round()))
                        .icon_end(IconName::CaretUpDown)
                        .selected(is_open(Open::Zoom))
                        .tooltip("Zoom")
                        .on_click(open(Open::Zoom)),
                )
                .child(
                    Button::new("export")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::DownloadSimple)
                        .selected(is_open(Open::Export))
                        .tooltip("Export")
                        .on_click(open(Open::Export)),
                )
                .children(menu)
        })
    }
}

pub fn new_area(
    playback: &Entity<Playback>,
    cx: &mut App,
) -> (Entity<PreviewArea>, Entity<PreviewToolbar>) {
    let area = cx.new(|cx| PreviewArea::new(playback.clone(), cx));
    let id = area.entity_id();
    let forget = {
        let playback = playback.clone();
        cx.observe_release(&area, move |_, cx| {
            playback.update(cx, |p, cx| p.forget_pane(id, cx));
        })
    };
    area.update(cx, |this, _| this._released = Some(forget));
    let toolbar = cx.new(|cx| PreviewToolbar::new(area.clone(), playback.clone(), cx));
    (area, toolbar)
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, Render, TestAppContext, Window};

    use super::*;

    /// A preview area in a box of the given width, which the test resizes.
    struct Harness {
        area: Entity<PreviewArea>,
        width: f32,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).h(px(300.0)).child(self.area.clone())
        }
    }

    fn redraw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn frames_follow_the_preview_area_while_following(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let playback = cx.new(|_| Playback::new());
            let area = cx.new(|cx| PreviewArea::new(playback, cx));
            Harness { area, width: 400.0 }
        });
        let playback = harness.read_with(cx, |h, cx| h.area.read(cx).playback.clone());
        let stored = Size::new(1920, 1080);
        cx.update(|window, cx| {
            playback.update(cx, |p, cx| p.set_canvas(true, stored, window, cx));
        });
        redraw(cx);

        let scale = cx.update(|window, _| window.scale_factor());
        let size = |cx: &gpui::VisualTestContext| playback.read_with(cx, |p, _| p.render_size());
        assert_eq!(size(cx), device_size(400.0, 300.0, scale).expect("a pane"));

        harness.update(cx, |h, cx| {
            h.width = 500.0;
            cx.notify();
        });
        redraw(cx);
        assert_eq!(size(cx), device_size(500.0, 300.0, scale).expect("a pane"));

        cx.update(|window, cx| {
            playback.update(cx, |p, cx| p.set_canvas(false, stored, window, cx));
        });
        redraw(cx);
        assert_eq!(size(cx), stored);
    }
}
