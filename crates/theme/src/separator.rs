use gpui::{App, IntoElement, RenderOnce, Styled as _, Window, div, px};

use crate::ActiveTheme as _;

#[derive(IntoElement)]
pub struct Separator {
    vertical: bool,
}

impl Separator {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }

    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl RenderOnce for Separator {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("Separator", || {
            let line = div().flex_none().bg(cx.theme().colors.border);
            if self.vertical {
                line.w(px(1.0)).h_full()
            } else {
                line.h(px(1.0)).w_full()
            }
        })
    }
}
