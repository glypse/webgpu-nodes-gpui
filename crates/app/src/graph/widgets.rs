use std::collections::{BTreeMap, BTreeSet, HashMap};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, WeakEntity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, Styled as _, div, prelude::FluentBuilder as _,
    px,
};
use shader_graph::color::{ColorMode, ColorSpace as DisplayGamut, Rgba};
use shader_graph::curve::{CurvePoint, linear_curve};
use shader_graph::graph::{ColorSpace, GraphModel, GraphNode, GraphProject, GroupWidget};
use shader_graph::nodes::{
    RAMP_HUE, RAMP_INTERPOLATION, RAMP_SPACE, color_ramp_stops, index_switch_cases,
};
use shader_graph::ramp::{
    RAMP_HUES, RAMP_INTERPOLATIONS, RAMP_SPACES, RampModes,
};
use shader_graph::types::{
    InputValue, NodeData, NodeWidget, ScalarValue, VectorValue, WgslType, enum_value,
};
use theme::{
    ActiveTheme as _, ColorWheels, ColorWheelsEvent, CurveEditor, CurveEditorEvent, InputEvent,
    RampEditor, RampEditorEvent, Textarea, TextInput,
};

use super::item_widgets;
use crate::editor::Editor;

/// What a linked ramp stop is drawn as: its colour is only known in the shader.
const UNKNOWN_STOP: Rgba = Rgba { r: 0.214, g: 0.214, b: 0.214, a: 1.0 };

/// One row of a Render Texture's items.
#[derive(Clone)]
pub struct ItemRow {
    pub id: String,
    pub name: Entity<TextInput>,
    pub format: String,
}

/// The editor a node card shows below its outputs.
#[derive(Clone)]
pub enum WidgetView {
    Curve(Entity<CurveEditor>),
    Ramp(Entity<RampEditor>),
    Color(Entity<ColorWheels>),
    Cases(u32),
    Items { rows: Vec<ItemRow>, can_remove: bool },
    Code { signature: String, code: Entity<Textarea> },
}

struct ItemName {
    input: Entity<TextInput>,
    /// The stored name last put in the field, to tell edits made elsewhere from typing.
    synced: String,
}

/// The entities behind the widgets of the nodes drawn, kept between syncs
/// so a drag or typing survives the model changing under it.
#[derive(Default)]
pub struct Widgets {
    curves: HashMap<String, Entity<CurveEditor>>,
    ramps: HashMap<String, (DisplayGamut, bool, Entity<RampEditor>)>,
    wheels: HashMap<String, Entity<ColorWheels>>,
    names: HashMap<String, ItemName>,
    code: HashMap<String, (String, Entity<Textarea>)>,
}

pub fn gamut_of(space: ColorSpace) -> DisplayGamut {
    match space {
        ColorSpace::Srgb => DisplayGamut::Srgb,
        ColorSpace::DisplayP3 => DisplayGamut::DisplayP3,
    }
}

/// The data of a node in whichever document of the project holds it.
pub fn node_data_mut<'a>(project: &'a mut GraphProject, node: &str) -> Option<&'a mut NodeData> {
    let key = project.definition_ref_of(node)?.to_owned();
    Some(&mut project.definitions.get_mut(&key)?.node_mut(node)?.data)
}

fn stored<'a>(node: &'a GraphNode, defaults: &'a NodeData, input: &str) -> Option<&'a InputValue> {
    node.data.values.get(input).or_else(|| defaults.values.get(input))
}

fn linked_inputs(model: &GraphModel, node: &str) -> BTreeSet<String> {
    model.active_document().edges_into(node).map(|e| e.target_handle.clone()).collect()
}

fn pick(value: Option<&InputValue>, list: &[(i64, &str)]) -> i64 {
    let value = value.map_or(0, InputValue::as_i64);
    list.iter().map(|(v, _)| *v).find(|v| *v == value).or_else(|| list.first().map(|(v, _)| *v)).unwrap_or(0)
}

/// How a colour widget reads its node: the mode, the inputs holding the
/// components, and which of them are linked.
pub struct ColorBinding {
    pub mode: ColorMode,
    pub inputs: [String; 3],
    pub linked: [bool; 3],
    pub components: [f64; 3],
}

pub fn color_binding(model: &GraphModel, node: &GraphNode) -> Option<ColorBinding> {
    let descriptor = model.resolve_descriptor(node);
    let Some(NodeWidget::Color(GroupWidget::Color { mode, modes })) = &descriptor.widget else {
        return None;
    };
    let variants = match descriptor.inputs.iter().find(|i| i.id == *mode).and_then(|i| i.control.as_ref()) {
        Some(shader_graph::types::InputControl::Enum { variants }) => variants.clone(),
        _ => Vec::new(),
    };
    let current = enum_value(stored(node, &descriptor.default_data, mode), &variants)?;
    let binding = modes.get(&current.to_string())?;
    let linked = linked_inputs(model, &node.id);
    let read = |c: &String| stored(node, &descriptor.default_data, c).map_or(0.0, |v| v.component(0));
    let [x, y, z] = &binding.components;
    Some(ColorBinding {
        mode: ColorMode::parse(&binding.space).unwrap_or(ColorMode::Rgb),
        inputs: binding.components.clone(),
        linked: [linked.contains(x), linked.contains(y), linked.contains(z)],
        components: [read(x), read(y), read(z)],
    })
}

fn rgba_of(value: Option<&InputValue>) -> Rgba {
    let Some(value) = value else { return Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 } };
    Rgba {
        r: value.component(0),
        g: value.component(1),
        b: value.component(2),
        a: match value {
            InputValue::Vector(v) => v.component(3).map_or(1.0, ScalarValue::as_f64),
            InputValue::Scalar(_) => 1.0,
        },
    }
}

pub fn vector_of(color: Rgba) -> InputValue {
    InputValue::Vector(VectorValue {
        x: Some(ScalarValue::Num(color.r)),
        y: Some(ScalarValue::Num(color.g)),
        z: Some(ScalarValue::Num(color.b)),
        w: Some(ScalarValue::Num(color.a)),
    })
}

impl Widgets {
    /// Entities of the nodes no longer in the document are dropped.
    pub fn retain(&mut self, nodes: &BTreeSet<String>) {
        let present = |key: &String| nodes.contains(key.split('/').next().unwrap_or(key));
        self.curves.retain(|k, _| present(k));
        self.ramps.retain(|k, _| present(k));
        self.wheels.retain(|k, _| present(k));
        self.names.retain(|k, _| present(k));
        self.code.retain(|k, _| present(k));
    }

    /// The wheel of a colour input drawn on the node itself.
    pub fn input_wheel(
        &mut self,
        node: &str,
        input: &str,
        rgb: [f64; 3],
        disabled: bool,
        gamut: DisplayGamut,
        cx: &mut Context<Editor>,
    ) -> Entity<ColorWheels> {
        let key = format!("{node}/{input}");
        let entity = self
            .wheels
            .entry(key)
            .or_insert_with(|| {
                let entity = cx.new(|_| ColorWheels::new(ColorMode::Rgb, rgb));
                let (node, input) = (node.to_owned(), input.to_owned());
                cx.subscribe(&entity, move |editor: &mut Editor, _, event: &ColorWheelsEvent, cx| {
                    let ColorWheelsEvent::Change(next) = event;
                    let next = *next;
                    guard::guard("wheel input", || editor.set_wheel_input(&node, &input, next, cx));
                })
                .detach();
                entity
            })
            .clone();
        entity.update(cx, |w, cx| {
            w.set_components(ColorMode::Rgb, rgb, cx);
            w.set_locked([false; 3], disabled, cx);
            w.set_gamut(gamut, cx);
        });
        entity
    }

    /// Puts the stored name back in an item's field after the model refused a new one.
    pub fn reset_name(&self, node: &str, item: &str, cx: &mut Context<Editor>) {
        if let Some(entry) = self.names.get(&format!("{node}/{item}")) {
            let label = entry.synced.clone();
            entry.input.update(cx, |input, cx| input.set_value(label, cx));
        }
    }

    pub fn view(
        &mut self,
        model: &GraphModel,
        node: &GraphNode,
        locked: bool,
        gamut: DisplayGamut,
        cx: &mut Context<Editor>,
    ) -> Option<WidgetView> {
        let descriptor = model.resolve_descriptor(node);
        let id = node.id.as_str();
        match descriptor.widget.as_ref()? {
            NodeWidget::Curve => {
                let points: Vec<CurvePoint> = node
                    .data
                    .curve
                    .clone()
                    .or_else(|| descriptor.default_data.curve.clone())
                    .unwrap_or_else(linear_curve);
                let entity = self
                    .curves
                    .entry(id.to_owned())
                    .or_insert_with(|| {
                        let entity = cx.new(|cx| CurveEditor::new(points.clone(), cx));
                        let node = id.to_owned();
                        cx.subscribe(&entity, move |editor: &mut Editor, _, event: &CurveEditorEvent, cx| {
                            let CurveEditorEvent::Change(points) = event;
                            let points = points.clone();
                            guard::guard("curve widget", || editor.set_curve(&node, points, cx));
                        })
                        .detach();
                        entity
                    })
                    .clone();
                entity.update(cx, |curve, cx| {
                    curve.set_points(points, cx);
                    curve.set_disabled(locked, cx);
                });
                Some(WidgetView::Curve(entity))
            }
            NodeWidget::Ramp => {
                let stops = color_ramp_stops(&node.data);
                let linked = linked_inputs(model, id);
                let defaults = &descriptor.default_data;
                let colors: BTreeMap<String, Rgba> = stops
                    .iter()
                    .map(|s| {
                        let color = if linked.contains(&s.id) {
                            UNKNOWN_STOP
                        } else {
                            rgba_of(stored(node, defaults, &s.id))
                        };
                        (s.id.clone(), color)
                    })
                    .collect();
                let modes = RampModes {
                    interpolation: pick(stored(node, defaults, RAMP_INTERPOLATION), &RAMP_INTERPOLATIONS),
                    space: pick(stored(node, defaults, RAMP_SPACE), &RAMP_SPACES),
                    hue: pick(stored(node, defaults, RAMP_HUE), &RAMP_HUES),
                };
                let alpha = model
                    .resolve_node_outputs(id)
                    .iter()
                    .find(|o| o.id == "color")
                    .is_some_and(|o| o.ty == WgslType::Vec4f);
                let stale = self.ramps.get(id).is_some_and(|(g, a, _)| *g != gamut || *a != alpha);
                if stale {
                    self.ramps.remove(id);
                }
                let (_, _, entity) = self.ramps.entry(id.to_owned()).or_insert_with(|| {
                    let entity = cx.new(|cx| {
                        RampEditor::new(stops.clone(), colors.clone(), modes, cx).alpha(alpha).gamut(gamut)
                    });
                    let node = id.to_owned();
                    cx.subscribe(&entity, move |editor: &mut Editor, _, event: &RampEditorEvent, cx| {
                        let RampEditorEvent::Change { stops, added } = event;
                        let (stops, added) = (stops.clone(), added.clone());
                        guard::guard("ramp widget", || editor.set_ramp(&node, stops, added, cx));
                    })
                    .detach();
                    (gamut, alpha, entity)
                });
                let entity = entity.clone();
                entity.update(cx, |ramp, cx| {
                    ramp.set_ramp(stops, colors, linked, modes, cx);
                    ramp.set_disabled(locked, cx);
                });
                Some(WidgetView::Ramp(entity))
            }
            NodeWidget::Color(_) => {
                let binding = color_binding(model, node)?;
                let entity = self
                    .wheels
                    .entry(id.to_owned())
                    .or_insert_with(|| {
                        let entity = cx.new(|_| ColorWheels::new(binding.mode, binding.components));
                        let node = id.to_owned();
                        cx.subscribe(&entity, move |editor: &mut Editor, _, event: &ColorWheelsEvent, cx| {
                            let ColorWheelsEvent::Change(next) = event;
                            let next = *next;
                            guard::guard("color widget", || editor.set_color_widget(&node, next, cx));
                        })
                        .detach();
                        entity
                    })
                    .clone();
                entity.update(cx, |wheels, cx| {
                    wheels.set_components(binding.mode, binding.components, cx);
                    wheels.set_locked(binding.linked, locked, cx);
                    wheels.set_gamut(gamut, cx);
                });
                Some(WidgetView::Color(entity))
            }
            NodeWidget::Cases => Some(WidgetView::Cases(index_switch_cases(&node.data))),
            NodeWidget::Items => {
                let items = model.render_items_of(id);
                let can_remove = items.len() > 1;
                let rows = items
                    .iter()
                    .map(|item| {
                        let key = format!("{id}/{}", item.id);
                        let entry = self.names.entry(key).or_insert_with(|| {
                            let input = cx.new(|cx| {
                                let mut input = TextInput::new(cx);
                                input.set_compact(Some((px(22.0), px(12.0))), cx);
                                input.set_value(item.label.clone(), cx);
                                input
                            });
                            let (node, item_id) = (id.to_owned(), item.id.clone());
                            cx.subscribe(&input, move |editor: &mut Editor, input, event: &InputEvent, cx| {
                                if matches!(event, InputEvent::Submit | InputEvent::Blur) {
                                    let name = input.read(cx).value().to_string();
                                    guard::guard("item name", || editor.rename_render_item(&node, &item_id, &name, cx));
                                }
                            })
                            .detach();
                            ItemName { input, synced: item.label.clone() }
                        });
                        if entry.synced != item.label {
                            entry.synced = item.label.clone();
                            let label = item.label.clone();
                            entry.input.update(cx, |input, cx| input.set_value(label, cx));
                        }
                        ItemRow {
                            id: item.id.clone(),
                            name: entry.input.clone(),
                            format: item.format.clone().unwrap_or_else(|| "rgba16float".to_owned()),
                        }
                    })
                    .collect();
                Some(WidgetView::Items { rows, can_remove })
            }
            NodeWidget::Asset(_) => None,
            NodeWidget::Code => {
                let sockets = shader_graph::nodes::custom_sockets(&node.data);
                let labels = |list: &[shader_graph::types::HandleDef]| {
                    list.iter().map(|s| s.label.clone()).collect::<Vec<_>>().join(", ")
                };
                let outputs = labels(&sockets.outputs);
                let signature = format!(
                    "({}) → {}",
                    labels(&sockets.inputs),
                    if outputs.is_empty() { "nothing".to_owned() } else { outputs }
                );
                let text = node.data.code.clone().unwrap_or_default();
                let (synced, entity) = self.code.entry(id.to_owned()).or_insert_with(|| {
                    let entity = cx.new(|cx| {
                        let mut area = Textarea::new(cx);
                        area.set_value(text.clone(), cx);
                        area
                    });
                    let node = id.to_owned();
                    cx.subscribe(&entity, move |editor: &mut Editor, area, event: &InputEvent, cx| {
                        let value = area.read(cx).value().to_string();
                        match event {
                            InputEvent::Change => editor.code_typed(&node, value, cx),
                            InputEvent::Blur => editor.code_commit(&node, value, cx),
                            _ => {}
                        }
                    })
                    .detach();
                    (text.clone(), entity)
                });
                if *synced != text {
                    *synced = text.clone();
                    let current = entity.read(cx).value().to_string();
                    if current != text {
                        entity.update(cx, |area, cx| area.set_value(text, cx));
                    }
                }
                Some(WidgetView::Code { signature, code: entity.clone() })
            }
        }
    }
}

/// The card's widget, laid out at the card's zoom.
pub fn render(
    widget: &WidgetView,
    node: &str,
    zoom: f32,
    locked: bool,
    ed: &WeakEntity<Editor>,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let c = &theme.colors;
    let z = zoom;
    let body = match widget {
        WidgetView::Curve(curve) => {
            let side = px(150.0 * z);
            curve.update(cx, |e, cx| e.set_side(side, cx));
            curve.clone().into_any_element()
        }
        WidgetView::Ramp(ramp) => ramp.clone().into_any_element(),
        WidgetView::Color(wheels) => {
            wheels.update(cx, |w, cx| w.set_size(px(112.0 * z), cx));
            wheels.clone().into_any_element()
        }
        WidgetView::Cases(count) => {
            let count = *count;
            let button = |id: &'static str, label: &'static str, enabled: bool, next: i64, node: String, ed: WeakEntity<Editor>| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.0 * z))
                    .rounded(px(4.0 * z))
                    .text_size(px(13.0 * z))
                    .when(!enabled, |d| d.opacity(0.4))
                    .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c.muted)))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        if enabled {
                            ed.update(cx, |e, cx| e.set_cases(&node, next, cx)).ok();
                        }
                    })
                    .child(label)
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.0 * z))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0 * z))
                        .text_color(c.muted_foreground)
                        .child(format!("{count} {}", if count == 1 { "case" } else { "cases" })),
                )
                .child(button("fewer-cases", "−", !locked && count > 1, i64::from(count) - 1, node.to_owned(), ed.clone()))
                .child(button("more-cases", "+", !locked, i64::from(count) + 1, node.to_owned(), ed.clone()))
                .into_any_element()
        }
        WidgetView::Items { rows, can_remove } => {
            let mut list = div().flex().flex_col().gap(px(4.0 * z));
            for row in rows {
                list = list.child(item_widgets::row(row, node, *can_remove && !locked, locked, z, ed, cx));
            }
            list.child(item_widgets::add(node, locked, z, ed, cx)).into_any_element()
        }
        WidgetView::Code { signature, code } => div()
            .flex()
            .flex_col()
            .gap(px(4.0 * z))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0 * z))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0 * z))
                            .text_color(c.muted_foreground)
                            .child(SharedString::from(signature.clone())),
                    )
                    .child(item_widgets::sockets_button(node, locked, z, ed, cx)),
            )
            .child(code.clone())
            .into_any_element(),
    };
    div()
        .px(px(12.0 * z))
        .py(px(4.0 * z))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(body)
        .into_any_element()
}
