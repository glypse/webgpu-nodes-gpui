use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{ActiveTheme as _, Icon, IconName};

type Change = Rc<dyn Fn(usize, &mut Window, &mut App)>;

struct Item {
    label: Option<SharedString>,
    icon: Option<IconName>,
}

/// A row of mutually exclusive options, one selected: tabs and segmented controls.
#[derive(IntoElement)]
pub struct ToggleGroup {
    id: SharedString,
    items: Vec<Item>,
    selected: usize,
    on_change: Option<Change>,
}

impl ToggleGroup {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(Item {
            label: Some(label.into()),
            icon: None,
        });
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.items.push(Item {
            label: None,
            icon: Some(icon),
        });
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ToggleGroup {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        guard::render("ToggleGroup", || {
            let theme = cx.theme();
            let c = &theme.colors;
            let last = self.items.len().saturating_sub(1);
            div()
                .flex()
                .items_center()
                .rounded(theme.radius.lg)
                .border_1()
                .border_color(c.border)
                .overflow_hidden()
                .children(self.items.into_iter().enumerate().map(|(index, item)| {
                    let selected = index == self.selected;
                    let on_change = self.on_change.clone();
                    div()
                        .id(ElementId::NamedInteger(self.id.clone(), index as u64))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .h(px(28.0))
                        .min_w(px(28.0))
                        .px_2()
                        .font_family(theme.font.clone())
                        .text_size(theme.text.sm)
                        .when(index != last, |d| d.border_r_1().border_color(c.border))
                        .when(selected, |d| d.bg(c.muted).text_color(c.foreground))
                        .when(!selected, |d| {
                            d.text_color(c.muted_foreground)
                                .cursor_pointer()
                                .hover(|s| s.bg(c.muted.opacity(0.5)))
                        })
                        .children(item.icon.map(|icon| Icon::new(icon).size(px(16.0))))
                        .children(item.label)
                        .when_some(on_change, |d, handler| {
                            d.on_click(move |_, window, cx| handler(index, window, cx))
                        })
                }))
        })
    }
}
