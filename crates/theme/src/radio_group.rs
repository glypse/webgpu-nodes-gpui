use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::ActiveTheme as _;

type Change = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A column of options, one selected at most.
#[derive(IntoElement)]
pub struct RadioGroup {
    id: SharedString,
    items: Vec<SharedString>,
    selected: Option<usize>,
    disabled: bool,
    on_change: Option<Change>,
}

impl RadioGroup {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: None,
            disabled: false,
            on_change: None,
        }
    }

    pub fn item(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(label.into());
        self
    }

    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected = index;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for RadioGroup {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let dark = theme.is_dark();
        let disabled = self.disabled;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .font_family(theme.font.clone())
            .text_size(theme.text.sm)
            .text_color(c.foreground)
            .children(self.items.into_iter().enumerate().map(|(index, label)| {
                let checked = self.selected == Some(index);
                let on_change = self.on_change.clone().filter(|_| !disabled);
                let row = div()
                    .id(ElementId::NamedInteger(self.id.clone(), index as u64))
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(disabled, |d| d.opacity(0.5))
                    .when(!disabled, |d| d.cursor_pointer())
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(16.0))
                            .rounded_full()
                            .border_1()
                            .border_color(if checked { c.primary } else { c.input })
                            .when(checked, |d| d.bg(c.primary))
                            .when(!checked && dark, |d| d.bg(c.input.opacity(0.3)))
                            .when(checked, |d| {
                                d.child(div().size(px(8.0)).rounded_full().bg(c.primary_foreground))
                            }),
                    )
                    .child(label)
                    .when_some(on_change, |d, handler| {
                        d.on_click(move |_, window, cx| handler(index, window, cx))
                    });
                #[cfg(test)]
                let row = row.debug_selector(|| format!("radio-{index}"));
                row
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Modifiers, Render, TestAppContext};

    use super::*;

    struct Host {
        selected: Option<usize>,
        disabled: bool,
        changes: Rc<RefCell<Vec<usize>>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let host = cx.entity();
            let changes = self.changes.clone();
            div().w(px(200.0)).child(
                RadioGroup::new("radio")
                    .item("One")
                    .item("Two")
                    .item("Three")
                    .selected(self.selected)
                    .disabled(self.disabled)
                    .on_change(move |index, _, cx| {
                        changes.borrow_mut().push(index);
                        host.update(cx, |host, cx| {
                            host.selected = Some(index);
                            cx.notify();
                        });
                    }),
            )
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (Rc<RefCell<Vec<usize>>>, &mut gpui::VisualTestContext) {
        cx.update(crate::init);
        let changes = Rc::new(RefCell::new(Vec::new()));
        let shared = changes.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host {
            selected: Some(0),
            disabled,
            changes: shared,
        });
        cx.run_until_parked();
        (changes, cx)
    }

    fn click_row(cx: &mut gpui::VisualTestContext, row: usize) {
        let selector: &'static str = ["radio-0", "radio-1", "radio-2"][row];
        let bounds = cx.debug_bounds(selector).expect("the row is drawn");
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn clicking_an_option_selects_it(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx, false);
        click_row(cx, 2);
        click_row(cx, 1);
        assert_eq!(*changes.borrow(), [2, 1]);
    }

    #[gpui::test]
    fn a_disabled_group_ignores_clicks(cx: &mut TestAppContext) {
        let (changes, cx) = setup(cx, true);
        click_row(cx, 1);
        assert!(changes.borrow().is_empty());
    }
}
