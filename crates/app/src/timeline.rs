use guard::SafeListener as _;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, Window, div, point, prelude::FluentBuilder as _, px,
};
use runtime::Timing;
use gpui_component::{Selectable as _, Sizable as _, button::{Button, ButtonVariants as _}};
use theme::{ActiveTheme as _, IconName, Menu, MenuItem, NumberEvent, NumberField};

use crate::playback::{Playback, PlaybackEvent};

/// How far below the target rate counts as falling behind, past what timing jitter explains.
const SLOW_RATIO: f64 = 0.95;
/// The readouts are cached views, which are laid out at a fixed size rather
/// than measured, so these hold the widest text they show.
const FRAME_FIELD_WIDTH: f32 = 144.0;
const TIME_WIDTH: f32 = 64.0;
const READOUT_WIDTH: f32 = 8.0 + FRAME_FIELD_WIDTH + 4.0 + TIME_WIDTH;
const TIMING_WIDTH: f32 = 320.0;
const FRAME_RATES: [f64; 5] = [24.0, 30.0, 60.0, 120.0, 144.0];

/// The playback controls of a timeline area's header. Only the play state is
/// kept here, so the bar re-renders when it changes rather than with every
/// frame; the readouts observe playback themselves.
pub struct TimelineBar {
    playback: Entity<Playback>,
    playing: bool,
    readout: Entity<TimelineReadout>,
    timing: Entity<TimingControl>,
    _subscription: Subscription,
}

impl TimelineBar {
    pub fn new(playback: Entity<Playback>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playing = playback.read(cx).player.playing();
        let readout = cx.new(|cx| TimelineReadout::new(playback.clone(), window, cx));
        let timing = cx.new(|cx| TimingControl::new(playback.clone(), cx));
        let subscription = cx.subscribe(&playback, |this, playback, _: &PlaybackEvent, cx| {
            let playing = playback.read(cx).player.playing();
            if playing != this.playing {
                this.playing = playing;
                cx.notify();
            }
        });
        Self {
            playback,
            playing,
            readout,
            timing,
            _subscription: subscription,
        }
    }
}

/// The frame field and the time, which change with every drawn frame.
struct TimelineReadout {
    playback: Entity<Playback>,
    frame: Entity<NumberField>,
    _subscriptions: [Subscription; 2],
}

impl TimelineReadout {
    fn new(playback: Entity<Playback>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let frame_now = playback.read(cx).player.clock.frame;
        let frame = cx.new(|cx| {
            NumberField::new(frame_now as f64, cx)
                .label("Frame")
                .integer()
                .min(0.0)
        });
        let subscriptions = [
            cx.observe(&playback, |this, playback, cx| {
                let frame = playback.read(cx).player.clock.frame;
                this.frame.update(cx, |field, cx| field.set_value(frame as f64, cx));
                cx.notify();
            }),
            cx.subscribe_in(
                &frame,
                window,
                |this, _, event: &NumberEvent, window, cx| {
                    let NumberEvent::Change(frame) = *event;
                    this.playback.update(cx, |p, cx| {
                        let fps = p.timing().fps;
                        p.seek(frame / fps, window, cx);
                    });
                },
            ),
        ];
        Self {
            playback,
            frame,
            _subscriptions: subscriptions,
        }
    }
}

/// The timing button, showing the rate the frames are drawn at, and the menu it opens.
struct TimingControl {
    playback: Entity<Playback>,
    menu: Option<Point<Pixels>>,
    _observe: Subscription,
}

impl TimingControl {
    fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&playback, |_, _, cx| cx.notify());
        Self {
            playback,
            menu: None,
            _observe: observe,
        }
    }

    fn menu(&self, at: Point<Pixels>, cx: &mut Context<Self>) -> Menu {
        let this = cx.entity();
        let timing = self.playback.read(cx).timing();
        let menu = Menu::new("timing-menu").at(at).on_dismiss(move |_, cx| {
            this.update(cx, |this, cx| {
                this.menu = None;
                cx.notify();
            });
        });
        let set = |playback: &Entity<Playback>, timing: Timing| {
            let playback = playback.clone();
            move |window: &mut Window, cx: &mut App| {
                playback.update(cx, |p, cx| p.set_timing(timing, window, cx));
            }
        };
        let menu = FRAME_RATES.iter().fold(menu, |menu, &fps| {
            menu.item(
                MenuItem::new(format!("{fps} fps"))
                    .checked(timing.fps == fps)
                    .on_select(set(&self.playback, Timing { fps, ..timing })),
            )
        });
        menu.separator().item(
            MenuItem::new("Real time")
                .checked(timing.realtime)
                .on_select(set(
                    &self.playback,
                    Timing {
                        realtime: !timing.realtime,
                        ..timing
                    },
                )),
        )
    }
}

fn format_ms(seconds: f64) -> String {
    format!("{:.1}", seconds * 1000.0)
}

fn format_time(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:05.2}", minutes as u64, seconds - minutes * 60.0)
}

impl Render for TimelineBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { guard::render("timeline", || {
        let playing = self.playing;
        let playback = self.playback.clone();
        let act = move |f: fn(&mut Playback, &mut Window, &mut Context<Playback>)| {
            let playback = playback.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                playback.update(cx, |p, cx| f(p, window, cx));
            }
        };

        // The readouts take h_full, which is 0 unless this row has a definite height.
        div()
            .flex()
            .size_full()
            .items_center()
            .gap_1()
            .child(
                Button::new("to-start")
                    .ghost()
                    .small()
                    .icon(IconName::SkipBack)
                    .tooltip("Back to the start (Home)")
                    .on_click(act(|p, w, cx| p.seek(0.0, w, cx))),
            )
            .child(
                Button::new("previous")
                    .ghost()
                    .small()
                    .icon(IconName::CaretLeft)
                    .tooltip("Previous frame (,)")
                    .on_click(act(|p, w, cx| p.step(-1, w, cx))),
            )
            .child(
                Button::new("play")
                    .ghost()
                    .small()
                    .icon(if playing {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .tooltip(if playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    })
                    .on_click(act(Playback::toggle)),
            )
            .child(
                Button::new("next")
                    .ghost()
                    .small()
                    .icon(IconName::CaretRight)
                    .tooltip("Next frame (.)")
                    .on_click(act(|p, w, cx| p.step(1, w, cx))),
            )
            .child(
                self.readout
                    .clone()
                    .cached(StyleRefinement::default().flex_none().w(px(READOUT_WIDTH)).h_full()),
            )
            .child(div().flex_1())
            .child(
                self.timing
                    .clone()
                    .cached(StyleRefinement::default().flex_shrink_1().min_w_0().w(px(TIMING_WIDTH)).h_full()),
            )
    })
    }
}

impl Render for TimelineReadout {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("timeline readout", || {
        let theme = cx.theme().clone();
        let time = self.playback.read(cx).player.clock.time;
        div()
            .size_full()
            .flex()
            .items_center()
            .gap_1()
            .child(div().ml_2().w(px(FRAME_FIELD_WIDTH)).flex_none().child(self.frame.clone()))
            .child(
                div()
                    .w(px(TIME_WIDTH))
                    .px_2()
                    .text_size(theme.text.xs)
                    .text_color(theme.colors.muted_foreground)
                    .child(format_time(time)),
            )
    })
    }
}

impl Render for TimingControl {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("timing control", || {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let playback = self.playback.read(cx);
        let timing = playback.timing();
        let rate = playback.player.meter.rate;
        let refresh = playback.refresh_rate.map(f64::round);
        let target = playback.target_rate();
        let slow = rate.is_some_and(|r| r.fps < target * SLOW_RATIO);
        let gpu_slow = rate
            .and_then(|r| r.gpu_time)
            .is_some_and(|t| t > 1.0 / target);

        let mut hint = Vec::new();
        if let Some(rate) = rate {
            hint.push(format!(
                "Longest frame: {} ms",
                format_ms(rate.worst_frame_time)
            ));
        }
        if let Some(refresh) = refresh {
            if timing.realtime {
                hint.push(format!("Drawing at the display's {refresh} Hz"));
            } else if timing.fps > refresh {
                hint.push(format!("The display refreshes at only {refresh} Hz"));
            }
        }
        hint.push("Time settings".into());

        let stat = |text: String, bad: bool| {
            div()
                .text_color(if bad {
                    c.destructive
                } else {
                    c.muted_foreground
                })
                .child(text)
        };
        let label: Vec<SharedString> = match rate {
            Some(rate) => {
                let mut parts = vec![
                    format!("{}/{} fps", rate.fps.round(), target),
                    format!("{} ms", format_ms(rate.frame_time)),
                ];
                if let Some(gpu) = rate.gpu_time {
                    parts.push(format!("GPU {} ms", format_ms(gpu)));
                }
                parts.into_iter().map(Into::into).collect()
            }
            None => vec![format!("{} fps", timing.fps).into()],
        };

        div()
            .debug_selector(|| "timing-control".into())
            .size_full()
            .flex()
            .items_center()
            .justify_end()
            .overflow_hidden()
            .child(
                Button::new("timing")
                    .ghost()
                    .xsmall()
                    .tooltip(hint.join("\n"))
                    .selected(self.menu.is_some())
                    .on_click(cx.safe_listener(|this, event: &ClickEvent, _, cx| {
                        this.menu = Some(event.position() + point(px(0.0), px(8.0)));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_color(c.muted_foreground)
                            .child(stat(label[0].to_string(), slow))
                            .children(label.get(1).map(|l| stat(format!("· {l}"), slow)))
                            .children(label.get(2).map(|l| stat(format!("· {l}"), gpu_slow)))
                            .when(timing.realtime, |d| d.child("· real time")),
                    ),
            )
            .children(self.menu.map(|at| self.menu(at, cx)))
    })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Entity, IntoElement, Render, StyleRefinement, TestAppContext, Window};

    use super::*;

    /// A typical width for the timeline's header, and the header's height.
    const WIDTH: f32 = 600.0;
    const HEADER: f32 = 36.0;

    /// The bar laid out as a header lays out its toolbar: a cached view filling the row.
    struct Header {
        bar: Entity<TimelineBar>,
    }

    impl Render for Header {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "header".into())
                .flex()
                .items_center()
                .w(px(WIDTH))
                .h(px(HEADER))
                .px(px(6.0))
                .child(
                    self.bar
                        .clone()
                        .cached(StyleRefinement::default().flex_1().min_w_0().h_full()),
                )
        }
    }

    #[gpui::test]
    fn timing_control_is_drawn_at_the_right_end(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let playback = cx.new(|_| Playback::new());
            let bar = cx.new(|cx| TimelineBar::new(playback, window, cx));
            Header { bar }
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        let header = cx.debug_bounds("header").expect("the header is drawn");
        let timing = cx.debug_bounds("timing-control").expect("the timing control is drawn");
        assert!(timing.size.width > px(0.0), "the timing control has no width: {timing:?}");
        assert!(timing.size.height > px(0.0), "the timing control has no height: {timing:?}");
        assert!(
            timing.origin.x >= header.origin.x && timing.right() <= header.right() + px(0.5),
            "the timing control spills out of the header: {timing:?} in {header:?}"
        );
        assert!(
            timing.right() > header.right() - px(16.0),
            "the timing control isn't at the right end: {timing:?} in {header:?}"
        );
    }
}
