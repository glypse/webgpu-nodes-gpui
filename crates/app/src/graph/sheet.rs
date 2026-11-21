//! The side sheet of a node: its name, and for a group its interface (inputs,
//! outputs and type variables), for a zone its state items.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, Focusable as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use shader_graph::graph::{GraphModel, GroupSide};
use shader_graph::types::{HandleType, WgslType};
use shader_graph::zones::{is_zone_node_type, paired_output_id};
use theme::{ActiveTheme as _, InputEvent, TextInput};

use super::fields::picker;
use crate::editor::{Editor, ListItem};

#[derive(Default)]
pub struct Fields {
    inputs: HashMap<String, Entity<TextInput>>,
}

type Commit = Rc<dyn Fn(&mut Editor, String, &mut Context<Editor>)>;

/// A text field kept across frames under `key`, showing `value` unless it's being edited.
fn field(
    editor: &mut Editor,
    key: String,
    value: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
    commit: Commit,
) -> Entity<TextInput> {
    if let Some(existing) = editor.sheet_fields.inputs.get(&key).cloned() {
        let focused = existing.read(cx).focus_handle(cx).is_focused(window);
        if !focused && existing.read(cx).value().as_ref() != value {
            existing.update(cx, |i, cx| i.set_value(value.to_owned(), cx));
        }
        return existing;
    }
    let input = cx.new(|cx| {
        let mut input = TextInput::new(cx);
        input.set_value(value.to_owned(), cx);
        input
    });
    cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Submit | InputEvent::Blur) {
            let text = input.read(cx).value().to_string();
            commit(this, text, cx);
        }
    })
    .detach();
    editor.sheet_fields.inputs.insert(key, input.clone());
    input
}

fn type_label(ty: &HandleType, variables: &[shader_graph::types::TypeVariable]) -> String {
    match ty {
        HandleType::Fixed(t) => t.id().to_owned(),
        HandleType::Variable { variable } => variables
            .iter()
            .find(|v| v.id == *variable)
            .map_or_else(|| "?".to_owned(), |v| v.name.clone()),
        HandleType::Inferred { .. } => "inferred".to_owned(),
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    cx: &Context<Editor>,
    on_click: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    let (fg, muted) = (colors.foreground, colors.muted);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size_6()
        .flex_none()
        .rounded_sm()
        .text_size(px(12.0))
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(muted))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
}

pub fn render(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<AnyElement> {
    let node_id = editor.sheet.clone()?;
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let (label, reference, is_group, locked, zone_id, interface, variables) = {
        let model = editor.model();
        let node = model.node(&node_id)?;
        let reference = node.definition_ref().map(str::to_owned);
        let doc = reference.as_deref().and_then(|r| model.project().document(r));
        let zone = if is_zone_node_type(&node.node_type) {
            model.active_zones().zone_of_boundary.get(&node_id).cloned()
        } else {
            None
        };
        let _ = paired_output_id;
        (
            model.node_label(&node_id),
            reference,
            node.group.is_some(),
            model.is_locked() || node.group.as_ref().is_some_and(|g| g.locked),
            zone,
            doc.map(|d| (d.inputs.clone(), d.outputs.clone())),
            doc.map(|d| d.type_variables.clone()).unwrap_or_default(),
        )
    };

    let mut body = div().flex().flex_col().gap_2().p_3();
    let name_key = format!("label-{node_id}");
    let name_commit: Commit = {
        let id = node_id.clone();
        Rc::new(move |this, text, cx| {
            let id = id.clone();
            this.edit_model(cx, move |m: &mut GraphModel| {
                if m.node(&id).is_some_and(|n| n.group.is_some()) {
                    m.set_group_label(&id, &text);
                } else {
                    let t = text.trim().to_owned();
                    m.set_node_label(&id, if t.is_empty() { None } else { Some(t) });
                }
            });
        })
    };
    let name = field(editor, name_key, &label, window, cx, name_commit);
    body = body
        .child(div().text_size(theme.text.xs).text_color(c.muted_foreground).child("Name"))
        .child(div().px_1().border_1().border_color(c.input).rounded(theme.radius.md).child(name));

    if let (Some(reference), Some((inputs, outputs))) = (reference.clone(), interface) {
        for (side, title, handles) in [(GroupSide::Input, "Inputs", inputs), (GroupSide::Output, "Outputs", outputs)] {
            body = body.child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(theme.text.sm).child(title))
                    .when(!locked, |d| {
                        let (reference, node) = (reference.clone(), node_id.clone());
                        d.child(button(SharedString::from(format!("add-{title}")), "+", cx, move |this, cx| {
                            let (reference, node) = (reference.clone(), node.clone());
                            this.edit_model(cx, move |m| {
                                m.add_handle(&reference, side, HandleType::Fixed(WgslType::F32), None, None);
                            });
                            let _ = node;
                        }))
                    }),
            );
            let count = handles.len();
            for (index, handle) in handles.into_iter().enumerate() {
                let key = format!("handle-{reference}-{}", handle.id);
                let commit: Commit = {
                    let (reference, id) = (reference.clone(), handle.id.clone());
                    Rc::new(move |this, text, cx| {
                        let (reference, id) = (reference.clone(), id.clone());
                        this.edit_model(cx, move |m| {
                            m.set_handle_label(&reference, side, &id, &text);
                        });
                    })
                };
                let input = field(editor, key, &handle.label, window, cx, commit);
                let current = type_label(&handle.ty, &variables);
                let weak = cx.entity().downgrade();
                let (r, hid, vars) = (reference.clone(), handle.id.clone(), variables.clone());
                let type_picker = picker(
                    SharedString::from(format!("type-{}", handle.id)),
                    current,
                    1.0,
                    locked,
                    cx,
                    move |at, _, cx| {
                        let mut items: Vec<ListItem> = WgslType::ALL
                            .iter()
                            .map(|t| {
                                let (weak, r, hid, t) = (weak.clone(), r.clone(), hid.clone(), *t);
                                ListItem::new(t.id(), None, Rc::new(move |_, cx| {
                                    let (r, hid) = (r.clone(), hid.clone());
                                    weak.update(cx, |e, cx| e.edit_model(cx, move |m| m.set_handle_type(&r, side, &hid, HandleType::Fixed(t)))).ok();
                                }))
                            })
                            .collect();
                        for v in &vars {
                            let (weak, r, hid, vid) = (weak.clone(), r.clone(), hid.clone(), v.id.clone());
                            items.push(ListItem::new(format!("{} (variable)", v.name), None, Rc::new(move |_, cx| {
                                let (r, hid, vid) = (r.clone(), hid.clone(), vid.clone());
                                weak.update(cx, |e, cx| e.edit_model(cx, move |m| m.set_handle_type(&r, side, &hid, HandleType::Variable { variable: vid }))).ok();
                            })));
                        }
                        weak.update(cx, |e, cx| e.open_list(at, items, cx)).ok();
                    },
                );
                let (r1, h1) = (reference.clone(), handle.id.clone());
                let (r2, h2) = (reference.clone(), handle.id.clone());
                let (r3, h3) = (reference.clone(), handle.id.clone());
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().px_1().border_1().border_color(c.input).rounded(theme.radius.sm).child(input))
                        .child(div().w(px(84.0)).flex().child(type_picker))
                        .when(!locked, |d| {
                            d.child(button(SharedString::from(format!("up-{}", handle.id)), "↑", cx, move |this, cx| {
                                let (r, h) = (r1.clone(), h1.clone());
                                this.edit_model(cx, move |m| m.move_handle(&r, side, &h, -1));
                            }))
                            .child(button(SharedString::from(format!("down-{}", handle.id)), "↓", cx, move |this, cx| {
                                let (r, h) = (r2.clone(), h2.clone());
                                this.edit_model(cx, move |m| m.move_handle(&r, side, &h, 1));
                            }))
                            .child(button(SharedString::from(format!("del-{}", handle.id)), "×", cx, move |this, cx| {
                                let (r, h) = (r3.clone(), h3.clone());
                                this.edit_model(cx, move |m| m.remove_handle(&r, side, &h));
                            }))
                        })
                        .when(index >= count, |d| d),
                );
            }
        }
        // Type variables.
        body = body.child(
            div()
                .mt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(theme.text.sm).child("Type variables"))
                .when(!locked, |d| {
                    let reference = reference.clone();
                    d.child(button("add-variable", "+", cx, move |this, cx| {
                        let reference = reference.clone();
                        this.edit_model(cx, move |m| {
                            m.add_type_variable(&reference, &[WgslType::F32, WgslType::Vec2f, WgslType::Vec3f, WgslType::Vec4f]);
                        });
                    }))
                }),
        );
        for v in &variables {
            let (r, vid) = (reference.clone(), v.id.clone());
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(theme.text.sm)
                    .child(format!("{}: {}", v.name, v.types.iter().map(|t| t.id()).collect::<Vec<_>>().join(", ")))
                    .when(!locked, |d| {
                        d.child(button(SharedString::from(format!("rm-{vid}")), "×", cx, move |this, cx| {
                            let (r, vid) = (r.clone(), vid.clone());
                            this.edit_model(cx, move |m| m.remove_type_variable(&r, &vid));
                        }))
                    }),
            );
        }
    }

    if let Some(zone) = zone_id.clone() {
        let items = editor.model().zone_items_of(&zone);
        body = body.child(div().mt_2().text_size(theme.text.sm).child("State items"));
        for item in items {
            let key = format!("item-{zone}-{}", item.id);
            let commit: Commit = {
                let (zone, id) = (zone.clone(), item.id.clone());
                Rc::new(move |this, text, cx| {
                    let (zone, id) = (zone.clone(), id.clone());
                    this.edit_model(cx, move |m| {
                        m.rename_zone_item(&zone, &id, &text);
                    });
                })
            };
            let input = field(editor, key, &item.label, window, cx, commit);
            let (z1, i1) = (zone.clone(), item.id.clone());
            let (z2, i2) = (zone.clone(), item.id.clone());
            let (z3, i3) = (zone.clone(), item.id.clone());
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().px_1().border_1().border_color(c.input).rounded(theme.radius.sm).child(input))
                    .when(!locked, |d| {
                        d.child(button(SharedString::from(format!("iup-{}", item.id)), "↑", cx, move |this, cx| {
                            let (z, i) = (z1.clone(), i1.clone());
                            this.edit_model(cx, move |m| m.move_zone_item(&z, &i, -1));
                        }))
                        .child(button(SharedString::from(format!("idown-{}", item.id)), "↓", cx, move |this, cx| {
                            let (z, i) = (z2.clone(), i2.clone());
                            this.edit_model(cx, move |m| m.move_zone_item(&z, &i, 1));
                        }))
                        .child(button(SharedString::from(format!("idel-{}", item.id)), "×", cx, move |this, cx| {
                            let (z, i) = (z3.clone(), i3.clone());
                            this.edit_model(cx, move |m| m.remove_zone_item(&z, &i));
                        }))
                    }),
            );
        }
    }

    let _ = is_group;
    Some(
        div()
            .id("sheet")
            .w(px(300.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(c.border)
            .bg(c.card)
            .text_size(theme.text.sm)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .px_3()
                    .border_b_1()
                    .border_color(c.border)
                    .child("Settings")
                    .child(button("close-sheet", "×", cx, |this, cx| this.close_sheet(cx))),
            )
            .child(body)
            .into_any_element(),
    )
}
