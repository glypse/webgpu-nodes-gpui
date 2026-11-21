use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, Context, ElementId, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use guard::SafeListener as _;

use crate::popover::Popover;
use crate::{ActiveTheme as _, Icon, IconName};

actions!(theme_select, [Next, Previous, First, Last, Confirm, Cancel]);

const CONTEXT: &str = "ThemeSelect";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Previous, Some(CONTEXT)),
        KeyBinding::new("home", First, Some(CONTEXT)),
        KeyBinding::new("end", Last, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("space", Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

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
    /// The option the keys act on while the list is open.
    highlighted: Option<usize>,
    closed_at: Option<Instant>,
    focus: FocusHandle,
}

impl EventEmitter<SelectEvent> for Select {}

impl Select {
    pub fn new(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: None,
            placeholder: "Select".into(),
            disabled: false,
            menu_width: px(176.0),
            open: false,
            highlighted: None,
            closed_at: None,
            focus: cx.focus_handle(),
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

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        if !self.open && !self.disabled && !self.options.is_empty() {
            self.open = true;
            self.highlighted = self.selected.or(Some(0));
            cx.notify();
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.highlighted = None;
            self.closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else if !self.closed_at.is_some_and(|at| at.elapsed() < SAME_CLICK) {
            self.open(cx);
        }
    }

    fn highlight(&mut self, index: usize, cx: &mut Context<Self>) {
        let last = self.options.len().saturating_sub(1);
        self.highlighted = Some(index.min(last));
        cx.notify();
    }

    fn next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlight(self.highlighted.map_or(0, |i| i + 1), cx);
        } else if self.focus_blocked() {
            cx.propagate();
        } else {
            self.open(cx);
        }
    }

    fn previous(&mut self, _: &Previous, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlight(self.highlighted.map_or(0, |i| i.saturating_sub(1)), cx);
        } else if self.focus_blocked() {
            cx.propagate();
        } else {
            self.open(cx);
        }
    }

    fn first(&mut self, _: &First, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlight(0, cx);
        } else {
            cx.propagate();
        }
    }

    fn last(&mut self, _: &Last, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlight(self.options.len().saturating_sub(1), cx);
        } else {
            cx.propagate();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.open(cx);
        } else if let Some(index) = self.highlighted {
            self.choose(index, cx);
        } else {
            self.close(cx);
        }
    }

    /// Escape closes the list; with it closed the key belongs to whatever contains the select.
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            cx.propagate();
        }
    }

    fn focus_blocked(&self) -> bool {
        self.disabled || self.options.is_empty()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("Select", || {
            let focused = self.focus.is_focused(window);
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
                .border_color(if focused && !disabled { ring } else { c.input })
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
                        .track_focus(&self.focus)
                        .key_context(CONTEXT)
                        .on_action(cx.safe_listener(Self::next))
                        .on_action(cx.safe_listener(Self::previous))
                        .on_action(cx.safe_listener(Self::first))
                        .on_action(cx.safe_listener(Self::last))
                        .on_action(cx.safe_listener(Self::confirm))
                        .on_action(cx.safe_listener(Self::cancel))
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
                    let highlighted = self.highlighted == Some(index);
                    let choose = entity.clone();
                    let hovered = entity.clone();
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
                        .when(highlighted, |d| {
                            d.bg(c.accent).text_color(c.accent_foreground)
                        })
                        .on_mouse_move(move |_, _, cx| {
                            hovered.update(cx, |this, cx| {
                                if this.highlighted != Some(index) {
                                    this.highlight(index, cx);
                                }
                            });
                        })
                        .child(option.clone())
                        .child(div().size(px(14.0)).when(selected, |d| {
                            d.child(Icon::new(IconName::Check).size(px(14.0)))
                        }))
                        .on_click(move |_, _, cx| {
                            choose.update(cx, |this, cx| this.choose(index, cx))
                        });
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
        })
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
        let (select, cx) = cx.add_window_view(|_, cx| {
            Select::new("select", cx)
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

    fn press(cx: &mut VisualTestContext, keys: &str) {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_keys_open_the_list_move_through_it_and_choose(cx: &mut TestAppContext) {
        let (select, events, cx) = setup(cx, false);
        cx.update(|window, cx| select.update(cx, |s, _| s.focus(window)));
        press(cx, "down");
        assert!(cx.read(|cx| select.read(cx).is_open()));
        press(cx, "down down enter");
        assert_eq!(*events.borrow(), [SelectEvent::Change(2)]);
        assert!(!cx.read(|cx| select.read(cx).is_open()));
        press(cx, "space up enter");
        assert_eq!(
            *events.borrow(),
            [SelectEvent::Change(2), SelectEvent::Change(1)]
        );
    }

    #[gpui::test]
    fn escape_closes_the_list_without_choosing(cx: &mut TestAppContext) {
        let (select, events, cx) = setup(cx, false);
        cx.update(|window, cx| select.update(cx, |s, _| s.focus(window)));
        press(cx, "enter down escape");
        assert!(!cx.read(|cx| select.read(cx).is_open()));
        assert!(events.borrow().is_empty());
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
