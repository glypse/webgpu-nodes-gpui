use gpui::{
    Bounds, Hsla, ParentElement as _, Pixels, Point, Styled as _, Window,
    canvas, hsla, point, px, relative,
};
use gpui_flow::{Handle, HandleType, NodeProps, Position};
use shader_graph::types::{ScalarKind, WgslType};
use theme::Theme;

/// Colour follows the component count.
pub fn socket_color(ty: WgslType, theme: &Theme) -> Hsla {
    if ty.is_texture() {
        return hsla(199.0 / 360.0, 0.89, 0.48, 1.0);
    }
    match ty.components() {
        2 => hsla(142.0 / 360.0, 0.71, 0.45, 1.0),
        3 => hsla(258.0 / 360.0, 0.90, 0.66, 1.0),
        4 => hsla(38.0 / 360.0, 0.92, 0.50, 1.0),
        _ => theme.colors.muted_foreground,
    }
}

/// Size in flow units: diamonds and triangles need room to read.
pub fn socket_size(ty: WgslType) -> f32 {
    if ty.is_texture() {
        return 11.0;
    }
    match ty.kind() {
        ScalarKind::F32 | ScalarKind::I32 => 8.0,
        ScalarKind::U32 => 11.0,
        ScalarKind::Bool => 10.0,
    }
}

fn polygon(window: &mut Window, points: &[Point<Pixels>], color: Hsla) {
    let mut b = gpui::PathBuilder::fill();
    b.add_polygon(points, true);
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// A socket's shape: a circle for f32, a square for i32, a diamond for u32
/// and a triangle for bool, outlined in the card's colour.
pub fn paint_socket(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    kind: ScalarKind,
    texture: bool,
    fill: Hsla,
    outline: Hsla,
    unit: f32,
) {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let at = |px_x: f32, px_y: f32| point(px(x + px_x), px(y + px_y));
    let layer = |window: &mut Window, inset: f32, color: Hsla| match kind {
        _ if texture => {
            let radius = if inset == 0.0 { 3.0 } else { 2.0 };
            window.paint_quad(gpui::quad(
                Bounds::new(at(inset, inset), gpui::size(px(w - 2.0 * inset), px(h - 2.0 * inset))),
                px(radius),
                color,
                px(0.0),
                gpui::transparent_black(),
                gpui::BorderStyle::Solid,
            ));
        }
        ScalarKind::F32 | ScalarKind::I32 => {
            let radius = if kind == ScalarKind::F32 { (w / 2.0 - inset).max(0.0) } else { (1.5 * unit - inset * 0.5).max(0.0) };
            window.paint_quad(gpui::quad(
                Bounds::new(at(inset, inset), gpui::size(px(w - 2.0 * inset), px(h - 2.0 * inset))),
                px(radius),
                color,
                px(0.0),
                gpui::transparent_black(),
                gpui::BorderStyle::Solid,
            ));
        }
        ScalarKind::U32 => {
            let i = inset * 1.5;
            polygon(
                window,
                &[at(w / 2.0, i), at(w - i, h / 2.0), at(w / 2.0, h - i), at(i, h / 2.0)],
                color,
            );
        }
        ScalarKind::Bool => {
            let i = inset * 1.5;
            polygon(window, &[at(i, i), at(w - i, h / 2.0), at(i, h - i)], color);
        }
    };
    layer(window, 0.0, outline);
    layer(window, unit.max(0.75), fill);
}

/// A node's handle: positioned by `place`, drawn as the socket of `ty`.
pub struct SocketSpec {
    pub id: Option<String>,
    pub kind: HandleType,
    pub position: Position,
    pub ty: WgslType,
    /// Space between the handle and the row's edge on its side: the card's padding.
    pub inset: f32,
}

pub fn socket(props: &NodeProps, spec: &SocketSpec, theme: &Theme) -> Handle {
    let size = socket_size(spec.ty);
    let z = props.zoom;
    let fill = socket_color(spec.ty, theme);
    let outline = theme.colors.card;
    let hover = theme.colors.foreground;
    let state = props.handle_state(spec.kind, spec.id.as_ref().map(|s| s.clone().into()).as_ref());
    let active = state.connecting_to && state.valid;
    let kind = spec.ty.kind();
    let texture = spec.ty.is_texture();
    let offset = px(-(spec.inset + size / 2.0) * z);
    let half = px(-size / 2.0 * z);
    let handle = props
        .handle(spec.id.clone().map(Into::into), spec.kind, spec.position)
        .sized(size)
        .custom_look()
        .free()
        .absolute()
        .top(relative(0.5))
        .mt(half);
    let handle = match spec.position {
        Position::Right => handle.right(offset),
        _ => handle.left(offset),
    };
    handle.child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| { guard::guard("socket canvas", || {
                let color = if active { hover } else { fill };
                paint_socket(window, bounds, kind, texture, color, outline, z);
            }); },
        )
        .absolute()
        .size_full(),
    )
}
