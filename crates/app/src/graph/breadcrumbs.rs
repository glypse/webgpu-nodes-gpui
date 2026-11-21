//! The group path of a graph area, drawn in its header.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};
use guard::SafeListener as _;
use theme::{ActiveTheme as _, Icon, IconName};

use crate::editor::Editor;

pub struct Breadcrumbs {
    editor: Entity<Editor>,
    _observe: Subscription,
}

impl Breadcrumbs {
    pub fn new(editor: Entity<Editor>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&editor, |_, _, cx| cx.notify());
        Self { editor, _observe: observe }
    }
}

impl Render for Breadcrumbs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { guard::render("breadcrumbs", || {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let (crumbs, locked) = self.editor.read(cx).breadcrumbs();
        let last = crumbs.len() - 1;
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .size_full()
            .text_size(theme.text.sm)
            .children(crumbs.into_iter().enumerate().flat_map(|(i, (depth, label))| {
                let current = i == last;
                let crumb = div()
                    .id(("crumb", depth))
                    .px_2()
                    .rounded(theme.radius.sm)
                    .text_color(if current { c.foreground } else { c.muted_foreground })
                    .when(!current, |d| d.cursor_pointer().hover(|s| s.bg(c.muted)))
                    .on_click(cx.safe_listener(move |this, _: &ClickEvent, _, cx| {
                        this.editor.update(cx, |editor, cx| editor.jump_to(depth, cx));
                    }))
                    .child(label)
                    .into_any_element();
                let sep: Vec<AnyElement> = if i > 0 { vec![div().text_color(c.muted_foreground).child(Icon::new(IconName::CaretRight).size(px(12.0))).into_any_element()] } else { Vec::new() };
                sep.into_iter().chain([crumb])
            }))
            .when(locked, |d| d.child(div().ml_2().text_size(theme.text.xs).text_color(c.muted_foreground).child("locked")))
    })
    }
}
