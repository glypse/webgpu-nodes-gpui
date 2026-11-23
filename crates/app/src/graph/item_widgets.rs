use std::rc::Rc;

use gpui::{
    App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, SharedString,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::Input;
use shader_graph::passes::TextureFormat;
use theme::{ActiveTheme as _, Icon, IconName};

use super::fields::picker;
use super::widgets::{ItemNameField, ItemRow};
use crate::editor::{Editor, ListItem};

fn icon_button(
    id: SharedString,
    label: impl IntoElement,
    enabled: bool,
    z: f32,
    cx: &App,
    on_click: impl Fn(gpui::Point<gpui::Pixels>, &mut App) + 'static,
) -> impl IntoElement {
    let muted = cx.theme().colors.muted;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.0 * z))
        .rounded(px(4.0 * z))
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(muted)))
        .on_mouse_down(MouseButton::Left, move |e, _, cx| {
            cx.stop_propagation();
            if enabled {
                on_click(e.position, cx);
            }
        })
        .child(label)
}

#[allow(clippy::too_many_arguments)]
pub fn row(
    row: &ItemRow,
    node: &str,
    can_remove: bool,
    locked: bool,
    z: f32,
    ed: &WeakEntity<Editor>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let name = window.use_keyed_state(
        SharedString::from(format!("item-name-{node}-{}", row.id)),
        cx,
        |window, cx| ItemNameField::new(node, &row.id, &row.label, ed.clone(), window, cx),
    );
    name.update(cx, |name, cx| name.sync(&row.label, window, cx));
    let input = name.read(cx).input.clone();
    let colors = &cx.theme().colors;
    let items: Vec<ListItem> = TextureFormat::ALL
        .into_iter()
        .map(|format| {
            let (ed, node, item) = (ed.clone(), node.to_owned(), row.id.clone());
            ListItem::new(
                format.id(),
                Some(format.id() == row.format),
                Rc::new(move |_, cx| {
                    super::update_editor(&ed, cx, |e, cx| e.set_render_item_format(&node, &item, format, cx));
                }),
            )
        })
        .collect();
    let open = ed.clone();
    let (remove, node_id, item_id) = (ed.clone(), node.to_owned(), row.id.clone());
    div()
        .flex()
        .items_center()
        .gap(px(4.0 * z))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(px(22.0 * z))
                .rounded(px(4.0 * z))
                .bg(colors.foreground.opacity(0.05))
                .when(locked, |d| d.opacity(0.5).child(div().absolute().size_full()))
                .child(Input::new(&input).h(px(22.0 * z)).text_size(px(12.0 * z))),
        )
        .child(div().w(px(84.0 * z)).flex().child(picker(
            SharedString::from(format!("format-{node}-{}", row.id)),
            row.format.clone(),
            z,
            locked,
            cx,
            move |at, _, cx| {
                let items = items.clone();
                super::update_editor(&open, cx, |e, cx| e.open_list(at, items, cx));
            },
        )))
        .child(icon_button(
            SharedString::from(format!("remove-{node}-{}", row.id)),
            Icon::new(IconName::Trash).size(px(12.0 * z)),
            can_remove,
            z,
            cx,
            move |_, cx| {
                super::update_editor(&remove, cx, |e, cx| e.remove_render_item(&node_id, &item_id, cx));
            },
        ))
}

pub fn add(node: &str, locked: bool, z: f32, ed: &WeakEntity<Editor>, cx: &App) -> impl IntoElement {
    let (ed, node_id) = (ed.clone(), node.to_owned());
    div().flex().child(picker(
        SharedString::from(format!("add-render-item-{node}")),
        "+ Add item",
        z,
        locked,
        cx,
        move |_, _, cx| {
            super::update_editor(&ed, cx, |e, cx| e.add_render_item(&node_id, cx));
        },
    ))
}

pub fn sockets_button(node: &str, locked: bool, z: f32, ed: &WeakEntity<Editor>, cx: &App) -> impl IntoElement {
    let (ed, node_id) = (ed.clone(), node.to_owned());
    icon_button(SharedString::from(format!("sockets-{node}")), Icon::new(IconName::Plugs).size(px(14.0 * z)), !locked, z, cx, move |at, cx| {
        super::update_editor(&ed, cx, |e, cx| e.open_sockets(&node_id, at, cx));
    })
}
