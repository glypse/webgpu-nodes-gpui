use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{ActiveTheme as _, Icon, IconName};

type Change = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A box that is checked or not (or, while `indeterminate`, neither).
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    indeterminate: bool,
    disabled: bool,
    on_change: Option<Change>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            indeterminate: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Shown as a dash, until clicked: that checks it.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the state a click asks for.
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let filled = self.checked || self.indeterminate;
        let (checked, disabled) = (self.checked, self.disabled);
        let next = !checked;
        let primary_fg = c.primary_foreground;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(if filled { c.primary } else { c.input })
            .when(filled, |d| d.bg(c.primary).text_color(c.primary_foreground))
            .when(!filled && theme.is_dark(), |d| d.bg(c.input.opacity(0.3)))
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| d.cursor_pointer())
            .when(filled, |d| {
                let icon = if checked {
                    IconName::Check
                } else {
                    IconName::Minus
                };
                d.child(Icon::new(icon).size(px(14.0)).color(primary_fg))
            })
            .when_some(self.on_change.filter(|_| !disabled), move |d, handler| {
                d.on_click(move |_, window, cx| handler(next, window, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Modifiers, Render, TestAppContext, point};

    use super::*;

    struct Host {
        checked: bool,
        indeterminate: bool,
        disabled: bool,
        changes: Rc<RefCell<Vec<bool>>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let host = cx.entity();
            let changes = self.changes.clone();
            div().child(
                Checkbox::new("box")
                    .checked(self.checked)
                    .indeterminate(self.indeterminate)
                    .disabled(self.disabled)
                    .on_change(move |value, _, cx| {
                        changes.borrow_mut().push(value);
                        host.update(cx, |host, cx| {
                            host.checked = value;
                            host.indeterminate = false;
                            cx.notify();
                        });
                    }),
            )
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        checked: bool,
        indeterminate: bool,
        disabled: bool,
    ) -> (Rc<RefCell<Vec<bool>>>, &mut gpui::VisualTestContext) {
        cx.update(crate::init);
        let changes = Rc::new(RefCell::new(Vec::new()));
        let shared = changes.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host {
            checked,
            indeterminate,
            disabled,
            changes: shared,
        });
        cx.run_until_parked();
        (changes, cx)
    }

    fn click(cx: &mut gpui::VisualTestContext) {
        cx.simulate_click(point(px(8.0), px(8.0)), Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_click_toggles_it(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx, false, false, false);
        click(cx);
        click(cx);
        assert_eq!(*changes.borrow(), [true, false]);
    }

    #[gpui::test]
    fn an_indeterminate_one_becomes_checked(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx, false, true, false);
        click(cx);
        assert_eq!(*changes.borrow(), [true]);
    }

    #[gpui::test]
    fn a_disabled_one_ignores_clicks(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx, false, false, true);
        click(cx);
        assert!(changes.borrow().is_empty());
    }
}
