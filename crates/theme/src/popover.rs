use gpui::{
    AnyElement, App, Corner, Div, InteractiveElement as _, IntoElement, ParentElement, Pixels,
    Point, RenderOnce, Styled as _, Window, anchored, deferred, div, px,
};

use crate::ActiveTheme as _;

type Dismiss = Box<dyn Fn(&mut Window, &mut App)>;

/// A floating panel drawn above the rest of the window.
///
/// Placed with `at` it opens at a window position, as a context menu does.
/// Otherwise it opens at its own place in the layout, so put it in an
/// absolutely positioned wrapper next to the element it belongs to.
#[derive(IntoElement)]
pub struct Popover {
    at: Option<Point<Pixels>>,
    anchor: Corner,
    width: Option<Pixels>,
    on_dismiss: Option<Dismiss>,
    children: Vec<AnyElement>,
}

impl Popover {
    pub fn new() -> Self {
        Self {
            at: None,
            anchor: Corner::TopLeft,
            width: None,
            on_dismiss: None,
            children: Vec::new(),
        }
    }

    pub fn at(mut self, position: Point<Pixels>) -> Self {
        self.at = Some(position);
        self
    }

    /// Which corner of the panel sits at the opening point.
    pub fn anchor(mut self, anchor: Corner) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Called on a press outside the panel.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Box::new(handler));
        self
    }
}

impl Default for Popover {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Popover {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

pub(crate) fn panel(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .p_1()
        .rounded(theme.radius.lg)
        .bg(theme.colors.popover)
        .text_color(theme.colors.popover_foreground)
        .border_1()
        .border_color(theme.colors.foreground.opacity(0.1))
        .shadow_md()
        .font_family(theme.font.clone())
        .text_size(theme.text.sm)
}

impl RenderOnce for Popover {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("Popover", || {
            let mut panel = panel(cx)
                .occlude()
                .when_width(self.width)
                .children(self.children);
            if let Some(dismiss) = self.on_dismiss {
                panel = panel.on_mouse_down_out(move |_, window, cx| dismiss(window, cx));
            }
            let mut anchored = anchored()
                .anchor(self.anchor)
                .snap_to_window_with_margin(px(8.0));
            if let Some(position) = self.at {
                anchored = anchored.position(position);
            }
            deferred(anchored.child(panel)).with_priority(1)
        })
    }
}

trait WhenWidth {
    fn when_width(self, width: Option<Pixels>) -> Self;
}

impl WhenWidth for Div {
    fn when_width(self, width: Option<Pixels>) -> Self {
        match width {
            Some(width) => self.w(width),
            None => self,
        }
    }
}
