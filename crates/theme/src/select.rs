use std::time::{Duration, Instant};

use gpui::{
    ClickEvent, Context, ElementId, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::popover::Popover;
use crate::{ActiveTheme as _, Icon, IconName};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectEvent {
    Change(usize),
}

/// A press on the trigger right after one that closed the list is the same
/// click: the list stays closed.
const SAME_CLICK: Duration = Duration::from_millis(250);

/// One option chosen from a list that opens under the trigger.
pub struct Select {
    id: SharedString,
    options: Vec<SharedString>,
    selected: Option<usize>,
    placeholder: SharedString,
    disabled: bool,
    menu_width: Pixels,
    open: bool,
    closed_at: Option<Instant>,
}

impl EventEmitter<SelectEvent> for Select {}

impl Select {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: None,
            placeholder: "Select".into(),
            disabled: false,
            menu_width: px(176.0),
            open: false,
            closed_at: None,
        }
    }

    pub fn options(mut self, options: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected = index;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The width of the open list.
    pub fn menu_width(mut self, width: Pixels) -> Self {
        self.menu_width = width;
        self
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows `index` without emitting a change.
    pub fn set_selected(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != index {
            self.selected = index;
            cx.notify();
        }
    }

    pub fn set_options(
        &mut self,
        options: impl IntoIterator<Item = impl Into<SharedString>>,
        cx: &mut Context<Self>,
    ) {
        self.options = options.into_iter().map(Into::into).collect();
        if self.selected.is_some_and(|i| i >= self.options.len()) {
            self.selected = None;
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else if !self.closed_at.is_some_and(|at| at.elapsed() < SAME_CLICK) {
            self.open = true;
            cx.notify();
        }
    }

    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close(cx);
        if self.selected != Some(index) {
            self.selected = Some(index);
            cx.emit(SelectEvent::Change(index));
        }
    }
}

impl Render for Select {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let disabled = self.disabled;
        let label = self.selected.and_then(|i| self.options.get(i)).cloned();
        let placeholder = label.is_none();
        let (hover, ring) = (
            if theme.is_dark() {
                c.input.opacity(0.5)
            } else {
                c.input
            },
            c.ring,
        );
        let trigger = div()
            .id(ElementId::from(self.id.clone()))
            .flex()
            .items_center()
            .justify_between()
            .gap_1p5()
            .h(px(32.0))
            .w_full()
            .pl(px(10.0))
            .pr_2()
            .rounded(theme.radius.lg)
            .border_1()
            .border_color(c.input)
            .when(theme.is_dark(), |d| d.bg(c.input.opacity(0.3)))
            .font_family(theme.font.clone())
            .text_size(theme.text.sm)
            .text_color(if placeholder {
                c.muted_foreground
            } else {
                c.foreground
            })
            .whitespace_nowrap()
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.border_color(ring).bg(hover))
            })
            .child(label.unwrap_or_else(|| self.placeholder.clone()))
            .child(
                Icon::new(IconName::CaretDown)
                    .size(px(16.0))
                    .color(c.muted_foreground),
            )
            .when(!disabled, |d| {
                d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle(cx)))
            });
        #[cfg(test)]
        let trigger = trigger.debug_selector(|| "select-trigger".into());

        let list = self.open.then(|| {
            let entity = cx.entity();
            let items = self.options.iter().enumerate().map(|(index, option)| {
                let selected = self.selected == Some(index);
                let choose = entity.clone();
                let item = div()
                    .id(ElementId::NamedInteger(self.id.clone(), index as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1p5()
                    .py_1()
                    .pl_1p5()
                    .pr_2()
                    .rounded(theme.radius.md)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.accent).text_color(c.accent_foreground))
                    .child(option.clone())
                    .child(div().size(px(14.0)).when(selected, |d| {
                        d.child(Icon::new(IconName::Check).size(px(14.0)))
                    }))
                    .on_click(move |_, _, cx| choose.update(cx, |this, cx| this.choose(index, cx)));
                #[cfg(test)]
                let item = item.debug_selector(|| format!("select-option-{index}"));
                item
            });
            let dismiss = entity.clone();
            div().absolute().top(px(36.0)).left_0().child(
                Popover::new()
                    .width(self.menu_width)
                    .on_dismiss(move |_, cx| dismiss.update(cx, |this, cx| this.close(cx)))
                    .children(items),
            )
        });

        div().relative().w_full().child(trigger).children(list)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

    use super::*;

    fn setup(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (
        Entity<Select>,
        Rc<RefCell<Vec<SelectEvent>>>,
        &mut VisualTestContext,
    ) {
        cx.update(crate::init);
        let events = Rc::new(RefCell::new(Vec::new()));
        let (select, cx) = cx.add_window_view(|_, _| {
            Select::new("select")
                .options(["One", "Two", "Three"])
                .disabled(disabled)
        });
        let log = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&select, move |_, event: &SelectEvent, _| {
                log.borrow_mut().push(*event)
            })
            .detach();
        });
        cx.run_until_parked();
        (select, events, cx)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("the element is drawn");
        cx.simulate_click(bounds.center(), Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn choosing_an_option_changes_the_selection_and_closes_the_list(cx: &mut TestAppContext) {
        let (select, events, cx) = setup(cx, false);
        click(cx, "select-trigger");
        assert!(cx.read(|cx| select.read(cx).is_open()));
        click(cx, "select-option-1");
        assert_eq!(*events.borrow(), [SelectEvent::Change(1)]);
        assert_eq!(cx.read(|cx| select.read(cx).selected_index()), Some(1));
        assert!(!cx.read(|cx| select.read(cx).is_open()));
    }

    #[gpui::test]
    fn choosing_the_selected_option_changes_nothing(cx: &mut TestAppContext) {
        let (select, events, cx) = setup(cx, false);
        cx.update(|_, cx| select.update(cx, |s, cx| s.set_selected(Some(2), cx)));
        click(cx, "select-trigger");
        click(cx, "select-option-2");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn a_press_outside_closes_the_list(cx: &mut TestAppContext) {
        let (select, events, cx) = setup(cx, false);
        click(cx, "select-trigger");
        cx.simulate_click(point(px(600.0), px(400.0)), Modifiers::default());
        cx.run_until_parked();
        assert!(!cx.read(|cx| select.read(cx).is_open()));
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn the_trigger_closes_an_open_list_rather_than_reopening_it(cx: &mut TestAppContext) {
        let (select, _, cx) = setup(cx, false);
        click(cx, "select-trigger");
        click(cx, "select-trigger");
        assert!(!cx.read(|cx| select.read(cx).is_open()));
    }

    #[gpui::test]
    fn a_disabled_select_stays_closed(cx: &mut TestAppContext) {
        let (select, _, cx) = setup(cx, true);
        click(cx, "select-trigger");
        assert!(!cx.read(|cx| select.read(cx).is_open()));
    }

    #[gpui::test]
    fn options_can_be_replaced_dropping_a_selection_out_of_range(cx: &mut TestAppContext) {
        let (select, _, cx) = setup(cx, false);
        cx.update(|_, cx| {
            select.update(cx, |s, cx| {
                s.set_selected(Some(2), cx);
                s.set_options(["Only"], cx);
            });
        });
        assert_eq!(cx.read(|cx| select.read(cx).selected_index()), None);
    }
}
