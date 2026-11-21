use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Pixels, Point, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, actions, div,
    prelude::FluentBuilder as _, px,
};

use crate::popover::Popover;
use crate::{ActiveTheme as _, Icon, IconName, Kbd, Separator};

type Select = Rc<dyn Fn(&mut Window, &mut App)>;

actions!(theme_menu, [Next, Previous, First, Last, Confirm, Close]);

const CONTEXT: &str = "ThemeMenu";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Previous, Some(CONTEXT)),
        KeyBinding::new("home", First, Some(CONTEXT)),
        KeyBinding::new("end", Last, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("space", Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Close, Some(CONTEXT)),
    ]);
}

/// What a menu keeps between frames.
struct Navigation {
    /// The entry the keys act on.
    highlighted: Option<usize>,
    focus: FocusHandle,
    focused: bool,
}

pub struct MenuItem {
    label: SharedString,
    shortcut: Option<SharedString>,
    icon: Option<IconName>,
    checked: Option<bool>,
    disabled: bool,
    destructive: bool,
    on_select: Option<Select>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            icon: None,
            checked: None,
            disabled: false,
            destructive: false,
            on_select: None,
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Makes it a checkbox item, showing a check while `checked`.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

enum Entry {
    Item(MenuItem),
    Label(SharedString),
    Separator,
}

/// A list of actions in a `Popover`. Selecting an item runs its handler, then
/// dismisses the menu.
#[derive(IntoElement)]
pub struct Menu {
    id: SharedString,
    popover: Popover,
    entries: Vec<Entry>,
    on_dismiss: Option<Select>,
}

impl Menu {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            popover: Popover::new().width(px(176.0)),
            entries: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn at(mut self, position: Point<Pixels>) -> Self {
        self.popover = self.popover.at(position);
        self
    }

    pub fn anchor(mut self, anchor: gpui::Corner) -> Self {
        self.popover = self.popover.anchor(anchor);
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.popover = self.popover.width(width);
        self
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Label(label.into()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    /// Called after a selection and on a press outside the menu.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

/// An action moving the highlight to the enabled entry `pick` chooses, given the current one's
/// position among them and how many there are.
fn mover<A: 'static>(
    navigation: &Entity<Navigation>,
    enabled: &Rc<Vec<(usize, Option<Select>)>>,
    pick: fn(Option<usize>, usize) -> usize,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let (navigation, enabled) = (navigation.clone(), enabled.clone());
    move |_, _, cx| {
        if enabled.is_empty() {
            return;
        }
        navigation.update(cx, |nav, cx| {
            let at = nav
                .highlighted
                .and_then(|h| enabled.iter().position(|(i, _)| *i == h));
            nav.highlighted = enabled.get(pick(at, enabled.len())).map(|(i, _)| *i);
            cx.notify();
        });
    }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("Menu", || {
            let theme = cx.theme().clone();
            let c = &theme.colors;
            let (accent, accent_fg, destructive) = (c.accent, c.accent_foreground, c.destructive);
            let on_dismiss = self.on_dismiss;
            let id = self.id;
            let navigation = window.use_keyed_state(
                ElementId::NamedInteger(id.clone(), u64::MAX),
                cx,
                |_, cx| Navigation {
                    highlighted: None,
                    focus: cx.focus_handle(),
                    focused: false,
                },
            );
            let focus = navigation.read(cx).focus.clone();
            if !navigation.read(cx).focused {
                navigation.update(cx, |nav, _| nav.focused = true);
                let focus = focus.clone();
                window.defer(cx, move |window, _| window.focus(&focus));
            }
            let highlighted = navigation.read(cx).highlighted;
            let enabled: Rc<Vec<(usize, Option<Select>)>> = Rc::new(
                self.entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| match entry {
                        Entry::Item(item) if !item.disabled => {
                            Some((index, item.on_select.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
            );
            let entries = self
                .entries
                .into_iter()
                .enumerate()
                .map(|(index, entry)| match entry {
                    Entry::Separator => div()
                        .my_1()
                        .mx(px(-4.0))
                        .child(Separator::horizontal())
                        .into_any_element(),
                    Entry::Label(label) => div()
                        .px_1p5()
                        .py_1()
                        .text_size(theme.text.xs)
                        .text_color(c.muted_foreground)
                        .child(label)
                        .into_any_element(),
                    Entry::Item(item) => {
                        let disabled = item.disabled;
                        let select = item.on_select;
                        let dismiss = on_dismiss.clone();
                        let color = if item.destructive {
                            destructive
                        } else {
                            c.popover_foreground
                        };
                        let hover = navigation.clone();
                        div()
                            .id(ElementId::NamedInteger(id.clone(), index as u64))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_1p5()
                            .py_1()
                            .rounded(theme.radius.md)
                            .text_color(color)
                            .when(disabled, |d| d.opacity(0.5))
                            .when(!disabled, |d| {
                                let destructive_item = item.destructive;
                                let highlight = move |s: gpui::StyleRefinement| {
                                    if destructive_item {
                                        s.bg(destructive.opacity(0.1))
                                    } else {
                                        s.bg(accent).text_color(accent_fg)
                                    }
                                };
                                d.cursor_pointer()
                                    .hover(highlight)
                                    .when(highlighted == Some(index), |d| {
                                        if destructive_item {
                                            d.bg(destructive.opacity(0.1))
                                        } else {
                                            d.bg(accent).text_color(accent_fg)
                                        }
                                    })
                                    .on_mouse_move(move |_, _, cx| {
                                        hover.update(cx, |nav, cx| {
                                            if nav.highlighted != Some(index) {
                                                nav.highlighted = Some(index);
                                                cx.notify();
                                            }
                                        });
                                    })
                            })
                            .children(item.icon.map(|icon| Icon::new(icon).size(px(16.0))))
                            .children(item.checked.map(|checked| {
                                div().size(px(16.0)).flex_none().when(checked, |d| {
                                    d.child(Icon::new(IconName::Check).size(px(16.0)))
                                })
                            }))
                            .child(div().flex_1().child(item.label))
                            .children(item.shortcut.map(|s| Kbd::new(s).quiet()))
                            .when(!disabled, |d| {
                                d.on_click(move |_: &ClickEvent, window, cx| {
                                    if let Some(select) = &select {
                                        select(window, cx);
                                    }
                                    if let Some(dismiss) = &dismiss {
                                        dismiss(window, cx);
                                    }
                                })
                            })
                            .into_any_element()
                    }
                });
            let confirm = {
                let (navigation, enabled, dismiss) =
                    (navigation.clone(), enabled.clone(), on_dismiss.clone());
                move |_: &Confirm, window: &mut Window, cx: &mut App| {
                    let Some(highlighted) = navigation.read(cx).highlighted else {
                        return;
                    };
                    if let Some((_, Some(select))) = enabled.iter().find(|(i, _)| *i == highlighted)
                    {
                        select(window, cx);
                    }
                    if let Some(dismiss) = &dismiss {
                        dismiss(window, cx);
                    }
                }
            };
            let close = {
                let dismiss = on_dismiss.clone();
                move |_: &Close, window: &mut Window, cx: &mut App| {
                    if let Some(dismiss) = &dismiss {
                        dismiss(window, cx);
                    }
                }
            };
            let keyed = div()
                .id(ElementId::NamedInteger(id.clone(), u64::MAX - 1))
                .flex()
                .flex_col()
                .key_context(CONTEXT)
                .track_focus(&focus)
                .on_action(mover::<Next>(&navigation, &enabled, |at, len| {
                    at.map_or(0, |at| (at + 1).min(len - 1))
                }))
                .on_action(mover::<Previous>(&navigation, &enabled, |at, len| {
                    at.map_or(len - 1, |at| at.saturating_sub(1))
                }))
                .on_action(mover::<First>(&navigation, &enabled, |_, _| 0))
                .on_action(mover::<Last>(&navigation, &enabled, |_, len| len - 1))
                .on_action(confirm)
                .on_action(close)
                .children(entries);
            let popover = self.popover.child(keyed);
            match on_dismiss {
                Some(dismiss) => popover.on_dismiss(move |window, cx| dismiss(window, cx)),
                None => popover,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Render, TestAppContext, point};

    use super::*;

    struct Host {
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            guard::render("Host", || {
                let (first, second, dismissed) =
                    (self.log.clone(), self.log.clone(), self.log.clone());
                div().size_full().child(
                    Menu::new("menu")
                        .at(point(px(10.0), px(10.0)))
                        .item(
                            MenuItem::new("One")
                                .on_select(move |_, _| first.borrow_mut().push("one")),
                        )
                        .item(MenuItem::new("Off").disabled(true))
                        .item(
                            MenuItem::new("Two")
                                .on_select(move |_, _| second.borrow_mut().push("two")),
                        )
                        .on_dismiss(move |_, _| dismissed.borrow_mut().push("dismissed")),
                )
            })
        }
    }

    fn setup(
        cx: &mut TestAppContext,
    ) -> (Rc<RefCell<Vec<&'static str>>>, &mut gpui::VisualTestContext) {
        cx.update(crate::init);
        let log = Rc::new(RefCell::new(Vec::new()));
        let shared = log.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host { log: shared });
        cx.run_until_parked();
        cx.run_until_parked();
        (log, cx)
    }

    #[gpui::test]
    fn the_arrows_skip_disabled_entries_and_enter_selects(cx: &mut TestAppContext) {
        let (log, cx) = setup(cx);
        cx.simulate_keystrokes("down down enter");
        assert_eq!(*log.borrow(), ["two", "dismissed"]);
    }

    #[gpui::test]
    fn escape_dismisses_without_selecting(cx: &mut TestAppContext) {
        let (log, cx) = setup(cx);
        cx.simulate_keystrokes("down escape");
        assert_eq!(*log.borrow(), ["dismissed"]);
    }
}
