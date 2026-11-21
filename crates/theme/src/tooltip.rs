use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::ActiveTheme as _;

/// A hover hint, standing in for the original's `title` attributes.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    /// A builder for `tooltip(...)` on a stateful element.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("Tooltip", || {
            let theme = cx.theme();
            let c = &theme.colors;
            // Offset from the cursor, which GPUI anchors tooltips at.
            div().pl(px(12.0)).pt(px(16.0)).child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius.md)
                    .bg(c.popover)
                    .border_1()
                    .border_color(c.foreground.opacity(0.1))
                    .shadow_md()
                    .font_family(theme.font.clone())
                    .text_size(theme.text.xs)
                    .text_color(c.popover_foreground)
                    .child(self.text.clone()),
            )
        })
    }
}
