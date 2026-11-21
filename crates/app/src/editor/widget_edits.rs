//! Edits the widgets on node cards make.

use std::collections::BTreeMap;
use std::time::Instant;

use gpui::{AppContext as _, Context, Pixels, Point};
use shader_graph::color::Rgba;
use shader_graph::curve::CurvePoint;
use shader_graph::passes::TextureFormat;
use shader_graph::ramp::RampStop;
use shader_graph::types::{InputValue, WgslType};
use theme::{ColorPicker, ColorPickerEvent};

use super::{CODE_COMMIT_DELAY, Editor, GESTURE_GAP};
use crate::graph::popovers::{ColorPopup, SocketsPopup};
use crate::graph::widgets::{color_binding, gamut_of, node_data_mut, vector_of};

impl Editor {
    /// Runs an edit that is a step of a gesture: steps of the same `key`
    /// close together replace the undo step the first one added.
    pub(crate) fn gesture_edit(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut shader_graph::graph::GraphModel, bool),
    ) {
        if self.model.borrow().is_locked() {
            return;
        }
        let first = !self
            .widget_gesture
            .as_ref()
            .is_some_and(|(k, at)| *k == key && at.elapsed() < GESTURE_GAP);
        f(&mut self.model_mut(), first);
        self.widget_gesture = Some((key, Instant::now()));
        self.after_edit(cx);
    }

    pub(crate) fn set_curve(&mut self, node: &str, points: Vec<CurvePoint>, cx: &mut Context<Self>) {
        let id = node.to_owned();
        self.gesture_edit(format!("curve-{node}"), cx, move |m, first| {
            m.edit_gesture(first, |project| {
                if let Some(data) = node_data_mut(project, &id) {
                    data.curve = Some(points);
                }
            });
        });
    }

    pub(crate) fn set_ramp(
        &mut self,
        node: &str,
        stops: Vec<RampStop>,
        added: BTreeMap<String, Rgba>,
        cx: &mut Context<Self>,
    ) {
        let colors: BTreeMap<String, InputValue> = added.into_iter().map(|(id, c)| (id, vector_of(c))).collect();
        let id = node.to_owned();
        self.gesture_edit(format!("ramp-{node}"), cx, move |m, first| {
            m.set_color_ramp_gesture(first, &id, &stops, &colors);
        });
    }

    /// The colour widget of a group node edited its three components.
    pub(crate) fn set_color_widget(&mut self, node: &str, next: [f64; 3], cx: &mut Context<Self>) {
        let writes: Vec<(String, f64)> = {
            let model = self.model.borrow();
            let Some(binding) = model.node(node).and_then(|n| color_binding(&model, n)) else { return };
            binding
                .inputs
                .into_iter()
                .zip(binding.linked)
                .zip(next)
                .filter(|((_, linked), _)| !linked)
                .map(|((input, _), value)| (input, value))
                .collect()
        };
        let id = node.to_owned();
        self.gesture_edit(format!("color-{node}"), cx, move |m, first| {
            m.edit_gesture(first, |project| {
                if let Some(data) = node_data_mut(project, &id) {
                    for (input, value) in writes {
                        data.values.insert(input, InputValue::from(value));
                    }
                }
            });
        });
    }

    /// A wheel drawn on a colour input edited its colour; its alpha is kept.
    pub(crate) fn set_wheel_input(&mut self, node: &str, input: &str, rgb: [f64; 3], cx: &mut Context<Self>) {
        let alpha = self
            .model
            .borrow()
            .node(node)
            .and_then(|n| n.data.values.get(input).map(|v| v.component(3)))
            .unwrap_or(1.0);
        let (id, handle) = (node.to_owned(), input.to_owned());
        let color = Rgba { r: rgb[0], g: rgb[1], b: rgb[2], a: alpha };
        self.gesture_edit(format!("wheel-{node}-{input}"), cx, move |m, first| {
            m.edit_gesture(first, |project| {
                if let Some(data) = node_data_mut(project, &id) {
                    data.values.insert(handle, vector_of(color));
                }
            });
        });
    }

    pub(crate) fn open_color_picker(
        &mut self,
        at: Point<Pixels>,
        node: &str,
        input: &str,
        ty: WgslType,
        cx: &mut Context<Self>,
    ) {
        let (value, gamut) = {
            let model = self.model.borrow();
            let Some(n) = model.node(node) else { return };
            let default = model.resolve_descriptor(n).default_data.values.get(input).copied();
            (n.data.values.get(input).copied().or(default), gamut_of(model.settings().color_space))
        };
        let alpha = ty == WgslType::Vec4f;
        let component = |i| value.as_ref().map_or(0.0, |v| v.component(i));
        let color = Rgba {
            r: component(0),
            g: component(1),
            b: component(2),
            a: if alpha { value.as_ref().map_or(1.0, |v| v.component(3)) } else { 1.0 },
        };
        let picker = cx.new(|cx| ColorPicker::new(color, cx).alpha(alpha).gamut(gamut));
        let (node, input) = (node.to_owned(), input.to_owned());
        let subscription = cx.subscribe(&picker, move |editor: &mut Editor, _, event: &ColorPickerEvent, cx| {
            let ColorPickerEvent::Change(color) = *event;
            let (node, input) = (node.clone(), input.clone());
            guard::guard("color picker", || {
                let (id, handle) = (node.clone(), input.clone());
                // Edited as RGB, the alpha is kept (opaque if never set) for when the input becomes RGBA.
                let kept = editor
                    .model
                    .borrow()
                    .node(&node)
                    .and_then(|n| n.data.values.get(&input).map(|v| v.component(3)))
                    .unwrap_or(1.0);
                let color = Rgba { a: if alpha { color.a } else { kept }, ..color };
                editor.gesture_edit(format!("picker-{node}-{input}"), cx, move |m, first| {
                    m.edit_gesture(first, |project| {
                        if let Some(data) = node_data_mut(project, &id) {
                            data.values.insert(handle, vector_of(color));
                        }
                    });
                });
            });
        });
        self.color_popup = Some(ColorPopup { at, picker, _subscription: subscription });
        cx.notify();
    }

    pub(crate) fn set_cases(&mut self, node: &str, count: i64, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_index_switch_cases(node, count));
    }

    pub(crate) fn add_render_item(&mut self, node: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            m.add_render_item(node, WgslType::Vec4f, None);
        });
    }

    pub(crate) fn remove_render_item(&mut self, node: &str, item: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.remove_render_item(node, item));
    }

    pub(crate) fn set_render_item_format(&mut self, node: &str, item: &str, format: TextureFormat, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_render_item_format(node, item, format));
    }

    /// Whether the model took the name; a refused one is put back by the field showing it.
    pub(crate) fn rename_render_item(&mut self, node: &str, item: &str, name: &str, cx: &mut Context<Self>) -> bool {
        let accepted = self.model_mut().rename_render_item(node, item, name);
        if accepted {
            self.after_edit(cx);
        }
        accepted
    }

    pub(crate) fn code_typed(&mut self, node: &str, code: String, cx: &mut Context<Self>) {
        self.code_serial += 1;
        let (serial, node) = (self.code_serial, node.to_owned());
        self.code_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CODE_COMMIT_DELAY).await;
            this.update(cx, |editor, cx| {
                guard::guard("commit code", || {
                    if editor.code_serial == serial {
                        editor.code_commit(&node, code, cx);
                    }
                });
            })
            .ok();
        }));
    }

    pub(crate) fn code_commit(&mut self, node: &str, code: String, cx: &mut Context<Self>) {
        self.code_serial += 1;
        self.code_task = None;
        let changed = {
            let model = self.model.borrow();
            !model.is_locked() && model.node(node).is_some_and(|n| n.data.code.as_deref().unwrap_or_default() != code)
        };
        if changed {
            self.edit(cx, |m| m.set_custom_code(node, &code));
        }
    }

    pub(crate) fn open_sockets(&mut self, node: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.sockets = Some(SocketsPopup { node: node.to_owned(), at, rejected: None });
        cx.notify();
    }

    pub(crate) fn add_custom_socket(&mut self, node: &str, input: bool, cx: &mut Context<Self>) {
        self.edit(cx, |m| {
            m.add_custom_socket(node, input, WgslType::F32);
        });
    }

    pub(crate) fn remove_custom_socket(&mut self, node: &str, input: bool, socket: &str, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.remove_custom_socket(node, input, socket));
    }

    pub(crate) fn move_custom_socket(&mut self, node: &str, input: bool, socket: &str, offset: isize, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.move_custom_socket(node, input, socket, offset));
    }

    pub(crate) fn set_custom_socket_type(&mut self, node: &str, input: bool, socket: &str, ty: WgslType, cx: &mut Context<Self>) {
        self.edit(cx, |m| m.set_custom_socket_type(node, input, socket, ty));
    }

    pub(crate) fn rename_custom_socket(&mut self, node: &str, input: bool, socket: &str, label: &str, cx: &mut Context<Self>) {
        let accepted = self.model_mut().rename_custom_socket(node, input, socket, label);
        if let Some(popup) = &mut self.sockets {
            popup.rejected = (!accepted).then(|| socket.to_owned());
        }
        if accepted {
            self.after_edit(cx);
        } else {
            cx.notify();
        }
    }
}
