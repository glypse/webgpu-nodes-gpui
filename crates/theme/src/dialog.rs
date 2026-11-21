use std::rc::Rc;

use gpui::{
    AnyElement, App, Corner, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, ParentElement, Pixels, RenderOnce, SharedString, Styled as _, Window, actions,
    anchored, deferred, div, point, prelude::FluentBuilder as _, px,
};

use crate::{ActiveTheme as _, Button, ButtonSize, ButtonVariant, IconName};

actions!(theme_dialog, [Dismiss]);

const CONTEXT: &str = "ThemeDialog";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A modal panel centred over the window, dimming what is behind it.
///
/// Draw it only while open. It is dismissed by a press outside it, by its
/// close button and, when the focus is inside it (give it a `focus_handle`
/// and focus that when opening), by Escape.
#[derive(IntoElement)]
pub struct Dialog {
    title: Option<SharedString>,
    description: Option<SharedString>,
    width: Pixels,
    close_button: bool,
    focus_handle: Option<FocusHandle>,
    on_dismiss: Option<Handler>,
    children: Vec<AnyElement>,
    footer: Vec<AnyElement>,
}

impl Dialog {
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            width: px(384.0),
            close_button: true,
            focus_handle: None,
            on_dismiss: None,
            children: Vec::new(),
            footer: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn close_button(mut self, shown: bool) -> Self {
        self.close_button = shown;
        self
    }

    /// What Escape is listened for through: focus it to have Escape dismiss.
    pub fn focus_handle(mut self, handle: &FocusHandle) -> Self {
        self.focus_handle = Some(handle.clone());
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// An element of the bar along the bottom, whose buttons are right-aligned.
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer.push(element.into_any_element());
        self
    }
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let viewport = window.viewport_size();
        let dismiss = self.on_dismiss;
        let header = (self.title.is_some() || self.description.is_some()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(self.title.map(|t| {
                    div()
                        .text_size(theme.text.base)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(t)
                }))
                .children(
                    self.description
                        .map(|d| div().text_color(c.muted_foreground).child(d)),
                )
        });
        let close = self.close_button.then(|| {
            let dismiss = dismiss.clone();
            div().absolute().top_2().right_2().child(
                Button::new("dialog-close")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::X)
                    .on_click(move |_, window, cx| {
                        if let Some(dismiss) = &dismiss {
                            dismiss(window, cx);
                        }
                    }),
            )
        });
        let footer = (!self.footer.is_empty()).then(|| {
            div()
                .flex()
                .justify_end()
                .gap_2()
                .mx(px(-16.0))
                .mb(px(-16.0))
                .p_4()
                .rounded_b(theme.radius.xl)
                .border_t_1()
                .border_color(c.border)
                .bg(c.muted.opacity(0.5))
                .children(self.footer)
        });
        let on_escape = dismiss.clone();
        let on_backdrop = dismiss;
        let panel = div()
            .relative()
            .flex()
            .flex_col()
            .gap_4()
            .w(self.width)
            .max_w(viewport.width - px(32.0))
            .p_4()
            .rounded(theme.radius.xl)
            .bg(c.popover)
            .text_color(c.popover_foreground)
            .border_1()
            .border_color(c.foreground.opacity(0.1))
            .shadow_lg()
            .font_family(theme.font.clone())
            .text_size(theme.text.sm)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .key_context(CONTEXT)
            .when_some(self.focus_handle, |d, handle| d.track_focus(&handle))
            .on_action(move |_: &Dismiss, window, cx| {
                if let Some(dismiss) = &on_escape {
                    dismiss(window, cx);
                }
            })
            .children(header)
            .children(self.children)
            .children(footer)
            .children(close);
        #[cfg(test)]
        let panel = panel.debug_selector(|| "dialog-panel".into());

        let backdrop = div()
            .flex()
            .items_center()
            .justify_center()
            .w(viewport.width)
            .h(viewport.height)
            .bg(gpui::black().opacity(if theme.is_dark() { 0.5 } else { 0.1 }))
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(dismiss) = &on_backdrop {
                    dismiss(window, cx);
                }
            })
            .child(panel);
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .anchor(Corner::TopLeft)
                .child(backdrop),
        )
        .with_priority(3)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext};

    use super::*;

    struct Host {
        open: bool,
        focus: FocusHandle,
        dismissals: Rc<Cell<usize>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let host = cx.entity();
            let dismissals = self.dismissals.clone();
            div().size_full().when(self.open, |d| {
                d.child(
                    Dialog::new()
                        .title("Delete")
                        .description("This can't be undone.")
                        .focus_handle(&self.focus)
                        .on_dismiss(move |_, cx| {
                            dismissals.set(dismissals.get() + 1);
                            host.update(cx, |host, cx| {
                                host.open = false;
                                cx.notify();
                            });
                        })
                        .child(div().child("Body"))
                        .footer(div().child("Footer")),
                )
            })
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Rc<Cell<usize>>, &mut VisualTestContext) {
        cx.update(crate::init);
        let dismissals = Rc::new(Cell::new(0));
        let shared = dismissals.clone();
        let (host, cx) = cx.add_window_view(move |_, cx| Host {
            open: true,
            focus: cx.focus_handle(),
            dismissals: shared,
        });
        cx.update(|window, cx| {
            let focus = host.read(cx).focus.clone();
            window.focus(&focus);
        });
        cx.run_until_parked();
        (dismissals, cx)
    }

    #[gpui::test]
    fn a_press_outside_the_panel_dismisses_it(cx: &mut TestAppContext) {
        let (dismissals, cx) = setup(cx);
        cx.simulate_click(gpui::point(px(5.0), px(5.0)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(dismissals.get(), 1);
    }

    #[gpui::test]
    fn a_press_inside_the_panel_does_not(cx: &mut TestAppContext) {
        let (dismissals, cx) = setup(cx);
        let panel = cx.debug_bounds("dialog-panel").expect("the panel is drawn");
        cx.simulate_click(panel.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(dismissals.get(), 0);
    }

    #[gpui::test]
    fn escape_dismisses_it_while_focused(cx: &mut TestAppContext) {
        let (dismissals, cx) = setup(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(dismissals.get(), 1);
    }

    #[gpui::test]
    fn the_close_button_dismisses_it(cx: &mut TestAppContext) {
        let (dismissals, cx) = setup(cx);
        let panel = cx.debug_bounds("dialog-panel").expect("the panel is drawn");
        let close = gpui::point(panel.right() - px(18.0), panel.top() + px(18.0));
        cx.simulate_click(close, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(dismissals.get(), 1);
    }
}
