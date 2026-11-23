use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px,
};
use theme::ActiveTheme as _;

use crate::playback::{Playback, PlaybackEvent};

/// The WGSL of the program being drawn.
pub struct CodeView {
    code: SharedString,
    _subscription: Subscription,
}

impl CodeView {
    pub fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let code = playback.read(cx).code.clone();
        // Playback emits on every drawn frame, so the code is compared here
        // rather than read in render: the view then re-renders only when the
        // program's text changes.
        let subscription = cx.subscribe(&playback, |this, playback, _: &PlaybackEvent, cx| {
            guard::guard("code view update", || {
                let code = playback.read(cx).code.clone();
                if code != this.code {
                    this.code = code;
                    cx.notify();
                }
            });
        });
        Self {
            code,
            _subscription: subscription,
        }
    }
}

impl Render for CodeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("code view", || {
            let theme = cx.theme();
            let c = &theme.colors;
            div()
                .id("code")
                .size_full()
                .overflow_scroll()
                .p_3()
                .font_family(theme.mono_font.clone())
                .text_size(theme.text.xs)
                .text_color(c.foreground)
                .children(self.code.lines().enumerate().map(|(i, line)| {
                    div()
                        .flex()
                        .gap_3()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .w(px(28.0))
                                .flex_none()
                                .text_right()
                                .text_color(c.muted_foreground)
                                .child((i + 1).to_string()),
                        )
                        .child(line.to_string())
                }))
        })
    }
}

pub fn new(playback: &Entity<Playback>, cx: &mut gpui::App) -> Entity<CodeView> {
    cx.new(|cx| CodeView::new(playback.clone(), cx))
}
