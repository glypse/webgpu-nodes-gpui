//! Controls and the minimap, and the corner panels they sit in.

use guard::SafeListener as _;
use std::rc::Rc;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Context, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, canvas, div, point, px, quad, size,
};

use crate::flow::Flow;
use crate::geometry::{Rect, Xy, bounds_of};
use crate::model::Node;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelPosition {
    TopLeft,
    TopCenter,
    TopRight,
    #[default]
    BottomLeft,
    BottomCenter,
    BottomRight,
}

pub type PanelRender = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
pub type ControlAction = Rc<dyn Fn(&mut Window, &mut App)>;
pub type NodeColor = Rc<dyn Fn(&Node) -> Hsla>;

/// Anything the owner wants in a corner of the canvas.
#[derive(Clone)]
pub struct Panel {
    pub position: PanelPosition,
    pub render: PanelRender,
}

#[derive(Clone)]
pub struct ControlButton {
    pub label: SharedString,
    pub on_click: ControlAction,
}

#[derive(Clone)]
pub struct ControlsOptions {
    pub position: PanelPosition,
    pub show_zoom: bool,
    pub show_fit_view: bool,
    pub show_interactive: bool,
    pub extra: Vec<ControlButton>,
}

impl Default for ControlsOptions {
    fn default() -> Self {
        Self {
            position: PanelPosition::BottomLeft,
            show_zoom: true,
            show_fit_view: true,
            show_interactive: true,
            extra: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct MiniMapOptions {
    pub position: PanelPosition,
    pub width: f32,
    pub height: f32,
    pub pannable: bool,
    pub node_color: Option<NodeColor>,
}

impl Default for MiniMapOptions {
    fn default() -> Self {
        Self {
            position: PanelPosition::BottomRight,
            width: 200.0,
            height: 150.0,
            pannable: true,
            node_color: None,
        }
    }
}

const PANEL_MARGIN: f32 = 15.0;
const MINIMAP_OFFSET_SCALE: f32 = 5.0;

/// The flow rectangle the minimap shows, and how many flow units a minimap pixel is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MiniMapView {
    pub view_box: Rect,
    pub scale: f32,
}

pub(crate) fn minimap_view(nodes: Option<Rect>, viewport: Rect, width: f32, height: f32) -> MiniMapView {
    let bounding = nodes.map_or(viewport, |n| n.union(&viewport));
    let scaled_w = bounding.width / width;
    let scaled_h = bounding.height / height;
    let scale = scaled_w.max(scaled_h);
    let view_w = scale * width;
    let view_h = scale * height;
    let offset = MINIMAP_OFFSET_SCALE * scale;
    MiniMapView {
        view_box: Rect::new(
            bounding.x - (view_w - bounding.width) / 2.0 - offset,
            bounding.y - (view_h - bounding.height) / 2.0 - offset,
            view_w + offset * 2.0,
            view_h + offset * 2.0,
        ),
        scale,
    }
}

fn anchor(position: PanelPosition) -> gpui::Div {
    let d = div().absolute().occlude();
    let m = px(PANEL_MARGIN);
    match position {
        PanelPosition::TopLeft => d.top(m).left(m),
        PanelPosition::TopCenter => d.top(m).left(gpui::relative(0.5)),
        PanelPosition::TopRight => d.top(m).right(m),
        PanelPosition::BottomLeft => d.bottom(m).left(m),
        PanelPosition::BottomCenter => d.bottom(m).left(gpui::relative(0.5)),
        PanelPosition::BottomRight => d.bottom(m).right(m),
    }
}

impl Flow {
    pub(crate) fn minimap_view(&self) -> Option<MiniMapView> {
        let opts = self.options.minimap.as_ref()?;
        let (w, h) = self.size();
        let nodes = bounds_of(
            self.nodes
                .iter()
                .filter(|n| !n.hidden)
                .filter_map(|n| self.lookup.get(&n.id).filter(|i| i.size.is_some()).map(|i| i.rect())),
        );
        Some(minimap_view(nodes, self.viewport().visible_rect(w, h), opts.width, opts.height))
    }

    pub(crate) fn render_panels(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for panel in &self.panels {
            out.push(anchor(panel.position).child((panel.render)(window, cx)).into_any_element());
        }
        if let Some(controls) = self.render_controls(cx) {
            out.push(controls);
        }
        if let Some(minimap) = self.render_minimap(cx) {
            out.push(minimap);
        }
        out
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let opts = self.options.controls.clone()?;
        let theme = self.options.theme;
        let button = |id: &'static str, child: AnyElement| {
            div()
                .id(id)
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.panel_background)
                .text_color(theme.panel_text)
                .border_b(px(1.0))
                .border_color(theme.panel_border)
                .cursor_pointer()
                .hover(move |s| s.bg(theme.panel_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(child)
        };
        let bar = move |w: f32, h: f32| div().w(px(w)).h(px(h)).bg(theme.panel_text);
        let mut column = div().flex().flex_col().rounded(px(2.0)).overflow_hidden().border(px(1.0)).border_color(theme.panel_border);
        if opts.show_zoom {
            column = column
                .child(
                    button(
                        "flow-zoom-in",
                        div().relative().size(px(12.0)).child(bar(12.0, 2.0).absolute().top(px(5.0))).child(bar(2.0, 12.0).absolute().left(px(5.0))).into_any_element(),
                    )
                    .on_click(cx.safe_listener(|this, _, _, cx| this.zoom_in(cx))),
                )
                .child(
                    button("flow-zoom-out", div().w(px(12.0)).h(px(2.0)).bg(theme.panel_text).into_any_element())
                        .on_click(cx.safe_listener(|this, _, _, cx| this.zoom_out(cx))),
                );
        }
        if opts.show_fit_view {
            let fit = div().size(px(12.0)).border(px(2.0)).border_color(theme.panel_text).rounded(px(1.0));
            column = column.child(button("flow-fit", fit.into_any_element()).on_click(cx.safe_listener(|this, _, _, cx| {
                let options = crate::flow::FitViewOptions {
                    duration: Some(std::time::Duration::from_millis(300)),
                    ..this.options.fit_view_options
                };
                this.fit_view(options, cx);
            })));
        }
        if opts.show_interactive {
            let locked = !self.is_interactive();
            let lock = div()
                .relative()
                .size(px(12.0))
                .child(div().absolute().left(px(2.0)).top(px(0.0)).w(px(8.0)).h(px(6.0)).border(px(2.0)).border_color(theme.panel_text).rounded_t(px(4.0)))
                .child(div().absolute().left(px(0.0)).top(px(5.0)).w(px(12.0)).h(px(7.0)).bg(if locked { theme.panel_text } else { theme.panel_background }).border(px(2.0)).border_color(theme.panel_text).rounded(px(1.0)));
            column = column.child(button("flow-lock", lock.into_any_element()).on_click(cx.safe_listener(|this, _, _, cx| {
                let next = !this.is_interactive();
                this.set_interactive(next, cx);
            })));
        }
        for (i, extra) in opts.extra.iter().enumerate() {
            let on_click = extra.on_click.clone();
            column = column.child(
                button("flow-extra", div().text_size(px(11.0)).child(extra.label.clone()).into_any_element())
                    .id(("flow-extra", i))
                    .on_click(move |_, window, cx| on_click(window, cx)),
            );
        }
        Some(anchor(opts.position).child(column).into_any_element())
    }

    fn render_minimap(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let opts = self.options.minimap.clone()?;
        let view = self.minimap_view()?;
        let theme = self.options.theme;
        let (w, h) = self.size();
        let visible = self.viewport().visible_rect(w, h);
        let nodes: Vec<(Rect, Hsla)> = self
            .nodes
            .iter()
            .filter(|n| !n.hidden)
            .filter_map(|n| {
                let rect = self.lookup.get(&n.id).filter(|i| i.size.is_some())?.rect();
                let color = match &opts.node_color {
                    Some(f) => f(n),
                    None if n.selected => theme.minimap_node_selected,
                    None => theme.minimap_node,
                };
                Some((rect, color))
            })
            .collect();
        let (mw, mh) = (opts.width, opts.height);
        let paint = canvas(
            |_, _, _| {},
            move |bounds: Bounds<gpui::Pixels>, _, window, _| { guard::guard("panels canvas", || {
                let to_px = |p: Xy| {
                    point(
                        bounds.origin.x + px((p.x - view.view_box.x) / view.scale),
                        bounds.origin.y + px((p.y - view.view_box.y) / view.scale),
                    )
                };
                let fill = |window: &mut Window, r: Rect, color: Hsla| {
                    let origin = to_px(Xy::new(r.x, r.y));
                    window.paint_quad(quad(
                        Bounds::new(origin, size(px(r.width / view.scale), px(r.height / view.scale))),
                        px(2.0),
                        color,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::Solid,
                    ));
                };
                for (rect, color) in &nodes {
                    fill(window, *rect, *color);
                }
                // The mask covers everything but the visible area.
                let vb = view.view_box;
                let v = visible;
                let top = Rect::new(vb.x, vb.y, vb.width, (v.y - vb.y).max(0.0));
                let bottom = Rect::new(vb.x, v.bottom(), vb.width, (vb.bottom() - v.bottom()).max(0.0));
                let left = Rect::new(vb.x, v.y, (v.x - vb.x).max(0.0), v.height);
                let right = Rect::new(v.right(), v.y, (vb.right() - v.right()).max(0.0), v.height);
                for r in [top, bottom, left, right] {
                    fill(window, r, theme.minimap_mask);
                }
            }); },
        )
        .absolute()
        .size_full();

        let mut root = div()
            .id("flow-minimap")
            .relative()
            .w(px(mw))
            .h(px(mh))
            .overflow_hidden()
            .rounded(px(2.0))
            .border(px(1.0))
            .border_color(theme.panel_border)
            .bg(theme.minimap_background)
            .child(paint);
        if opts.pannable {
            root = root.on_mouse_down(
                MouseButton::Left,
                cx.safe_listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.begin_minimap_drag(this.local_xy(e.position));
                }),
            );
        }
        Some(anchor(opts.position).child(root).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimap_covers_nodes_and_view() {
        let nodes = Rect::new(0.0, 0.0, 400.0, 100.0);
        let view = Rect::new(-100.0, -50.0, 600.0, 400.0);
        let mm = minimap_view(Some(nodes), view, 200.0, 150.0);
        assert!(mm.view_box.x <= view.x && mm.view_box.right() >= view.right());
        assert!(mm.view_box.y <= view.y && mm.view_box.bottom() >= view.bottom());
        // The box has the panel's aspect ratio.
        assert!((mm.view_box.width / mm.view_box.height - 200.0 / 150.0).abs() < 0.2);
    }

    #[test]
    fn minimap_without_nodes_shows_the_view() {
        let view = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mm = minimap_view(None, view, 200.0, 150.0);
        assert!(mm.view_box.contains(view.center()));
        assert!(mm.scale > 0.0);
    }
}
