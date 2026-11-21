use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_flow::{HandleType, NodeProps, Position};
use shader_graph::graph::{
    GROUP_OUTPUT, GraphModel, GraphNode, GroupSide, LAYOUT_FRAME, NODE_GROUP, REROUTE, TypeChoice,
    VariableChoice, frame_note, frame_tint,
};
use shader_graph::nodes::CUSTOM_WGSL;
use shader_graph::types::{
    InputControl, InputValue, ScalarKind, WgslType, enum_value,
    input_control,
};
use shader_graph::zones::{NEW_ITEM_HANDLE, is_zone_node_type};
use shader_graph::color::{ColorSpace as DisplayGamut, Rgb};
use theme::{ActiveTheme as _, ColorWheels, linear_to_hsla};

use super::fields::{Edit, Scrub, picker};
use super::frames::{self, FrameView};
use super::socket::{SocketSpec, socket};
use super::type_picker::TypePicker;
use super::widgets::{self, WidgetView, Widgets};
use crate::editor::{Editor, ListItem};

#[derive(Clone)]
pub struct InputView {
    pub id: String,
    pub label: String,
    pub ty: WgslType,
    pub control: Option<InputControl>,
    pub value: Option<InputValue>,
    pub variadic: bool,
    pub fallback: bool,
    /// The name of the type variable deciding the socket's type.
    pub shared: Option<String>,
}

#[derive(Clone)]
pub struct OutputView {
    pub id: String,
    pub label: String,
    pub ty: WgslType,
    pub shared: Option<String>,
}

#[derive(Clone)]
pub struct SettingView {
    pub key: String,
    pub options: Vec<(String, String)>,
    pub value: String,
}

/// What a node's card shows, taken from the model so the card can be
/// drawn without it.
#[derive(Clone)]
pub struct NodeView {
    pub id: String,
    pub node_type: String,
    pub label: String,
    /// The viewed document can't be edited.
    pub locked: bool,
    pub group_locked: bool,
    pub settings: Vec<SettingView>,
    pub type_choice: Option<TypeChoice>,
    pub inputs: Vec<InputView>,
    pub outputs: Vec<OutputView>,
    /// Inputs of variants not picked: kept linked, undrawn.
    pub parked: Vec<String>,
    pub only_linked: bool,
    pub boundary: Option<GroupSide>,
    pub zone: bool,
    pub reroute_type: Option<WgslType>,
    pub frame: Option<FrameView>,
    /// On a group node: its type variables with more than one type to pick from.
    pub variables: Vec<VariableChoice>,
    /// On a Group Output: the inputs linked on another Group Output node.
    pub elsewhere: HashSet<String>,
    /// A zone node whose partner is gone.
    pub zone_missing: bool,
    pub widget: Option<WidgetView>,
    /// The wheels of the colour inputs drawn as wheels, by input id.
    pub wheels: HashMap<String, Entity<ColorWheels>>,
    pub gamut: DisplayGamut,
}

impl NodeView {
    /// Adds what needs entities kept between renders: the node's widget and its wheels.
    pub fn attach_widgets(
        &mut self,
        widgets: &mut Widgets,
        model: &GraphModel,
        node: &GraphNode,
        gamut: DisplayGamut,
        cx: &mut Context<Editor>,
    ) {
        self.gamut = gamut;
        self.widget = widgets.view(model, node, self.locked, gamut, cx);
        let linked: HashSet<String> =
            model.active_document().edges_into(&node.id).map(|e| e.target_handle.clone()).collect();
        for input in &self.inputs {
            if matches!(input.control, Some(InputControl::Wheel)) && !input.variadic && !linked.contains(&input.id) {
                let component = |i| input.value.as_ref().map_or(0.0, |v| v.component(i));
                let rgb = [component(0), component(1), component(2)];
                let wheel = widgets.input_wheel(&self.id, &input.id, rgb, self.locked, gamut, cx);
                self.wheels.insert(input.id.clone(), wheel);
            }
        }
    }
}

pub fn node_width(node_type: &str) -> f32 {
    match node_type {
        CUSTOM_WGSL => 320.0,
        REROUTE => 18.0,
        _ => 180.0,
    }
}

pub fn view_of(model: &GraphModel, node: &GraphNode) -> NodeView {
    let descriptor = model.resolve_descriptor(node);
    let id = node.id.as_str();
    let doc = model.active_document();
    let variables = &doc.type_variables;
    let shared_of = |handle: &str, boundary: bool| -> Option<String> {
        if !boundary {
            return None;
        }
        let side = node.side()?;
        let def = doc.interface(side).iter().find(|h| h.id == handle)?;
        let variable = def.ty.variable()?;
        variables.iter().find(|v| v.id == variable).map(|v| v.name.clone())
    };
    let boundary = node.side().is_some();
    let hidden = model.hidden_inputs(id);
    let inputs = model
        .resolve_node_inputs(id)
        .into_iter()
        .filter(|i| !hidden.contains(&i.id))
        .map(|def| {
            let ty = model.resolve_input_type(id, &def.id);
            let value = node
                .data
                .values
                .get(&def.id)
                .or_else(|| descriptor.default_data.values.get(&def.id))
                .copied();
            InputView {
                control: def.control.as_ref().map(|_| input_control(&def, ty)).or_else(|| {
                    (!def.variadic && !def.fallback).then(|| input_control(&def, ty))
                }),
                shared: shared_of(&def.id, boundary),
                id: def.id,
                label: def.label,
                ty,
                value,
                variadic: def.variadic,
                fallback: def.fallback,
            }
        })
        .collect();
    let outputs = model
        .resolve_node_outputs(id)
        .into_iter()
        .map(|def| OutputView {
            ty: model.resolve_output_handle_type(id, &def.id),
            shared: shared_of(&def.id, boundary),
            id: def.id,
            label: def.label,
        })
        .collect();
    let settings = descriptor
        .settings
        .iter()
        .map(|s| SettingView {
            key: s.key.to_owned(),
            options: s.options.iter().map(|(v, l)| ((*v).to_owned(), (*l).to_owned())).collect(),
            value: s.value(&node.data).to_owned(),
        })
        .collect();
    NodeView {
        id: node.id.clone(),
        node_type: node.node_type.clone(),
        label: model.node_label(id),
        locked: model.is_locked(),
        group_locked: node.group.as_ref().is_some_and(|g| g.locked),
        settings,
        type_choice: model.type_choice(id),
        inputs,
        outputs,
        parked: hidden.into_iter().collect(),
        only_linked: node.data.hide_unlinked == Some(true),
        boundary: node.side(),
        zone: is_zone_node_type(&node.node_type),
        reroute_type: node.is(REROUTE).then(|| model.resolve_input_type(id, "in")),
        variables: if node.is(NODE_GROUP) {
            model.variable_choices(id).into_iter().filter(|c| c.variable.types.len() > 1).collect()
        } else {
            Vec::new()
        },
        elsewhere: if node.is(GROUP_OUTPUT) {
            doc.nodes
                .iter()
                .filter(|n| n.is(GROUP_OUTPUT) && n.id != node.id)
                .flat_map(|n| doc.edges_into(&n.id).map(|e| e.target_handle.clone()))
                .collect()
        } else {
            HashSet::new()
        },
        zone_missing: is_zone_node_type(&node.node_type)
            && !model.active_zones().zone_of_boundary.contains_key(id),
        widget: None,
        wheels: HashMap::new(),
        gamut: DisplayGamut::Srgb,
        frame: node.is(LAYOUT_FRAME).then(|| FrameView {
            label: model.node_label(id),
            note: frame_note(&node.data).map(str::to_owned),
            tint: frame_tint(&node.data),
        }),
    }
}

const PAD: f32 = 12.0;

type PointAction = Rc<dyn Fn(gpui::Point<gpui::Pixels>, &mut App)>;

/// The menu a node's type picker opens.
fn node_type_picker(id: &str, choice: &TypeChoice) -> TypePicker {
    let (pick_id, reset_id) = (id.to_owned(), id.to_owned());
    TypePicker {
        types: choice.types.clone(),
        value: if choice.inferable { choice.forced } else { choice.forced.or(Some(choice.inferred)) },
        inferred: choice.inferable.then_some(choice.inferred),
        on_pick: Rc::new(move |ty, e, cx| e.set_node_type(&pick_id, Some(ty), cx)),
        on_reset: Some(Rc::new(move |e, cx| e.set_node_type(&reset_id, None, cx))),
        extra: Vec::new(),
    }
}

fn variable_type_picker(id: &str, choice: &VariableChoice) -> TypePicker {
    let (pick_id, reset_id) = (id.to_owned(), id.to_owned());
    let (pick_var, reset_var) = (choice.variable.id.clone(), choice.variable.id.clone());
    TypePicker {
        types: choice.variable.types.clone(),
        value: if choice.inferable { choice.forced } else { choice.forced.or(Some(choice.ty)) },
        inferred: choice.inferable.then_some(choice.inferred),
        on_pick: Rc::new(move |ty, e, cx| {
            if !ty.is_texture() {
                e.set_variable_type(&pick_id, &pick_var, Some(ty), cx);
            }
        }),
        on_reset: Some(Rc::new(move |e, cx| e.set_variable_type(&reset_id, &reset_var, None, cx))),
        extra: Vec::new(),
    }
}

fn value_control(
    props: &NodeProps,
    view: &NodeView,
    input: &InputView,
    ed: &WeakEntity<Editor>,
    cx: &App,
) -> AnyElement {
    let z = props.zoom;
    let readonly = view.locked;
    let Some(control) = &input.control else {
        return div().into_any_element();
    };
    let (node, handle, ty) = (view.id.clone(), input.id.clone(), input.ty);
    match control {
        InputControl::Enum { variants } => {
            let current = enum_value(input.value.as_ref(), variants);
            let label = variants
                .iter()
                .find(|v| Some(v.int()) == current)
                .map_or_else(String::new, |v| v.label.clone());
            let items: Vec<ListItem> = variants
                .iter()
                .map(|v| {
                    let (ed, node, handle, value) = (ed.clone(), node.clone(), handle.clone(), v.int());
                    ListItem::new(
                        v.label.clone(),
                        Some(Some(value) == current),
                        Rc::new(move |_, cx| {
                            ed.update(cx, |e, cx| e.set_enum(&node, &handle, value, cx)).ok();
                        }),
                    )
                })
                .collect();
            let ed = ed.clone();
            picker(
                SharedString::from(format!("enum-{}-{}", view.id, input.id)),
                label,
                z,
                readonly,
                cx,
                move |at, _, cx| {
                    let items = items.clone();
                    ed.update(cx, |e, cx| e.open_list(at, items, cx)).ok();
                },
            )
            .into_any_element()
        }
        InputControl::Toggle { .. } => {
            let n = ty.components();
            div()
                .flex()
                .items_center()
                .gap(px(8.0 * z))
                .children((0..n).map(|i| {
                    let on = input.value.as_ref().is_some_and(|v| v.component(i) != 0.0);
                    let (ed, node, handle) = (ed.clone(), node.clone(), handle.clone());
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0 * z))
                        .child(
                            theme::Checkbox::new(SharedString::from(format!("t-{node}-{handle}-{i}")))
                                .checked(on)
                                .disabled(readonly)
                                .on_change(move |value, _, cx| {
                                    ed.update(cx, |e, cx| {
                                        e.set_component(&node, &handle, ty, i, f64::from(u8::from(value)), Edit::Commit, cx);
                                    })
                                    .ok();
                                }),
                        )
                        .children((n > 1).then(|| label_text(["x", "y", "z", "w"][i].to_owned(), z, cx)))
                }))
                .into_any_element()
        }
        InputControl::Number { min, max, .. } => {
            let n = ty.components();
            let kind = ty.kind();
            let integer = kind != ScalarKind::F32;
            let min = if kind == ScalarKind::U32 { Some(min.unwrap_or(0.0).max(0.0)) } else { *min };
            let max = *max;
            div()
                .flex()
                .flex_col()
                .children((0..n).map(|i| {
                    let value = input.value.as_ref().map_or(0.0, |v| v.component(i));
                    let (ed, node, handle) = (ed.clone(), node.clone(), handle.clone());
                    Scrub::new(SharedString::from(format!("n-{node}-{handle}-{i}")), value, z)
                        .integer(integer)
                        .range(min, max)
                        .readonly(readonly)
                        .on_change(move |v, edit, _, cx| {
                            ed.update(cx, |e, cx| e.set_component(&node, &handle, ty, i, v, edit, cx)).ok();
                        })
                }))
                .into_any_element()
        }
        InputControl::Color => {
            let alpha = ty == WgslType::Vec4f;
            let channel = |i: usize| input.value.as_ref().map_or(0.0, |v| v.component(i));
            let solid = Rgb::new(channel(0), channel(1), channel(2));
            let opaque = linear_to_hsla(solid, view.gamut);
            let translucent = opaque.opacity(if alpha { channel(3) as f32 } else { 1.0 });
            let ed = ed.clone();
            div()
                .id(SharedString::from(format!("color-{node}-{handle}")))
                .flex()
                .h(px(24.0 * z))
                .w_full()
                .overflow_hidden()
                .rounded(px(4.0 * z))
                .border(px(1.0))
                .border_color(cx.theme().colors.border)
                .when(readonly, |d| d.opacity(0.5))
                .when(!readonly, |d| d.cursor_pointer())
                .on_mouse_down(MouseButton::Left, move |e, _, cx| {
                    cx.stop_propagation();
                    if !readonly {
                        let (node, handle, at) = (node.clone(), handle.clone(), e.position);
                        ed.update(cx, |ed, cx| ed.open_color_picker(at, &node, &handle, ty, cx)).ok();
                    }
                })
                .child(div().flex_1().bg(opaque))
                .when(alpha, |d| d.child(div().flex_1().bg(gpui::hsla(0.0, 0.0, 0.5, 1.0)).child(div().size_full().bg(translucent))))
                .into_any_element()
        }
        InputControl::Wheel => {
            let alpha = ty == WgslType::Vec4f;
            let a = input.value.as_ref().map_or(1.0, |v| v.component(3));
            div()
                .flex()
                .flex_col()
                .gap(px(4.0 * z))
                .children(view.wheels.get(&input.id).cloned())
                .children(alpha.then(|| {
                    let (ed, node, handle) = (ed.clone(), node.clone(), handle.clone());
                    Scrub::new(SharedString::from(format!("a-{node}-{handle}")), a, z)
                        .range(Some(0.0), None)
                        .readonly(readonly)
                        .on_change(move |v, edit, _, cx| {
                            ed.update(cx, |e, cx| e.set_component(&node, &handle, ty, 3, v, edit, cx)).ok();
                        })
                }))
                .into_any_element()
        }
    }
}

fn label_text(text: String, z: f32, cx: &App) -> gpui::Div {
    div()
        .text_size(px(12.0 * z))
        .text_color(cx.theme().colors.muted_foreground)
        .whitespace_nowrap()
        .child(text)
}

fn reroute(props: &NodeProps, view: &NodeView, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let ty = view.reroute_type.unwrap_or(WgslType::F32);
    let z = props.zoom;
    let mk = |kind: HandleType, position: Position| {
        let spec = SocketSpec { id: Some(if kind == HandleType::Target { "in" } else { "out" }.into()), kind, position, ty, inset: 0.0 };
        let h = socket(props, &spec, theme);
        h.left(px(0.0)).mt(px(-4.0 * z)).ml(px(5.0 * z))
    };
    div()
        .relative()
        .size(px(18.0 * z))
        .rounded_full()
        .when(props.selected, |d| d.border(px(1.0)).border_color(theme.colors.primary))
        .child(mk(HandleType::Target, Position::Left))
        .child(mk(HandleType::Source, Position::Right))
        .into_any_element()
}

pub fn render_node(
    view: &NodeView,
    props: &NodeProps,
    ed: &WeakEntity<Editor>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if view.node_type == REROUTE {
        return reroute(props, view, cx);
    }
    if let Some(frame) = &view.frame {
        return frames::header(frame, props, ed, cx);
    }
    let z = props.zoom;
    for wheel in view.wheels.values() {
        wheel.update(cx, |w, cx| w.set_size(px(112.0 * z), cx));
    }
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let s = |v: f32| px(v * z);
    let widget = view.widget.as_ref().map(|w| widgets::render(w, &view.id, z, view.locked, ed, cx));
    let linked_in = props.connected_handles(HandleType::Target);
    let linked_out = props.connected_handles(HandleType::Source);
    let is_linked = |set: &HashSet<Option<gpui_flow::Id>>, id: &str| set.contains(&Some(id.to_owned().into()));

    let settings_ed = ed.clone();
    let settings_id = view.id.clone();
    let header = div()
        .flex()
        .flex_col()
        .gap(s(4.0))
        .px(s(PAD))
        .pt(s(PAD))
        .pb(s(8.0))
        .border_b(px(1.0))
        .border_color(c.border)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(s(8.0))
                .child(div().min_w_0().truncate().text_size(s(14.0)).font_weight(gpui::FontWeight::MEDIUM).child(view.label.clone()))
                .children(view.only_linked.then(|| {
                    let (ed, id, locked) = (ed.clone(), view.id.clone(), view.locked);
                    div()
                        .id("show-sockets")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(s(20.0))
                        .rounded(s(4.0))
                        .text_size(s(13.0))
                        .text_color(c.muted_foreground)
                        .when(locked, |d| d.opacity(0.5))
                        .when(!locked, |d| d.cursor_pointer().hover(|st| st.bg(c.muted).text_color(c.foreground)))
                        .tooltip(theme::Tooltip::text("Unlinked sockets hidden: show them (Ctrl+H)"))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            if !locked {
                                ed.update(cx, |e, cx| e.toggle_unlinked_sockets(&id, cx)).ok();
                            }
                        })
                        .child("◌")
                }))
                .child(
                    div()
                        .id("settings")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(s(20.0))
                        .rounded(s(4.0))
                        .text_size(s(13.0))
                        .text_color(c.muted_foreground)
                        .cursor_pointer()
                        .hover(|st| st.bg(c.muted).text_color(c.foreground))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            settings_ed.update(cx, |e, cx| e.open_sheet(&settings_id, cx)).ok();
                        })
                        .child("≡"),
                ),
        )
        .when(view.zone_missing, |d| {
            d.child(div().text_size(s(12.0)).text_color(c.destructive).child("Missing its paired node"))
        })
        .when(view.node_type == NODE_GROUP, |d| {
            let button = |id: &'static str, label: &'static str, on_click: Rc<dyn Fn(&mut App)>| {
                div()
                    .id(id)
                    .flex_1()
                    .h(s(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(s(6.0))
                    .bg(c.secondary)
                    .text_size(s(12.0))
                    .cursor_pointer()
                    .hover(|st| st.opacity(0.85))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        on_click(cx);
                    })
                    .child(label)
            };
            let (enter, clone) = (ed.clone(), ed.clone());
            let (enter_id, clone_id) = (view.id.clone(), view.id.clone());
            d.child(
                div()
                    .flex()
                    .gap(s(4.0))
                    .child(button(
                        "enter",
                        if view.group_locked { "Enter (locked)" } else { "Enter group" },
                        Rc::new(move |cx| {
                            enter.update(cx, |e, cx| e.enter_group(&enter_id, cx)).ok();
                        }),
                    ))
                    .when(view.group_locked && !view.locked, |d| {
                        d.child(button(
                            "clone-group",
                            "Clone & edit",
                            Rc::new(move |cx| {
                                clone.update(cx, |e, cx| e.clone_group(&clone_id, cx)).ok();
                            }),
                        ))
                    }),
            )
        })
        .children(view.type_choice.clone().map(|choice| {
            let label = match (choice.forced, choice.inferable) {
                (Some(t), _) => t.id().to_owned(),
                (None, true) => format!("{} (inferred)", choice.inferred.id()),
                (None, false) => choice.inferred.id().to_owned(),
            };
            let (ed, id) = (ed.clone(), view.id.clone());
            div().flex().child(picker(
                SharedString::from(format!("type-{}", view.id)),
                label,
                z,
                view.locked,
                cx,
                move |at, _, cx| {
                    let pick = node_type_picker(&id, &choice);
                    ed.update(cx, |e, cx| e.open_type_picker(at, pick, cx)).ok();
                },
            ))
        }))
        .children(view.variables.iter().map(|choice| {
            let label = match (choice.forced, choice.inferable) {
                (Some(t), _) => t.id().to_owned(),
                (None, true) => format!("{} (inferred)", choice.inferred.id()),
                (None, false) => choice.ty.id().to_owned(),
            };
            let (ed, id, choice) = (ed.clone(), view.id.clone(), choice.clone());
            div()
                .flex()
                .items_center()
                .gap(s(4.0))
                .child(
                    div()
                        .w(s(20.0))
                        .flex_none()
                        .text_size(s(12.0))
                        .text_color(c.muted_foreground)
                        .child(choice.variable.name.clone()),
                )
                .child(picker(
                    SharedString::from(format!("var-{}-{}", view.id, choice.variable.id)),
                    label,
                    z,
                    view.locked,
                    cx,
                    move |at, _, cx| {
                        let pick = variable_type_picker(&id, &choice);
                        ed.update(cx, |e, cx| e.open_type_picker(at, pick, cx)).ok();
                    },
                ))
        }))
        .children(view.settings.iter().map(|setting| {
            let label = setting
                .options
                .iter()
                .find(|(v, _)| *v == setting.value)
                .map_or_else(|| setting.value.clone(), |(_, l)| l.clone());
            let items: Vec<ListItem> = setting
                .options
                .iter()
                .map(|(value, label)| {
                    let (ed, id, key, value) = (ed.clone(), view.id.clone(), setting.key.clone(), value.clone());
                    ListItem::new(
                        label.clone(),
                        Some(value == setting.value),
                        Rc::new(move |_, cx| {
                            ed.update(cx, |e, cx| e.set_setting(&id, &key, &value, cx)).ok();
                        }),
                    )
                })
                .collect();
            let ed = ed.clone();
            div().flex().child(picker(
                SharedString::from(format!("set-{}-{}", view.id, setting.key)),
                label,
                z,
                view.locked,
                cx,
                move |at, _, cx| {
                    let items = items.clone();
                    ed.update(cx, |e, cx| e.open_list(at, items, cx)).ok();
                },
            ))
        }));

    let row = || div().relative().flex().items_center().gap(s(6.0)).min_h(s(24.0));

    let outputs = view.outputs.iter().filter(|o| !view.only_linked || is_linked(&linked_out, &o.id)).map(|o| {
        let spec = SocketSpec { id: Some(o.id.clone()), kind: HandleType::Source, position: Position::Right, ty: o.ty, inset: PAD };
        let shared = o.shared.as_ref().map(|n| format!(" ({n})")).unwrap_or_default();
        row()
            .justify_end()
            .child(label_text(format!("{}: {}{shared}", o.label, o.ty.id()), z, cx))
            .child(socket(props, &spec, &theme))
    });

    let hidden_handles = view.parked.iter().filter(|id| is_linked(&linked_in, id)).map(|id| {
        props
            .handle(Some(id.clone().into()), HandleType::Target, Position::Left)
            .connectable(false)
            .custom_look()
            .free()
            .absolute()
            .size(px(1.0))
            .opacity(0.0)
    });

    let inputs = view.inputs.iter().filter(|i| !view.only_linked || is_linked(&linked_in, &i.id)).map(|i| {
        let linked = is_linked(&linked_in, &i.id);
        let spec = SocketSpec { id: Some(i.id.clone()), kind: HandleType::Target, position: Position::Left, ty: i.ty, inset: PAD };
        let shared = i.shared.as_ref().map(|n| format!(" ({n})")).unwrap_or_default();
        let body: AnyElement = if i.variadic {
            label_text(format!("{}: {}{shared}", i.label, i.ty.id()), z, cx).into_any_element()
        } else if view.elsewhere.contains(&i.id) {
            label_text(format!("{}: {} (elsewhere)", i.label, i.ty.id()), z, cx).opacity(0.6).into_any_element()
        } else if linked {
            label_text(format!("{}: {}{shared}", i.label, i.ty.id()), z, cx).into_any_element()
        } else if i.fallback {
            label_text(format!("{}: {} (default)", i.label, i.ty.id()), z, cx).into_any_element()
        } else if i.control.is_some() {
            let enum_control = matches!(i.control, Some(InputControl::Enum { .. }));
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(s(2.0))
                .child(label_text(
                    if enum_control { i.label.clone() } else { format!("{}: {}", i.label, i.ty.id()) },
                    z,
                    cx,
                ))
                .child(value_control(props, view, i, ed, cx))
                .into_any_element()
        } else {
            label_text(format!("{}: {}", i.label, i.ty.id()), z, cx).into_any_element()
        };
        row().child(socket(props, &spec, &theme)).child(body)
    });

    let (add_boundary, add_zone) = (
        view.boundary.is_some() && !view.locked && !view.only_linked,
        view.zone && !view.locked && !view.only_linked,
    );
    let add_button = |id: &'static str, label: &'static str, on_click: PointAction| {
        picker(id, label, z, false, cx, move |at, _, cx| on_click(at, cx))
    };

    let virtual_row = |kind: HandleType| {
        let handle = props
            .handle(Some(NEW_ITEM_HANDLE.to_owned().into()), kind, if kind == HandleType::Target { Position::Left } else { Position::Right })
            .sized(8.0)
            .custom_look()
            .free()
            .absolute()
            .top(gpui::relative(0.5))
            .mt(px(-4.0 * z))
            .rounded_full()
            .border(px(1.0))
            .border_color(c.muted_foreground);
        let handle = if kind == HandleType::Target { handle.left(px(-(PAD + 4.0) * z)) } else { handle.right(px(-(PAD + 4.0) * z)) };
        row()
            .when(kind == HandleType::Source, |d| d.justify_end())
            .child(label_text("new item".to_owned(), z, cx).opacity(0.6))
            .child(handle)
    };

    let boundary_add = add_boundary.then(|| {
        let ed = ed.clone();
        let id = view.id.clone();
        add_button(
            "add-handle",
            "+ socket",
            Rc::new(move |at, cx| {
                let (ed, id) = (ed.clone(), id.clone());
                ed.update(cx, |e, cx| e.open_type_list(at, id, true, cx)).ok();
            }),
        )
        .into_any_element()
    });
    let (add_top, add_bottom) = match view.boundary {
        Some(GroupSide::Input) => (boundary_add, None),
        Some(GroupSide::Output) => (None, boundary_add),
        None => (None, None),
    };
    let zone_add = {
        let ed = ed.clone();
        let id = view.id.clone();
        add_button(
            "add-item",
            "+ item",
            Rc::new(move |at, cx| {
                let (ed, id) = (ed.clone(), id.clone());
                ed.update(cx, |e, cx| e.open_type_list(at, id, false, cx)).ok();
            }),
        )
    };

    div()
        .relative()
        .w(px(node_width(&view.node_type) * z))
        .rounded(s(4.0))
        .border(px(1.0))
        .border_color(if props.selected { c.primary } else { c.border })
        .bg(c.card)
        .text_color(c.card_foreground)
        .child(header)
        .child(
            div()
                .relative()
                .flex()
                .flex_col()
                .gap(s(4.0))
                .p(s(PAD))
                .children(outputs)
                .children(widget)
                .children(add_top.map(|b| div().flex().child(b)))
                .when(view.zone && add_zone && !view.outputs.is_empty(), |d| d.child(virtual_row(HandleType::Source)))
                .children(hidden_handles)
                .children(inputs)
                .children(add_bottom.map(|b| div().flex().child(b)))
                .when(add_zone, |d| d.child(virtual_row(HandleType::Target)).child(div().flex().child(zone_add))),
        )
        .into_any_element()
}
