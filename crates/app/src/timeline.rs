use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Point, Render, SharedString, Styled as _, Subscription, Window, div, point,
    prelude::FluentBuilder as _, px,
};
use runtime::Timing;
use theme::{
    ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName, Menu, MenuItem, NumberEvent,
    NumberField,
};

use crate::playback::Playback;

/// How far below the target rate counts as falling behind, past what timing jitter explains.
const SLOW_RATIO: f64 = 0.95;
const FRAME_RATES: [f64; 5] = [24.0, 30.0, 60.0, 120.0, 144.0];

/// The playback controls of a timeline area's header.
pub struct TimelineBar {
    playback: Entity<Playback>,
    frame: Entity<NumberField>,
    menu: Option<Point<Pixels>>,
    _subscriptions: [Subscription; 2],
}

impl TimelineBar {
    pub fn new(playback: Entity<Playback>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let frame = cx.new(|cx| NumberField::new(0.0, cx).label("Frame").integer().min(0.0));
        let subscriptions = [
            cx.observe(&playback, |_, _, cx| cx.notify()),
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
            menu: None,
            _subscriptions: subscriptions,
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let playback = self.playback.read(cx);
        let player = &playback.player;
        let timing = playback.timing();
        let playing = player.playing();
        let (frame, time) = (player.clock.frame, player.clock.time);
        let rate = player.meter.rate;
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

        self.frame
            .update(cx, |field, cx| field.set_value(frame as f64, cx));

        let playback = self.playback.clone();
        let act = move |f: fn(&mut Playback, &mut Window, &mut Context<Playback>)| {
            let playback = playback.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                playback.update(cx, |p, cx| f(p, window, cx));
            }
        };
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
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .child(
                Button::new("to-start")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::SkipBack)
                    .tooltip("Back to the start (Home)")
                    .on_click(act(|p, w, cx| p.seek(0.0, w, cx))),
            )
            .child(
                Button::new("previous")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::CaretLeft)
                    .tooltip("Previous frame (,)")
                    .on_click(act(|p, w, cx| p.step(-1, w, cx))),
            )
            .child(
                Button::new("play")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
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
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::CaretRight)
                    .tooltip("Next frame (.)")
                    .on_click(act(|p, w, cx| p.step(1, w, cx))),
            )
            .child(
                div()
                    .ml_2()
                    .w(px(144.0))
                    .flex_none()
                    .child(self.frame.clone()),
            )
            .child(
                div()
                    .px_2()
                    .text_size(theme.text.xs)
                    .text_color(c.muted_foreground)
                    .child(format_time(time)),
            )
            .child(div().flex_1())
            .child(
                Button::new("timing")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .tooltip(hint.join("\n"))
                    .selected(self.menu.is_some())
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
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
    }
}
