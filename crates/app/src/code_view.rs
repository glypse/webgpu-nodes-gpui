use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use theme::ActiveTheme as _;

use crate::playback::Playback;

/// The WGSL of the program being drawn.
pub struct CodeView {
    playback: Entity<Playback>,
    _observe: Subscription,
}

impl CodeView {
    pub fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&playback, |_, _, cx| cx.notify());
        Self {
            playback,
            _observe: observe,
        }
    }
}

impl Render for CodeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("code view", || {
        let theme = cx.theme();
        let c = &theme.colors;
        let code = self.playback.read(cx).code.clone();
        div()
            .id("code")
            .size_full()
            .overflow_scroll()
            .p_3()
            .font_family(theme.mono_font.clone())
            .text_size(theme.text.xs)
            .text_color(c.foreground)
            .children(code.lines().enumerate().map(|(i, line)| {
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
