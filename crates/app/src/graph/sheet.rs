//! The side sheet of a node: its name, and for a frame its note and colour,
//! for a group its interface (inputs, outputs and shared types), for a zone
//! its state items.

use guard::SafeListener as _;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use shader_graph::graph::{
    FrameTint, GraphModel, GroupSide, LAYOUT_FRAME, REROUTE, frame_note, frame_tint, set_frame_note,
    set_frame_tint,
};
use shader_graph::passes::{ItemSizeMode, TextureFormat};
use shader_graph::types::{HandleType, SOCKET_TYPES, TypeVariable, WgslType, type_set_of};
use shader_graph::zones::{DEFAULT_MAX_ITERATIONS, MAX_ITERATIONS, is_zone_node_type, zone_max_iterations};
use theme::{ActiveTheme as _, InputEvent, NumberEvent, NumberField, TextInput};

use super::frames::tint_color;
use super::type_picker::{TypePicker, on_editor};
use super::type_set_picker;
use crate::editor::{Editor, ListItem};

#[derive(Default)]
pub struct Fields {
    inputs: HashMap<String, Entity<TextInput>>,
    numbers: HashMap<String, Entity<NumberField>>,
}

type NumberCommit = Rc<dyn Fn(&mut Editor, f64, &mut Context<Editor>)>;
pub(super) type Commit = Rc<dyn Fn(&mut Editor, String, &mut Context<Editor>)>;

/// A text field kept across frames under `key`, showing `value` unless it's being edited.
pub(super) fn field(
    editor: &mut Editor,
    key: String,
    value: &str,
    placeholder: &str,
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
    let placeholder = placeholder.to_owned();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(cx).placeholder(placeholder);
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

/// A number field kept across frames under `key`.
fn number(
    editor: &mut Editor,
    key: String,
    value: f64,
    cx: &mut Context<Editor>,
    make: impl FnOnce(&mut Context<NumberField>) -> NumberField,
    commit: NumberCommit,
) -> Entity<NumberField> {
    if let Some(existing) = editor.sheet_fields.numbers.get(&key).cloned() {
        existing.update(cx, |n, cx| n.set_value(value, cx));
        return existing;
    }
    let field = cx.new(|cx| {
        let mut f = make(cx);
        f.set_value(value, cx);
        f
    });
    cx.subscribe(&field, move |this, _, NumberEvent::Change(v), cx| commit(this, *v, cx)).detach();
    editor.sheet_fields.numbers.insert(key, field.clone());
    field
}

fn type_label(ty: &HandleType, variables: &[TypeVariable]) -> String {
    match ty {
        HandleType::Fixed(t) => t.id().to_owned(),
        HandleType::Variable { variable } => variables
            .iter()
            .find(|v| v.id == *variable)
            .map_or_else(|| "?".to_owned(), |v| v.name.clone()),
        HandleType::Inferred { .. } => "Inferred".to_owned(),
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
        .on_click(cx.safe_listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
}

/// A bordered text button.
fn outline_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
    cx: &Context<Editor>,
    on_click: impl Fn(&mut Editor, gpui::Point<gpui::Pixels>, &mut Context<Editor>) + 'static,
) -> impl IntoElement {
    let theme = cx.theme();
    let (fg, input) = (theme.colors.foreground, theme.colors.input);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.0))
        .px_3()
        .rounded(theme.radius.md)
        .border_1()
        .border_color(input)
        .text_size(theme.text.sm)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.cursor_pointer().hover(move |s| s.bg(fg.opacity(0.08))))
        .on_click(cx.safe_listener(move |this, event: &gpui::ClickEvent, _, cx| {
            if !disabled {
                on_click(this, event.position(), cx);
            }
        }))
        .child(label.into())
}

pub(super) fn heading(text: &str, cx: &Context<Editor>) -> gpui::Div {
    let theme = cx.theme();
    div().text_size(theme.text.sm).font_weight(gpui::FontWeight::MEDIUM).child(text.to_owned())
}

pub(super) fn hint(text: &str, cx: &Context<Editor>) -> gpui::Div {
    let theme = cx.theme();
    div().text_size(theme.text.xs).text_color(theme.colors.muted_foreground).child(text.to_owned())
}

pub(super) fn boxed(input: Entity<TextInput>, cx: &Context<Editor>) -> gpui::Div {
    let theme = cx.theme();
    div().px_1().border_1().border_color(theme.colors.input).rounded(theme.radius.md).child(input)
}

/// What the sheet shows of the node, taken from the model.
struct Subject {
    label: Option<String>,
    shown_label: String,
    default_label: String,
    node_type: String,
    hide_links: bool,
    /// The group whose interface the sheet edits: the node, or the one a boundary node belongs to.
    group: Option<GroupSubject>,
    zone: Option<String>,
    frame: Option<(String, Option<FrameTint>)>,
    locked: bool,
}

struct GroupSubject {
    reference: String,
    label: String,
    node: Option<String>,
    inputs: Vec<shader_graph::types::GroupHandleDef>,
    outputs: Vec<shader_graph::types::GroupHandleDef>,
    variables: Vec<TypeVariable>,
}

fn subject(model: &GraphModel, node_id: &str) -> Option<Subject> {
    let node = model.node(node_id)?;
    let in_view = model.active_document().has_node(node_id);
    let group_ref = if node.definition_ref().is_some() {
        node.definition_ref().map(str::to_owned)
    } else if node.side().is_some() && in_view && !model.path().is_empty() {
        Some(model.active_definition_ref())
    } else {
        None
    };
    let group = group_ref.and_then(|reference| {
        let doc = model.project().document(&reference)?;
        Some(GroupSubject {
            label: doc.label.clone().unwrap_or_default(),
            node: if node.definition_ref().is_some() {
                Some(node_id.to_owned())
            } else {
                model.path().last().cloned()
            },
            inputs: doc.inputs.clone(),
            outputs: doc.outputs.clone(),
            variables: doc.type_variables.clone(),
            reference,
        })
    });
    let zone = if in_view && is_zone_node_type(&node.node_type) {
        model.active_zones().zone_of_boundary.get(node_id).cloned()
    } else {
        None
    };
    Some(Subject {
        label: node.data.label.clone(),
        shown_label: model.node_label(node_id),
        default_label: model.default_node_label(node_id),
        node_type: node.node_type.clone(),
        hide_links: node.data.hide_links == Some(true),
        locked: model.is_locked() || node.group.as_ref().is_some_and(|g| g.locked),
        frame: node.is(LAYOUT_FRAME).then(|| (frame_note(&node.data).unwrap_or_default().to_owned(), frame_tint(&node.data))),
        zone,
        group,
    })
}

pub fn render(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<AnyElement> {
    let node_id = editor.sheet.clone()?;
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let subject = subject(&editor.model(), &node_id)?;
    let locked = subject.locked;

    let mut body = div().flex().flex_col().gap_4().p_4();

    // Label
    let label_commit: Commit = {
        let id = node_id.clone();
        Rc::new(move |this, text, cx| {
            let id = id.clone();
            this.edit_model(cx, move |m: &mut GraphModel| {
                let text = text.trim().to_owned();
                let custom = (!text.is_empty() && text != m.default_node_label(&id)).then_some(text);
                if m.node(&id).is_some_and(|n| n.data.label != custom) {
                    m.set_node_label(&id, custom);
                }
            });
        })
    };
    let label_input = field(
        editor,
        format!("label-{node_id}"),
        subject.label.as_deref().unwrap_or(&subject.default_label),
        &subject.default_label,
        window,
        cx,
        label_commit,
    );
    let reset = {
        let id = node_id.clone();
        let default = subject.default_label.clone();
        let disabled = locked || subject.label.is_none();
        div()
            .id("reset-label")
            .flex()
            .items_center()
            .justify_center()
            .size_7()
            .flex_none()
            .rounded(theme.radius.md)
            .text_size(theme.text.sm)
            .when(disabled, |d| d.opacity(0.4))
            .when(!disabled, |d| d.cursor_pointer().hover(|s| s.bg(c.muted)))
            .tooltip(theme::Tooltip::text(format!("Reset to {default}")))
            .on_click(cx.safe_listener(move |this, _, _, cx| {
                if disabled {
                    return;
                }
                let id = id.clone();
                this.sheet_fields.inputs.remove(&format!("label-{id}"));
                this.edit_model(cx, move |m| m.set_node_label(&id, None));
            }))
            .child("↺")
    };
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(heading("Label", cx))
            .child(div().flex().items_center().gap_1().child(div().flex_1().child(boxed(label_input, cx))).child(reset)),
    );

    if subject.node_type == REROUTE {
        let id = node_id.clone();
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().mt(px(2.0)).child({
                    let weak = cx.entity().downgrade();
                    theme::Checkbox::new("hide-links").checked(subject.hide_links).disabled(locked).on_change(
                        move |_, _, cx| {
                            let ids = vec![id.clone()];
                            weak.update(cx, |e, cx| e.edit_model(cx, move |m| m.toggle_hidden_links(&ids))).ok();
                        },
                    )
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(heading("Hide links", cx))
                        .child(hint(
                            "Its links are drawn as tags named after it on the inputs they reach. Link it to an input from anywhere by dragging from the input and picking it in the add menu (Layout › Named reroutes).",
                            cx,
                        )),
                ),
        );
    }

    if let Some((note, tint)) = subject.frame.clone() {
        let commit: Commit = {
            let id = node_id.clone();
            Rc::new(move |this, text, cx| {
                let id = id.clone();
                let note = text.trim_end().to_owned();
                this.edit_model(cx, move |m: &mut GraphModel| {
                    m.update_node_data(&id, |d| set_frame_note(d, Some(&note)));
                });
            })
        };
        let input = field(editor, format!("note-{node_id}"), &note, "What this part of the graph does…", window, cx, commit);
        body = body
            .child(div().flex().flex_col().gap_1p5().child(heading("Note", cx)).child(boxed(input, cx)))
            .child(
                div().flex().flex_col().gap_1p5().child(heading("Colour", cx)).child(
                    div().flex().flex_wrap().gap_1p5().children(
                        std::iter::once(None).chain(FrameTint::ALL.into_iter().map(Some)).map(|option| {
                            let id = node_id.clone();
                            let name = option.map_or_else(|| "None".to_owned(), FrameTint::label);
                            div()
                                .id(SharedString::from(format!("tint-{name}")))
                                .size(px(28.0))
                                .rounded_full()
                                .border_1()
                                .border_color(c.border)
                                .bg(tint_color(option, &theme).opacity(0.6))
                                .when(tint == option, |d| d.border_2().border_color(c.primary))
                                .when(!locked, |d| d.cursor_pointer())
                                .tooltip(theme::Tooltip::text(name))
                                .on_click(cx.safe_listener(move |this, _, _, cx| {
                                    if locked {
                                        return;
                                    }
                                    let id = id.clone();
                                    this.edit_model(cx, move |m| m.update_node_data(&id, |d| set_frame_tint(d, option)));
                                }))
                        }),
                    ),
                ),
            );
    }

    if let Some(group) = subject.group {
        body = group_settings(editor, body, group, locked, window, cx);
    }
    if let Some(zone) = subject.zone {
        body = zone_settings(editor, body, &zone, locked, window, cx);
    }

    Some(
        div()
            .id("sheet")
            .w(px(350.0))
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
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(subject.shown_label.clone()))
                            .children((subject.label.is_some()).then(|| hint(&subject.default_label, cx))),
                    )
                    .child(button("close-sheet", "×", cx, |this, cx| this.close_sheet(cx))),
            )
            .child(body)
            .into_any_element(),
    )
}

fn group_settings(
    editor: &mut Editor,
    mut body: gpui::Div,
    group: GroupSubject,
    locked: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> gpui::Div {
    let theme = cx.theme().clone();
    let reference = group.reference.clone();
    let weak = cx.entity().downgrade();

    if let Some(node) = group.node.clone() {
        body = body.child(outline_button("clone-edit", "Clone & Edit", false, cx, {
            let node = node.clone();
            move |this, _, cx| this.clone_group(&node, cx)
        }));
        let commit: Commit = {
            let node = node.clone();
            Rc::new(move |this, text, cx| {
                let node = node.clone();
                this.edit_model(cx, move |m: &mut GraphModel| m.set_group_label(&node, &text));
            })
        };
        let input = field(editor, format!("group-{reference}"), &group.label, "Group name", window, cx, commit);
        body = body.child(div().flex().flex_col().gap_1p5().child(heading("Group name", cx)).child(boxed(input, cx)));
    }

    if !group.variables.is_empty() || !locked {
        let mut section = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(heading("Shared types", cx))
            .child(hint(
                "Sockets sharing a type take the same one on each group node, inferred from what's linked to them or picked on the node.",
                cx,
            ));
        for variable in &group.variables {
            let name_commit: Commit = {
                let (reference, id) = (reference.clone(), variable.id.clone());
                Rc::new(move |this, text, cx| {
                    let (reference, id) = (reference.clone(), id.clone());
                    this.edit_model(cx, move |m| m.set_type_variable(&reference, &id, Some(&text), None));
                })
            };
            let name = field(editor, format!("variable-{}", variable.id), &variable.name, "Name", window, cx, name_commit);
            let (w, r, vid, types) = (weak.clone(), reference.clone(), variable.id.clone(), variable.types.clone());
            let types_button = outline_button(
                SharedString::from(format!("types-{}", variable.id)),
                type_set_picker::label(&variable.types),
                locked,
                cx,
                move |this, at, cx| {
                    let (r, vid) = (r.clone(), vid.clone());
                    let change: type_set_picker::Change = Rc::new(move |next, e, cx| {
                        let (r, vid) = (r.clone(), vid.clone());
                        e.edit_model(cx, move |m| m.set_type_variable(&r, &vid, None, Some(&next)));
                    });
                    let items = type_set_picker::items(&w, at, types.clone(), change);
                    this.open_list(at, items, cx);
                },
            );
            let sockets = group
                .inputs
                .iter()
                .chain(&group.outputs)
                .filter(|h| h.ty.variable() == Some(variable.id.as_str()))
                .count();
            let (r, vid) = (reference.clone(), variable.id.clone());
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(56.0)).flex_none().child(boxed(name, cx)))
                    .child(div().flex().flex_1().min_w_0().child(types_button))
                    .child(
                        div()
                            .w(px(56.0))
                            .flex_none()
                            .text_size(theme.text.xs)
                            .text_color(theme.colors.muted_foreground)
                            .child(format!("{sockets} {}", if sockets == 1 { "socket" } else { "sockets" })),
                    )
                    .when(!locked, |d| {
                        d.child(button(SharedString::from(format!("rm-{}", variable.id)), "×", cx, move |this, cx| {
                            let (r, vid) = (r.clone(), vid.clone());
                            this.edit_model(cx, move |m| m.remove_type_variable(&r, &vid));
                        }))
                    }),
            );
        }
        if !locked {
            let r = reference.clone();
            section = section.child(outline_button("add-variable", "Add shared type", false, cx, move |this, _, cx| {
                let r = r.clone();
                this.edit_model(cx, move |m| {
                    m.add_type_variable(&r, shader_graph::types::FLOAT_TYPES);
                });
            }));
        }
        body = body.child(section);
    }

    for (side, title, handles) in [
        (GroupSide::Input, "Inputs", group.inputs.clone()),
        (GroupSide::Output, "Outputs", group.outputs.clone()),
    ] {
        let mut section = div().flex().flex_col().gap_1p5().child(heading(title, cx));
        for handle in handles {
            let commit: Commit = {
                let (reference, id) = (reference.clone(), handle.id.clone());
                Rc::new(move |this, text, cx| {
                    let (reference, id) = (reference.clone(), id.clone());
                    this.edit_model(cx, move |m| {
                        m.set_handle_label(&reference, side, &id, &text);
                    });
                })
            };
            let input = field(editor, format!("handle-{reference}-{}", handle.id), &handle.label, "", window, cx, commit);
            let current = type_label(&handle.ty, &group.variables);
            let (w, r, hid, variables) = (weak.clone(), reference.clone(), handle.id.clone(), group.variables.clone());
            let handle_ty = handle.ty.clone();
            let type_button = outline_button(
                SharedString::from(format!("type-{}", handle.id)),
                current,
                locked,
                cx,
                move |this, at, cx| {
                    let mut extra: Vec<ListItem> = variables
                        .iter()
                        .map(|v| {
                            let (r, hid, vid) = (r.clone(), hid.clone(), v.id.clone());
                            ListItem::new(
                                v.name.clone(),
                                Some(handle_ty.variable() == Some(v.id.as_str())),
                                on_editor(&w, move |e, cx| {
                                    let (r, hid, vid) = (r.clone(), hid.clone(), vid.clone());
                                    e.edit_model(cx, move |m| m.set_handle_type(&r, side, &hid, HandleType::Variable { variable: vid }));
                                }),
                            )
                            .detail(type_set_of(&v.types).map_or("Custom", |s| s.label))
                        })
                        .collect();
                    if side == GroupSide::Output {
                        let (r, hid) = (r.clone(), hid.clone());
                        let inferred = ListItem::new(
                            "Inferred",
                            Some(handle_ty.is_inferred()),
                            on_editor(&w, move |e, cx| {
                                let (r, hid) = (r.clone(), hid.clone());
                                e.edit_model(cx, move |m| m.set_handle_type(&r, side, &hid, HandleType::inferred()));
                            }),
                        )
                        .detail("What reaches it");
                        extra.push(inferred);
                    }
                    let (r2, hid2) = (r.clone(), hid.clone());
                    let picker = TypePicker {
                        types: SOCKET_TYPES.to_vec(),
                        value: match &handle_ty {
                            HandleType::Fixed(t) => Some(*t),
                            _ => None,
                        },
                        inferred: None,
                        on_pick: Rc::new(move |ty, e, cx| {
                            let (r, hid) = (r2.clone(), hid2.clone());
                            e.edit_model(cx, move |m| m.set_handle_type(&r, side, &hid, HandleType::Fixed(ty)));
                        }),
                        on_reset: None,
                        extra,
                    };
                    this.open_type_picker(at, picker, cx);
                },
            );
            let (r1, h1) = (reference.clone(), handle.id.clone());
            let (r2, h2) = (reference.clone(), handle.id.clone());
            let (r3, h3) = (reference.clone(), handle.id.clone());
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(boxed(input, cx)))
                    .child(div().w(px(96.0)).flex_none().flex().child(type_button))
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
            );
        }
        if !locked {
            let (w, r) = (weak.clone(), reference.clone());
            section = section.child(outline_button(
                SharedString::from(format!("add-{title}")),
                if side == GroupSide::Input { "Add input" } else { "Add output" },
                false,
                cx,
                move |this, at, cx| {
                    let (r, r2) = (r.clone(), r.clone());
                    let extra: Vec<ListItem> = (side == GroupSide::Output)
                        .then(|| {
                            ListItem::new(
                                "Inferred",
                                None,
                                on_editor(&w, move |e, cx| {
                                    let r = r2.clone();
                                    e.edit_model(cx, move |m| {
                                        m.add_handle(&r, side, HandleType::inferred(), None, None);
                                    });
                                }),
                            )
                            .detail("What reaches it")
                        })
                        .into_iter()
                        .collect();
                    this.open_type_picker(
                        at,
                        TypePicker {
                            types: SOCKET_TYPES.to_vec(),
                            value: None,
                            inferred: None,
                            on_pick: Rc::new(move |ty, e, cx| {
                                let r = r.clone();
                                e.edit_model(cx, move |m| {
                                    m.add_handle(&r, side, HandleType::Fixed(ty), None, None);
                                });
                            }),
                            on_reset: None,
                            extra,
                        },
                        cx,
                    );
                },
            ));
        }
        body = body.child(section);
    }
    body
}

fn zone_settings(
    editor: &mut Editor,
    mut body: gpui::Div,
    zone: &str,
    locked: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> gpui::Div {
    let weak = cx.entity().downgrade();
    let (nodes, items, variables) = {
        let model = editor.model();
        let Some(nodes) = model.zone_nodes(zone) else { return body };
        (nodes, model.zone_items_of(zone), model.active_document().type_variables.clone())
    };
    let simulation = nodes.kind == shader_graph::zones::ZoneKind::Simulation;
    let (title, description) = if simulation {
        (
            "Simulation zone",
            "Nodes linked from the Simulation Input run once per frame, from the previous frame's state; each pixel keeps its own. A simulation starts from its initial values on the first frame, and whenever time goes back. A texture item keeps an image, which can be sampled anywhere.",
        )
    } else {
        (
            "Repeat zone",
            "Nodes linked from the Repeat Input run once per iteration. Each state item carries a value from one iteration to the next.",
        )
    };
    body = body.child(div().flex().flex_col().gap_1p5().child(heading(title, cx)).child(hint(description, cx)));

    if !simulation {
        let id = zone.to_owned();
        let max = zone_max_iterations(&nodes.input);
        let field = number(
            editor,
            format!("max-{zone}"),
            f64::from(max),
            cx,
            |cx| NumberField::new(f64::from(DEFAULT_MAX_ITERATIONS), cx).integer().min(0.0).max(f64::from(MAX_ITERATIONS)),
            Rc::new(move |this, value, cx| {
                let id = id.clone();
                this.edit_model(cx, move |m| m.set_zone_max_iterations(&id, value as u32));
            }),
        );
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(heading("Maximum iterations", cx))
                .child(div().when(locked, |d| d.opacity(0.5).occlude()).child(field))
                .child(hint("The iteration count is clamped to this value, so a large or animated count can't stall the GPU.", cx)),
        );
    }

    let item_types: Vec<WgslType> = if simulation { SOCKET_TYPES.to_vec() } else { WgslType::ALL.to_vec() };
    let mut section = div().flex().flex_col().gap_1p5().child(heading("State items", cx));
    for item in items {
        let commit: Commit = {
            let (zone, id) = (zone.to_owned(), item.id.clone());
            Rc::new(move |this, text, cx| {
                let (zone, id) = (zone.clone(), id.clone());
                this.edit_model(cx, move |m| {
                    m.rename_zone_item(&zone, &id, &text);
                });
            })
        };
        let input = field(editor, format!("item-{zone}-{}", item.id), &item.label, "", window, cx, commit);
        let shared = item.ty.variable().and_then(|v| variables.iter().find(|x| x.id == v)).cloned();
        let label = shared
            .as_ref()
            .map(|v| v.name.clone())
            .or_else(|| item.ty.is_inferred().then(|| "Inferred".to_owned()))
            .unwrap_or_else(|| type_label(&item.ty, &variables));
        let (w, z, iid, ty, vars, types) =
            (weak.clone(), zone.to_owned(), item.id.clone(), item.ty.clone(), variables.clone(), item_types.clone());
        let type_button = outline_button(SharedString::from(format!("item-type-{}", item.id)), label, locked, cx, move |this, at, cx| {
            let mut extra = vec![{
                let (z, iid) = (z.clone(), iid.clone());
                ListItem::new(
                    "Inferred",
                    Some(ty.is_inferred()),
                    on_editor(&w, move |e, cx| {
                        let (z, iid) = (z.clone(), iid.clone());
                        e.edit_model(cx, move |m| m.set_zone_item_type(&z, &iid, HandleType::inferred()));
                    }),
                )
                .detail("Initial value")
            }];
            for (i, v) in vars.iter().enumerate() {
                let (z, iid, vid) = (z.clone(), iid.clone(), v.id.clone());
                let mut entry = ListItem::new(
                    format!("{} (shared)", v.name),
                    Some(ty.variable() == Some(v.id.as_str())),
                    on_editor(&w, move |e, cx| {
                        let (z, iid, vid) = (z.clone(), iid.clone(), vid.clone());
                        e.edit_model(cx, move |m| m.set_zone_item_type(&z, &iid, HandleType::Variable { variable: vid }));
                    }),
                );
                if i == 0 {
                    entry = entry.separated();
                }
                extra.push(entry);
            }
            let (z, iid) = (z.clone(), iid.clone());
            this.open_type_picker(
                at,
                TypePicker {
                    types: types.clone(),
                    value: match &ty {
                        HandleType::Fixed(t) => Some(*t),
                        _ => None,
                    },
                    inferred: None,
                    on_pick: Rc::new(move |t, e, cx| {
                        let (z, iid) = (z.clone(), iid.clone());
                        e.edit_model(cx, move |m| m.set_zone_item_type(&z, &iid, HandleType::Fixed(t)));
                    }),
                    on_reset: None,
                    extra,
                },
                cx,
            );
        });

        let mut row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(div().flex_1().min_w_0().child(boxed(input, cx)))
            .child(div().w(px(88.0)).flex_none().flex().child(type_button));
        if simulation {
            let (w, z, iid) = (weak.clone(), zone.to_owned(), item.id.clone());
            let current = item.format.clone();
            row = row.child(div().w(px(72.0)).flex_none().flex().child(outline_button(
                SharedString::from(format!("item-format-{}", item.id)),
                current.clone().unwrap_or_else(|| "Auto".to_owned()),
                locked,
                cx,
                move |this, at, cx| {
                    let mut entries = vec![{
                        let (z, iid) = (z.clone(), iid.clone());
                        ListItem::new(
                            "Auto",
                            Some(current.is_none()),
                            on_editor(&w, move |e, cx| {
                                let (z, iid) = (z.clone(), iid.clone());
                                e.edit_model(cx, move |m| m.set_zone_item_format(&z, &iid, None));
                            }),
                        )
                    }];
                    for format in TextureFormat::ALL {
                        let (z, iid) = (z.clone(), iid.clone());
                        entries.push(ListItem::new(
                            format.id(),
                            Some(current.as_deref() == Some(format.id())),
                            on_editor(&w, move |e, cx| {
                                let (z, iid) = (z.clone(), iid.clone());
                                e.edit_model(cx, move |m| m.set_zone_item_format(&z, &iid, Some(format)));
                            }),
                        ));
                    }
                    this.open_list(at, entries, cx);
                },
            )));
            let (w, z, iid) = (weak.clone(), zone.to_owned(), item.id.clone());
            let size = item.size.clone();
            row = row.child(div().w(px(64.0)).flex_none().flex().child(outline_button(
                SharedString::from(format!("item-size-{}", item.id)),
                match size.as_deref() {
                    Some("scale") => "Scale",
                    Some("pixels") => "Pixels",
                    _ => "Canvas",
                },
                locked,
                cx,
                move |this, at, cx| {
                    let choices: [(&str, Option<ItemSizeMode>); 3] =
                        [("Canvas", None), ("Scale", Some(ItemSizeMode::Scale)), ("Pixels", Some(ItemSizeMode::Pixels))];
                    let entries = choices
                        .into_iter()
                        .map(|(label, mode)| {
                            let (z, iid) = (z.clone(), iid.clone());
                            ListItem::new(
                                label,
                                Some(size.as_deref() == mode.map(ItemSizeMode::id)),
                                on_editor(&w, move |e, cx| {
                                    let (z, iid) = (z.clone(), iid.clone());
                                    e.edit_model(cx, move |m| m.set_zone_item_size(&z, &iid, mode));
                                }),
                            )
                        })
                        .collect();
                    this.open_list(at, entries, cx);
                },
            )));
        }
        let (z1, i1) = (zone.to_owned(), item.id.clone());
        let (z2, i2) = (zone.to_owned(), item.id.clone());
        let (z3, i3) = (zone.to_owned(), item.id.clone());
        section = section.child(row.when(!locked, |d| {
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
        }));
    }
    if !locked {
        let (w, z, types) = (weak, zone.to_owned(), item_types);
        section = section.child(outline_button("add-item", "Add item", false, cx, move |this, at, cx| {
            let (z, z2) = (z.clone(), z.clone());
            let inferred = ListItem::new(
                "Inferred",
                None,
                on_editor(&w, move |e, cx| {
                    let z = z2.clone();
                    e.edit_model(cx, move |m| {
                        m.add_zone_item(&z, HandleType::inferred(), None);
                    });
                }),
            )
            .detail("Initial value");
            this.open_type_picker(
                at,
                TypePicker {
                    types: types.clone(),
                    value: None,
                    inferred: None,
                    on_pick: Rc::new(move |t, e, cx| {
                        let z = z.clone();
                        e.edit_model(cx, move |m| {
                            m.add_zone_item(&z, HandleType::Fixed(t), None);
                        });
                    }),
                    on_reset: None,
                    extra: vec![inferred],
                },
                cx,
            );
        }));
    }
    body.child(section)
}
