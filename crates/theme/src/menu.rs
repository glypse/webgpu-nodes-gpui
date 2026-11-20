use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Point, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::popover::Popover;
use crate::{ActiveTheme as _, Icon, IconName, Kbd, Separator};

type Select = Rc<dyn Fn(&mut Window, &mut App)>;

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

impl RenderOnce for Menu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let c = &theme.colors;
        let (accent, accent_fg, destructive) = (c.accent, c.accent_foreground, c.destructive);
        let on_dismiss = self.on_dismiss;
        let id = self.id;
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
                            d.cursor_pointer().hover(move |s| {
                                if item.destructive {
                                    s.bg(destructive.opacity(0.1))
                                } else {
                                    s.bg(accent).text_color(accent_fg)
                                }
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
        let popover = self.popover.children(entries);
        match on_dismiss {
            Some(dismiss) => popover.on_dismiss(move |window, cx| dismiss(window, cx)),
            None => popover,
        }
    }
}
