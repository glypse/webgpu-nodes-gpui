use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Image, ImageSource, ObjectFit, Subscription,
    WeakEntity, Window, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StyledImage as _, Styled as _, div, img, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{InputEvent, InputState, Textarea, TextareaState};
use shader_graph::color::{ColorMode, ColorSpace as DisplayGamut, Rgba};
use shader_graph::curve::{CurvePoint, linear_curve};
use shader_graph::assets::{Asset, AssetKind, DEFAULT_PLAYBACK, VideoEnd, VideoPlayback};
use shader_graph::graph::{ColorSpace, GraphModel, GraphNode, GraphProject, GroupWidget, PlaybackChange};
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
    ActiveTheme as _, ColorWheels, ColorWheelsEvent, CurveEditor, CurveEditorEvent, Icon, IconName,
    RampEditor, RampEditorEvent,
};

use super::fields::{Edit, Scrub, Stack, picker};
use super::item_widgets;
use super::sheet::new_text_area;
use crate::assets::image_format;
use crate::editor::{Editor, ListItem, OnSelect};
use crate::project::AssetFiles;

/// What a linked ramp stop is drawn as: its colour is only known in the shader.
const UNKNOWN_STOP: Rgba = Rgba { r: 0.214, g: 0.214, b: 0.214, a: 1.0 };

/// One row of a Render Texture's items.
#[derive(Clone)]
pub struct ItemRow {
    pub id: String,
    pub label: String,
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
    Code { signature: String, text: String },
    Asset(AssetView),
}

/// An Image or Video node's asset: what it reads, its thumbnail and the choices its picker offers.
#[derive(Clone)]
pub struct AssetView {
    pub kind: AssetKind,
    /// The asset read, by id and name; `None` when it reads none (or one the project lacks).
    pub current: Option<(String, String)>,
    /// The asset is read but its file isn't in the project.
    pub missing: bool,
    pub thumbnail: Option<Arc<Image>>,
    /// The project's assets of this kind, by id and name.
    pub choices: Vec<(String, String)>,
    pub playback: VideoPlayback,
}

/// The name field of a Render Texture item. Kept across renders by the item's key, since
/// the window it needs only exists while drawing.
pub struct ItemNameField {
    pub(super) input: Entity<InputState>,
    /// The stored name last put in the field, to tell edits made elsewhere from typing.
    synced: String,
    _subscription: Subscription,
}

impl ItemNameField {
    pub(super) fn new(
        node: &str,
        item: &str,
        label: &str,
        ed: WeakEntity<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(label.to_owned(), window, cx);
            input
        });
        let (node, item) = (node.to_owned(), item.to_owned());
        let subscription = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let name = input.read(cx).value().to_string();
            let accepted = guard::guard("item name", || {
                ed.update(cx, |editor, cx| editor.rename_render_item(&node, &item, &name, cx))
            })
            .and_then(Result::ok)
            .unwrap_or(true);
            if !accepted {
                let stored = this.synced.clone();
                input.update(cx, |input, cx| input.set_value(stored, window, cx));
            }
        });
        Self { input, synced: label.to_owned(), _subscription: subscription }
    }

    /// Puts a name the model changed into the field; typing that hasn't been submitted is kept.
    pub(super) fn sync(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.synced != label {
            self.synced = label.to_owned();
            let label = label.to_owned();
            self.input.update(cx, |input, cx| input.set_value(label, window, cx));
        }
    }
}

/// The Custom WGSL code of a node, kept across renders by the node's key.
pub struct CodeField {
    pub(super) area: Entity<TextareaState>,
    /// The stored code last put in the field, to tell edits made elsewhere from typing.
    synced: String,
    _subscription: Subscription,
}

impl CodeField {
    pub(super) fn new(node: &str, text: &str, ed: WeakEntity<Editor>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let area = cx.new(|cx| new_text_area("", text, window, cx));
        let node = node.to_owned();
        let subscription = cx.subscribe_in(&area, window, move |_, area, event: &InputEvent, _, cx| {
            if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                return;
            }
            let value = area.read(cx).value().to_string();
            let is_change = matches!(event, InputEvent::Change);
            guard::guard("code editor", || {
                ed.update(cx, |editor, cx| {
                    if is_change {
                        editor.code_typed(&node, value, cx);
                    } else {
                        editor.code_commit(&node, value, cx);
                    }
                })
            });
        });
        Self { area, synced: text.to_owned(), _subscription: subscription }
    }

    pub(super) fn sync(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.synced != text {
            self.synced = text.to_owned();
            if self.area.read(cx).value().as_ref() != text {
                let text = text.to_owned();
                self.area.update(cx, |area, cx| area.set_value(text, window, cx));
            }
        }
    }
}

/// The entities behind the widgets of the nodes drawn, kept between syncs
/// so a drag or typing survives the model changing under it.
#[derive(Default)]
pub struct Widgets {
    curves: HashMap<String, Entity<CurveEditor>>,
    ramps: HashMap<String, (DisplayGamut, bool, Entity<RampEditor>)>,
    wheels: HashMap<String, Entity<ColorWheels>>,
    /// Decoded thumbnails by asset id, with the file bytes they were decoded from.
    thumbnails: HashMap<String, (Arc<Vec<u8>>, Arc<Image>)>,
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

    /// The thumbnail of an image asset, decoded once per file.
    fn thumbnail(&mut self, asset: &Asset, bytes: &Arc<Vec<u8>>) -> Option<Arc<Image>> {
        if let Some((known, image)) = self.thumbnails.get(asset.id())
            && Arc::ptr_eq(known, bytes)
        {
            return Some(image.clone());
        }
        let format = image_format(&asset.file_info().mime)?;
        let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
        self.thumbnails.insert(asset.id().to_owned(), (bytes.clone(), image.clone()));
        Some(image)
    }

    pub fn view(
        &mut self,
        model: &GraphModel,
        node: &GraphNode,
        locked: bool,
        gamut: DisplayGamut,
        files: &AssetFiles,
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
                    .map(|item| ItemRow {
                        id: item.id.clone(),
                        label: item.label.clone(),
                        format: item.format.clone().unwrap_or_else(|| "rgba16float".to_owned()),
                    })
                    .collect();
                Some(WidgetView::Items { rows, can_remove })
            }
            NodeWidget::Asset(kind) => {
                let kind = *kind;
                let asset = node.data.asset.as_deref().and_then(|a| model.asset(a));
                let current = asset.map(|a| (a.id().to_owned(), a.name().to_owned()));
                let thumbnail = match (asset, node.data.asset.as_deref().and_then(|a| files.get(a))) {
                    (Some(asset), Some(bytes)) => self.thumbnail(asset, bytes),
                    _ => None,
                };
                Some(WidgetView::Asset(AssetView {
                    kind,
                    current,
                    missing: model.missing_asset(id, |a| files.contains_key(a)),
                    thumbnail,
                    choices: model
                        .assets()
                        .iter()
                        .filter(|a| a.kind() == kind)
                        .map(|a| (a.id().to_owned(), a.name().to_owned()))
                        .collect(),
                    playback: node.data.playback.unwrap_or(DEFAULT_PLAYBACK),
                }))
            }
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
                Some(WidgetView::Code { signature, text })
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
    window: &mut Window,
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
            let button = |id: &'static str, label: IconName, enabled: bool, next: i64, node: String, ed: WeakEntity<Editor>| {
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
                    .child(Icon::new(label).size(px(12.0 * z)))
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
                .child(button("fewer-cases", IconName::Minus, !locked && count > 1, i64::from(count) - 1, node.to_owned(), ed.clone()))
                .child(button("more-cases", IconName::Plus, !locked, i64::from(count) + 1, node.to_owned(), ed.clone()))
                .into_any_element()
        }
        WidgetView::Items { rows, can_remove } => {
            let mut list = div().flex().flex_col().gap(px(4.0 * z));
            for row in rows {
                list = list.child(item_widgets::row(row, node, *can_remove && !locked, locked, z, ed, window, cx));
            }
            list.child(item_widgets::add(node, locked, z, ed, cx)).into_any_element()
        }
        WidgetView::Asset(asset) => asset_body(asset, node, locked, z, ed, cx),
        WidgetView::Code { signature, text } => {
            let field = window.use_keyed_state(
                SharedString::from(format!("code-{node}")),
                cx,
                |window, cx| CodeField::new(node, text, ed.clone(), window, cx),
            );
            field.update(cx, |field, cx| field.sync(text, window, cx));
            let area = field.read(cx).area.clone();
            div()
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
                .child(Textarea::new(&area))
                .into_any_element()
        }
    };
    div()
        .px(px(12.0 * z))
        .py(px(4.0 * z))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(body)
        .into_any_element()
}

fn end_label(end: VideoEnd) -> &'static str {
    match end {
        VideoEnd::Loop => "Loop",
        VideoEnd::Hold => "Hold last frame",
        VideoEnd::Transparent => "Transparent",
    }
}

const VIDEO_ENDS: [VideoEnd; 3] = [VideoEnd::Loop, VideoEnd::Hold, VideoEnd::Transparent];

/// A labelled row of a Video node's fields.
fn field_row(label: &'static str, child: impl IntoElement, z: f32, muted: gpui::Hsla) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0 * z))
        .child(div().w(px(44.0 * z)).text_size(px(12.0 * z)).text_color(muted).child(label))
        .child(div().flex_1().min_w_0().child(child))
}

/// The body of an Image or Video node: the preview of its asset, the asset picker and,
/// for a video, its playback fields.
fn asset_body(view: &AssetView, node: &str, locked: bool, z: f32, ed: &WeakEntity<Editor>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (muted, muted_fg) = (c.muted, c.muted_foreground);
    let icon = match view.kind {
        AssetKind::Image => IconName::Image,
        AssetKind::Video => IconName::FilmStrip,
    };
    let preview: AnyElement = match (&view.current, &view.thumbnail, view.missing) {
        (Some(_), _, true) => div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.0 * z))
            .text_size(px(12.0 * z))
            .child(Icon::new(IconName::ImageBroken).size(px(24.0 * z)))
            .child("File missing")
            .into_any_element(),
        (_, Some(image), false) => img(ImageSource::Image(image.clone()))
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        (None, _, _) => Icon::new(IconName::Image).size(px(24.0 * z)).into_any_element(),
        (Some(_), None, false) => Icon::new(icon).size(px(24.0 * z)).into_any_element(),
    };
    let preview = div()
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .aspect_ratio(16.0 / 9.0)
        .overflow_hidden()
        .rounded(px(4.0 * z))
        .bg(muted)
        .text_color(muted_fg)
        .child(preview);

    let current_id = view.current.as_ref().map(|(id, _)| id.clone());
    let label = view.current.as_ref().map_or_else(|| "None".to_owned(), |(_, name)| name.clone());
    let mut choices = vec![ListItem::new(
        "None",
        Some(current_id.is_none()),
        asset_choice(ed, node, None),
    )];
    choices.extend(view.choices.iter().map(|(id, name)| {
        ListItem::new(
            name.clone(),
            Some(current_id.as_deref() == Some(id.as_str())),
            asset_choice(ed, node, Some(id.clone())),
        )
    }));
    choices.push(ListItem::new("Import…", None, import_choice(ed, node)));
    let asset_picker = picker(
        SharedString::from(format!("asset-{node}")),
        label,
        z,
        locked,
        cx,
        {
            let ed = ed.clone();
            move |at, _, cx| {
                let items = choices.clone();
                ed.update(cx, |e, cx| e.open_list(at, items, cx)).ok();
            }
        },
    );

    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(4.0 * z))
        .child(preview)
        .child(asset_picker.into_any_element());
    if view.kind == AssetKind::Video {
        let playback = view.playback;
        let video_field = |field: &'static str, value: f64, change: fn(f64) -> PlaybackChange| {
            let (ed, node) = (ed.clone(), node.to_owned());
            Scrub::new(SharedString::from(format!("video-{field}-{node}")), value, z)
                .stack(Stack::Single)
                .readonly(locked)
                .on_change(move |v, edit, _, cx| {
                    ed.update(cx, |e, cx| e.set_video_playback(&node, change(v), edit, cx)).ok();
                })
        };
        let end_items: Vec<ListItem> = VIDEO_ENDS
            .iter()
            .map(|&end| {
                let (ed, node) = (ed.clone(), node.to_owned());
                ListItem::new(
                    end_label(end),
                    Some(end == playback.end),
                    Rc::new(move |_, cx| {
                        ed.update(cx, |e, cx| {
                            e.set_video_playback(&node, PlaybackChange { end: Some(end), ..Default::default() }, Edit::Commit, cx)
                        })
                        .ok();
                    }),
                )
            })
            .collect();
        let end_ed = ed.clone();
        let end_picker = picker(
            SharedString::from(format!("video-end-{node}")),
            end_label(playback.end),
            z,
            locked,
            cx,
            move |at, _, cx| {
                let items = end_items.clone();
                end_ed.update(cx, |e, cx| e.open_list(at, items, cx)).ok();
            },
        );
        body = body
            .child(field_row(
                "Start",
                video_field("start", playback.start, |v| PlaybackChange { start: Some(v), ..Default::default() }),
                z,
                muted_fg,
            ))
            .child(field_row(
                "Speed",
                video_field("speed", playback.speed, |v| PlaybackChange { speed: Some(v), ..Default::default() }),
                z,
                muted_fg,
            ))
            .child(field_row("After end", end_picker.into_any_element(), z, muted_fg));
    }
    div()
        .px(px(12.0 * z))
        .py(px(4.0 * z))
        .child(body)
        .into_any_element()
}

/// The handler of the picker's Import entry: asks for a file and has the node read it.
fn import_choice(ed: &WeakEntity<Editor>, node: &str) -> OnSelect {
    let (ed, node) = (ed.clone(), node.to_owned());
    Rc::new(move |window, cx| {
        ed.update(cx, |e, cx| e.import_for_node(&node, window, cx)).ok();
    })
}

/// The handler of an asset picker entry: `None` reads no asset.
fn asset_choice(ed: &WeakEntity<Editor>, node: &str, asset: Option<String>) -> OnSelect {
    let (ed, node) = (ed.clone(), node.to_owned());
    Rc::new(move |_, cx| {
        ed.update(cx, |e, cx| e.set_node_asset(&node, asset.as_deref(), cx)).ok();
    })
}
