//! What a group input holds beyond its name and type: its default value,
//! the options of an enum, the range its number is edited in, and how a
//! colour is edited.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::Disableable as _;
use gpui_component::checkbox::Checkbox;
use shader_graph::enums::next_variant_value;
use shader_graph::types::{
    ColorDisplay, EnumVariant, GroupHandleDef, HandleType, InputValue, ScalarKind, TypeVariable,
    WgslType,
};
use theme::{ActiveTheme as _, Icon, IconName};

use super::fields::{Edit, Scrub};
use super::sheet::{Commit, button, field, hint, outline_button};
use super::type_picker::on_editor;
use crate::editor::{Editor, ListItem};

type Action = Rc<dyn Fn(&mut Editor, &mut Context<Editor>)>;
type RangeAction = Rc<dyn Fn(&mut Editor, Option<f64>, &mut Context<Editor>)>;

/// The type the handle's value is edited as, when it has one.
fn edited_type(handle: &GroupHandleDef, variables: &[TypeVariable]) -> Option<WgslType> {
    match &handle.ty {
        HandleType::Fixed(t) => Some(*t),
        HandleType::Variable { variable } => variables
            .iter()
            .find(|v| v.id == *variable)?
            .types
            .first()
            .copied(),
        HandleType::Inferred { .. } => None,
    }
}

fn label(text: &str, cx: &Context<Editor>) -> gpui::Div {
    div()
        .text_size(cx.theme().text.xs)
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.to_owned())
}

#[allow(clippy::too_many_arguments)]
pub fn details(
    editor: &mut Editor,
    window: &mut Window,
    reference: &str,
    handle: &GroupHandleDef,
    defaults: &BTreeMap<String, InputValue>,
    variables: &[TypeVariable],
    locked: bool,
    cx: &mut Context<Editor>,
) -> gpui::Div {
    let theme = cx.theme().clone();
    let weak = cx.entity().downgrade();
    let mut column = div()
        .flex()
        .flex_col()
        .gap_3()
        .ml_2()
        .pl_3()
        .pb_2()
        .border_l_1()
        .border_color(theme.colors.border);
    let Some(ty) = edited_type(handle, variables) else {
        return column;
    };
    let toggle = handle.variants.is_some() && ty == WgslType::Bool;
    let stored = defaults.get(&handle.id).copied();

    // Default value
    let value_editor = if let (Some(variants), false) = (&handle.variants, toggle) {
        let current = shader_graph::types::enum_value(stored.as_ref(), variants);
        let items: Vec<ListItem> = variants
            .iter()
            .map(|v| {
                let (r, id, value) = (reference.to_owned(), handle.id.clone(), v.value);
                ListItem::new(
                    v.label.clone(),
                    Some(Some(value) == current),
                    on_editor(&weak, move |e, cx| {
                        let (r, id) = (r.clone(), id.clone());
                        e.edit(cx, move |m| {
                            m.set_handle_default(&r, &id, Some(InputValue::from(value as f64)))
                        });
                    }),
                )
            })
            .collect();
        let name = variants
            .iter()
            .find(|v| Some(v.value) == current)
            .map_or_else(String::new, |v| v.label.clone());
        let w = weak.clone();
        div().flex().child(super::fields::picker(
            SharedString::from(format!("default-{}", handle.id)),
            name,
            1.0,
            locked,
            cx,
            move |at, _, cx| {
                let items = items.clone();
                super::update_editor(&w, cx, |e, cx| e.open_list(at, items, cx));
            },
        ))
    } else {
        let integer = ty.kind() != ScalarKind::F32;
        let toggle_values = toggle;
        div().flex().gap_1().children((0..ty.components()).map(|i| {
            let value = stored.as_ref().map_or(0.0, |v| v.component(i));
            let (w, r, id) = (weak.clone(), reference.to_owned(), handle.id.clone());
            let mut scrub = Scrub::new(
                SharedString::from(format!("default-{}-{i}", handle.id)),
                value,
                1.0,
            )
            .integer(integer || toggle_values)
            .readonly(locked)
            .on_change(move |v, _: Edit, _, cx| {
                let (r, id) = (r.clone(), id.clone());
                super::update_editor(&w, cx, |e, cx| {
                    e.edit(cx, move |m| {
                        let current = m
                            .project()
                            .document(&r)
                            .and_then(|d| d.defaults.get(&id).copied());
                        m.set_handle_default(
                            &r,
                            &id,
                            Some(InputValue::with_component(current.as_ref(), ty, i, v)),
                        );
                    });
                });
            });
            if toggle_values {
                scrub = scrub.range(Some(0.0), Some(1.0));
            } else {
                scrub = scrub.range(handle.min, handle.max);
            }
            scrub
        }))
    };
    column = column.child(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label("Default value", cx))
            .child(value_editor),
    );

    // Options of an enum
    if let (Some(options), false) = (&handle.variants, toggle) {
        let mut list = div().flex().flex_col().gap_1().child(label("Options", cx));
        let write = {
            let (r, id) = (reference.to_owned(), handle.id.clone());
            Rc::new(
                move |this: &mut Editor, variants: Vec<EnumVariant>, cx: &mut Context<Editor>| {
                    let (r, id) = (r.clone(), id.clone());
                    this.edit(cx, move |m| m.set_handle_variants(&r, &id, Some(variants)));
                },
            )
        };
        for (i, variant) in options.iter().enumerate() {
            let commit: Commit = {
                let (options, write, value) = (options.clone(), write.clone(), variant.value);
                Rc::new(move |this, text, cx| {
                    let mut next = options.clone();
                    if let Some(v) = next.iter_mut().find(|v| v.value == value) {
                        v.label = text;
                    }
                    write(this, next, cx);
                })
            };
            let input = field(
                editor,
                format!("variant-{}-{}", handle.id, variant.value),
                &variant.label,
                "",
                window,
                cx,
                commit,
            );
            let action =
                |glyph: IconName, id: String, enabled: bool, f: Action, cx: &Context<Editor>| {
                    div().when(!enabled, |d| d.opacity(0.4)).child(button(
                        SharedString::from(id),
                        Icon::new(glyph).size(px(14.0)),
                        cx,
                        move |e, cx| {
                            if enabled {
                                f(e, cx);
                            }
                        },
                    ))
                };
            let (up_opts, down_opts, remove_opts) =
                (options.clone(), options.clone(), options.clone());
            let (w1, w2, w3) = (write.clone(), write.clone(), write.clone());
            let count = options.len();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(super::sheet::boxed(input, cx)),
                    )
                    .when(!locked, |d| {
                        d.child(action(
                            IconName::ArrowUp,
                            format!("vup-{}-{i}", handle.id),
                            i > 0,
                            Rc::new(move |e, cx| {
                                let mut next = up_opts.clone();
                                next.swap(i, i - 1);
                                w1(e, next, cx);
                            }),
                            cx,
                        ))
                        .child(action(
                            IconName::ArrowDown,
                            format!("vdown-{}-{i}", handle.id),
                            i + 1 < count,
                            Rc::new(move |e, cx| {
                                let mut next = down_opts.clone();
                                next.swap(i, i + 1);
                                w2(e, next, cx);
                            }),
                            cx,
                        ))
                        .child(action(
                            IconName::Trash,
                            format!("vdel-{}-{i}", handle.id),
                            count > 1,
                            Rc::new(move |e, cx| {
                                let mut next = remove_opts.clone();
                                next.remove(i);
                                w3(e, next, cx);
                            }),
                            cx,
                        ))
                    }),
            );
        }
        if !locked {
            let (options, write) = (options.clone(), write);
            list = list.child(outline_button(
                SharedString::from(format!("add-variant-{}", handle.id)),
                "Add option",
                false,
                cx,
                move |this, _, cx| {
                    let mut next = options.clone();
                    let label = format!("Option {}", next.len() + 1);
                    next.push(EnumVariant::new(next_variant_value(&options), label));
                    write(this, next, cx);
                },
            ));
        }
        column = column.child(list);
    }

    // Compile in
    if handle.can_compile() {
        let (w, r, id, on) = (
            weak.clone(),
            reference.to_owned(),
            handle.id.clone(),
            handle.compiled == Some(true),
        );
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Checkbox::new(SharedString::from(format!("compile-{id}")))
                                .checked(on)
                                .disabled(locked)
                                .on_change(move |checked, _, cx| {
                                    let (r, id, checked) = (r.clone(), id.clone(), *checked);
                                    super::update_editor(&w, cx, |e, cx| {
                                        e.edit(cx, move |m| m.set_handle_compiled(&r, &id, checked))
                                    });
                                }),
                        )
                        .child(label("Compile in", cx)),
                )
                .child(hint(
                    "Lets it pick among textures. Editing it recompiles the shader.",
                    cx,
                )),
        );
    }

    // Range
    if handle.variants.is_none() && handle.display.is_none() && ty.kind() != ScalarKind::Bool {
        let mut range = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label("Range while edited", cx));
        for (is_min, name, current) in [(true, "Min", handle.min), (false, "Max", handle.max)] {
            let apply: RangeAction = {
                let (r, id, min, max) = (
                    reference.to_owned(),
                    handle.id.clone(),
                    handle.min,
                    handle.max,
                );
                Rc::new(move |e, value, cx| {
                    let (r, id) = (r.clone(), id.clone());
                    let (min, max) = if is_min { (value, max) } else { (min, value) };
                    e.edit(cx, move |m| m.set_handle_range(&r, &id, min, max));
                })
            };
            let body = match current {
                None => {
                    let apply = apply.clone();
                    outline_button(
                        SharedString::from(format!("range-{name}-{}", handle.id)),
                        "None",
                        locked,
                        cx,
                        move |this, _, cx| apply(this, Some(if is_min { 0.0 } else { 1.0 }), cx),
                    )
                    .into_any_element()
                }
                Some(value) => {
                    let (w, set, clear) = (weak.clone(), apply.clone(), apply);
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .gap_1()
                        .child(
                            Scrub::new(
                                SharedString::from(format!("range-{name}-{}", handle.id)),
                                value,
                                1.0,
                            )
                            .integer(ty.kind() != ScalarKind::F32)
                            .readonly(locked)
                            .on_change(move |v, _: Edit, _, cx| {
                                let set = set.clone();
                                super::update_editor(&w, cx, |e, cx| set(e, Some(v), cx));
                            }),
                        )
                        .when(!locked, |d| {
                            d.child(button(
                                SharedString::from(format!("range-clear-{name}-{}", handle.id)),
                                Icon::new(IconName::X).size(px(14.0)),
                                cx,
                                move |e, cx| clear(e, None, cx),
                            ))
                        })
                        .into_any_element()
                }
            };
            range = range.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().w(px(32.0)).child(label(name, cx)))
                    .child(div().flex_1().child(body)),
            );
        }
        column = column.child(range);
    }

    // Edit as
    if matches!(ty, WgslType::Vec3f | WgslType::Vec4f) {
        let current = handle.display;
        let name = match current {
            None => "Numbers",
            Some(ColorDisplay::Swatch) => "Colour swatch",
            Some(ColorDisplay::Wheel) => "Colour wheel",
        };
        let items: Vec<ListItem> = [
            ("Numbers", None),
            ("Colour swatch", Some(ColorDisplay::Swatch)),
            ("Colour wheel", Some(ColorDisplay::Wheel)),
        ]
        .into_iter()
        .map(|(text, display)| {
            let (r, id) = (reference.to_owned(), handle.id.clone());
            ListItem::new(
                text,
                Some(current == display),
                on_editor(&weak, move |e, cx| {
                    let (r, id) = (r.clone(), id.clone());
                    e.edit(cx, move |m| m.set_handle_display(&r, &id, display));
                }),
            )
        })
        .collect();
        let w = weak.clone();
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label("Edit as", cx))
                .child(div().flex().child(super::fields::picker(
                    SharedString::from(format!("display-{}", handle.id)),
                    name,
                    1.0,
                    locked,
                    cx,
                    move |at, _, cx| {
                        let items = items.clone();
                        super::update_editor(&w, cx, |e, cx| e.open_list(at, items, cx));
                    },
                ))),
        );
    }
    column
}
