//! Panels opened from a node card: the colour picker of a colour input and
//! the sockets of a Custom WGSL node.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, Point, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use shader_graph::nodes::custom_sockets;
use shader_graph::types::{HandleDef, WgslType};
use theme::{ActiveTheme as _, ColorPicker, Icon, IconName, popup};

use super::sheet::{Commit, boxed, field, heading, hint};
use super::type_picker::TypePicker;
use crate::editor::Editor;

pub struct ColorPopup {
    pub at: Point<Pixels>,
    pub picker: Entity<ColorPicker>,
    pub _subscription: Subscription,
}

pub struct SocketsPopup {
    pub node: String,
    pub at: Point<Pixels>,
    /// The socket whose last typed name was refused.
    pub rejected: Option<String>,
}

pub fn render_color(editor: &Editor, cx: &mut Context<Editor>) -> Option<AnyElement> {
    let popup = editor.color_popup.as_ref()?;
    let weak = cx.entity().downgrade();
    Some(
        popup::panel(
            cx,
            popup.at,
            move |_, cx| {
                super::update_editor(&weak, cx, |e, cx| {
                    e.color_popup = None;
                    cx.notify();
                });
            },
            popup.picker.clone(),
        )
        .into_any_element(),
    )
}

fn small_button(
    id: SharedString,
    label: impl IntoElement,
    enabled: bool,
    cx: &Context<Editor>,
    on_click: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
) -> impl IntoElement {
    let muted = cx.theme().colors.muted;
    let weak = cx.entity().downgrade();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size_6()
        .rounded_sm()
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(muted)))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            if enabled {
                super::update_editor(&weak, cx, |e, cx| on_click(e, cx));
            }
        })
        .child(label)
}

pub fn render_sockets(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<AnyElement> {
    let (node, at, rejected) = {
        let popup = editor.sockets.as_ref()?;
        (popup.node.clone(), popup.at, popup.rejected.clone())
    };
    let (sockets, locked) = {
        let model = editor.model();
        let data = model.node(&node)?.data.clone();
        (custom_sockets(&data), model.is_locked())
    };
    let theme = cx.theme().clone();
    let weak = cx.entity().downgrade();
    let mut panel = div().flex().flex_col().gap_4().p_2().w(px(380.0));
    for (is_input, label, hint_text, list) in [
        (
            true,
            "Inputs",
            "Read by the code as parameters.",
            &sockets.inputs,
        ),
        (
            false,
            "Outputs",
            "Variables the code sets; zero unless it does.",
            &sockets.outputs,
        ),
    ] {
        let add_node = node.clone();
        let mut section = div().flex().flex_col().gap_1p5().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(heading(label, cx))
                        .child(hint(hint_text, cx)),
                )
                .child(small_button(
                    SharedString::from(format!("add-socket-{label}")),
                    Icon::new(IconName::Plus).size(px(12.0)),
                    !locked,
                    cx,
                    move |e, cx| e.add_custom_socket(&add_node, is_input, cx),
                )),
        );
        let count = list.len();
        for (i, socket) in list.iter().enumerate() {
            section = section.child(socket_row(
                editor,
                window,
                &node,
                is_input,
                socket,
                i,
                count,
                locked,
                rejected.as_deref() == Some(&socket.id),
                cx,
            ));
        }
        panel = panel.child(section);
    }
    if rejected.is_some() {
        panel = panel.child(
            div().text_size(theme.text.xs).text_color(theme.colors.destructive).child(
                "A name must be a WGSL identifier (letters, digits and _, not starting with a digit or __), not a keyword or type, and not taken by another socket.",
            ),
        );
    }
    Some(
        popup::panel(
            cx,
            at,
            move |_, cx| {
                super::update_editor(&weak, cx, |e, cx| {
                    e.sockets = None;
                    cx.notify();
                });
            },
            panel,
        )
        .into_any_element(),
    )
}

#[allow(clippy::too_many_arguments)]
fn socket_row(
    editor: &mut Editor,
    window: &mut Window,
    node: &str,
    is_input: bool,
    socket: &HandleDef,
    index: usize,
    count: usize,
    locked: bool,
    rejected: bool,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let commit: Commit = {
        let (node, id) = (node.to_owned(), socket.id.clone());
        Rc::new(move |this, text, cx| this.rename_custom_socket(&node, is_input, &id, &text, cx))
    };
    let input = field(
        editor,
        format!("socket-{node}-{}", socket.id),
        &socket.label,
        "Name",
        window,
        cx,
        commit,
    );
    let (type_node, type_id) = (node.to_owned(), socket.id.clone());
    let picker = TypePicker {
        types: WgslType::ALL.to_vec(),
        value: Some(socket.ty),
        inferred: None,
        on_pick: Rc::new({
            let (node, id) = (type_node.clone(), type_id.clone());
            move |ty, e, cx| e.set_custom_socket_type(&node, is_input, &id, ty, cx)
        }),
        on_reset: None,
        extra: Vec::new(),
    };
    let picker = Rc::new(picker);
    let weak = cx.entity().downgrade();
    let type_button = div()
        .id(SharedString::from(format!("socket-type-{}", socket.id)))
        .w(px(84.0))
        .flex_none()
        .h_7()
        .px_2()
        .flex()
        .items_center()
        .rounded(theme.radius.md)
        .border_1()
        .border_color(theme.colors.input)
        .when(locked, |d| d.opacity(0.5))
        .when(!locked, |d| d.cursor_pointer())
        .on_mouse_down(MouseButton::Left, move |e, _, cx| {
            cx.stop_propagation();
            if !locked {
                let (at, picker) = (e.position, picker.clone());
                super::update_editor(&weak, cx, |ed, cx| {
                    let items = super::type_picker::items(&cx.entity().downgrade(), picker);
                    ed.open_list(at, items, cx);
                });
            }
        })
        .child(socket.ty.id());
    let (up, down, remove) = (
        (node.to_owned(), socket.id.clone()),
        (node.to_owned(), socket.id.clone()),
        (node.to_owned(), socket.id.clone()),
    );
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .flex_1()
                .when(rejected, |d| {
                    d.border_1().border_color(theme.colors.destructive)
                })
                .child(boxed(input, cx)),
        )
        .child(type_button)
        .child(small_button(
            SharedString::from(format!("up-{}", socket.id)),
            Icon::new(IconName::ArrowUp).size(px(12.0)),
            !locked && index > 0,
            cx,
            move |e, cx| e.move_custom_socket(&up.0, is_input, &up.1, -1, cx),
        ))
        .child(small_button(
            SharedString::from(format!("down-{}", socket.id)),
            Icon::new(IconName::ArrowDown).size(px(12.0)),
            !locked && index + 1 < count,
            cx,
            move |e, cx| e.move_custom_socket(&down.0, is_input, &down.1, 1, cx),
        ))
        .child(small_button(
            SharedString::from(format!("remove-{}", socket.id)),
            Icon::new(IconName::Trash).size(px(12.0)),
            !locked,
            cx,
            move |e, cx| e.remove_custom_socket(&remove.0, is_input, &remove.1, cx),
        ))
}
