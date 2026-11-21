use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, px,
};

use crate::ActiveTheme as _;

/// A key or shortcut hint.
#[derive(IntoElement)]
pub struct Kbd {
    text: SharedString,
    quiet: bool,
}

impl Kbd {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            quiet: false,
        }
    }

    /// Without the chip, as menus show shortcuts.
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("Kbd", || {
            let theme = cx.theme();
            let base = div()
                .flex()
                .items_center()
                .justify_center()
                .text_size(theme.text.xs)
                .text_color(theme.colors.muted_foreground);
            if self.quiet {
                base.child(self.text)
            } else {
                base.h(px(20.0))
                    .min_w(px(20.0))
                    .px_1()
                    .rounded(theme.radius.sm)
                    .bg(theme.colors.muted)
                    .child(self.text)
            }
        })
    }
}
