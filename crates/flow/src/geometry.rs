//! Pure coordinate maths: viewport transforms, bounds, snapping, fitting.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Xy {
    pub x: f32,
    pub y: f32,
}

impl Xy {
    pub const ZERO: Xy = Xy { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Xy) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

impl std::ops::Add for Xy {
    type Output = Xy;
    fn add(self, o: Xy) -> Xy {
        Xy::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Xy {
    type Output = Xy;
    fn sub(self, o: Xy) -> Xy {
        Xy::new(self.x - o.x, self.y - o.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    pub fn from_corners(a: Xy, b: Xy) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn center(&self) -> Xy {
        Xy::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, p: Xy) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }

    pub fn overlap_area(&self, o: &Rect) -> f32 {
        let x = (self.right().min(o.right()) - self.x.max(o.x)).max(0.0);
        let y = (self.bottom().min(o.bottom()) - self.y.max(o.y)).max(0.0);
        x * y
    }

    pub fn area(&self) -> f32 {
        self.width * self.height
    }

    pub fn inflate(&self, by: f32) -> Rect {
        Rect::new(self.x - by, self.y - by, self.width + 2.0 * by, self.height + 2.0 * by)
    }
}

/// The union of rects; `None` when there are none.
pub fn bounds_of(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    rects.into_iter().reduce(|a, b| a.union(&b))
}

/// Camera: `screen = flow * zoom + (x, y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

impl Viewport {
    pub fn to_screen(&self, p: Xy) -> Xy {
        Xy::new(p.x * self.zoom + self.x, p.y * self.zoom + self.y)
    }

    pub fn to_flow(&self, p: Xy) -> Xy {
        Xy::new((p.x - self.x) / self.zoom, (p.y - self.y) / self.zoom)
    }

    pub fn to_flow_snapped(&self, p: Xy, grid: Option<(f32, f32)>) -> Xy {
        let flow = self.to_flow(p);
        grid.map_or(flow, |g| snap_position(flow, g))
    }

    /// The flow-space rectangle visible in a canvas of the given size.
    pub fn visible_rect(&self, width: f32, height: f32) -> Rect {
        Rect::new(
            -self.x / self.zoom,
            -self.y / self.zoom,
            width / self.zoom,
            height / self.zoom,
        )
    }

    /// Zooms by `factor`, keeping the flow point under screen position `at` still.
    pub fn zoomed_at(&self, factor: f32, at: Xy, min: f32, max: f32) -> Viewport {
        let zoom = clamp(self.zoom * factor, min, max);
        let flow = self.to_flow(at);
        Viewport { x: at.x - flow.x * zoom, y: at.y - flow.y * zoom, zoom }
    }

    pub fn lerp(&self, to: &Viewport, t: f32) -> Viewport {
        Viewport {
            x: self.x + (to.x - self.x) * t,
            y: self.y + (to.y - self.y) * t,
            zoom: self.zoom * (to.zoom / self.zoom).powf(t),
        }
    }
}

pub fn clamp(v: f32, min: f32, max: f32) -> f32 {
    v.max(min).min(max)
}

pub fn snap_position(p: Xy, grid: (f32, f32)) -> Xy {
    Xy::new(
        grid.0 * (p.x / grid.0).round(),
        grid.1 * (p.y / grid.1).round(),
    )
}

/// `[min, max]` corners; unbounded by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub min: Xy,
    pub max: Xy,
}

impl Extent {
    pub const INFINITE: Extent = Extent {
        min: Xy::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
        max: Xy::new(f32::INFINITY, f32::INFINITY),
    };

    pub fn new(min: Xy, max: Xy) -> Self {
        Self { min, max }
    }
}

/// Keeps a node of the given size inside the extent.
pub fn clamp_position(p: Xy, extent: &Extent, width: f32, height: f32) -> Xy {
    Xy::new(
        clamp(p.x, extent.min.x, extent.max.x - width),
        clamp(p.y, extent.min.y, extent.max.y - height),
    )
}

/// Clamps a viewport so the translate extent keeps covering the canvas,
/// as d3-zoom's `constrain` does.
pub fn constrain_viewport(vp: Viewport, width: f32, height: f32, extent: &Extent) -> Viewport {
    let left = vp.to_flow(Xy::ZERO);
    let right = vp.to_flow(Xy::new(width, height));
    let shift = |d0: f32, d1: f32| {
        if d1 > d0 {
            (d0 + d1) / 2.0
        } else if d0.min(0.0) != 0.0 {
            d0.min(0.0)
        } else {
            d1.max(0.0)
        }
    };
    let tx = shift(left.x - extent.min.x, right.x - extent.max.x);
    let ty = shift(left.y - extent.min.y, right.y - extent.max.y);
    Viewport { x: vp.x + vp.zoom * tx, y: vp.y + vp.zoom * ty, zoom: vp.zoom }
}

/// The viewport that centres `bounds` in a canvas with the given padding
/// (a unitless fraction of the canvas).
pub fn viewport_for_bounds(
    bounds: Rect,
    width: f32,
    height: f32,
    min_zoom: f32,
    max_zoom: f32,
    padding: f32,
) -> Viewport {
    let pad = |size: f32| ((size - size / (1.0 + padding)) * 0.5).floor();
    let px = pad(width) * 2.0;
    let py = pad(height) * 2.0;
    let x_zoom = (width - px) / bounds.width;
    let y_zoom = (height - py) / bounds.height;
    let zoom = clamp(x_zoom.min(y_zoom), min_zoom, max_zoom);
    let c = bounds.center();
    Viewport {
        x: width / 2.0 - c.x * zoom,
        y: height / 2.0 - c.y * zoom,
        zoom,
    }
}

/// How fast to pan, per frame, while the pointer is near a canvas edge.
pub fn auto_pan(pos: Xy, width: f32, height: f32, speed: f32, distance: f32) -> Xy {
    fn velocity(value: f32, min: f32, max: f32) -> f32 {
        if value < min {
            clamp((value - min).abs(), 1.0, min) / min
        } else if value > max {
            -clamp((value - max).abs(), 1.0, min) / min
        } else {
            0.0
        }
    }
    Xy::new(
        velocity(pos.x, distance, width - distance) * speed,
        velocity(pos.y, distance, height - distance) * speed,
    )
}

/// d3-zoom's wheel delta: the exponent applied to the zoom (base 2).
pub fn wheel_exponent(dy: f32, lines: bool, ctrl: bool) -> f32 {
    -dy * if lines { 0.05 } else { 0.002 } * if ctrl { 10.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_and_flow_round_trip() {
        let vp = Viewport { x: 30.0, y: -10.0, zoom: 2.0 };
        let p = Xy::new(12.0, 7.0);
        let back = vp.to_flow(vp.to_screen(p));
        assert!((back.x - p.x).abs() < 1e-5 && (back.y - p.y).abs() < 1e-5);
    }

    #[test]
    fn zoom_keeps_the_anchor_still() {
        let vp = Viewport { x: 30.0, y: 20.0, zoom: 1.0 };
        let at = Xy::new(100.0, 80.0);
        let before = vp.to_flow(at);
        let zoomed = vp.zoomed_at(1.5, at, 0.1, 3.0);
        let after = zoomed.to_flow(at);
        assert!((before.x - after.x).abs() < 1e-4 && (before.y - after.y).abs() < 1e-4);
        assert!((zoomed.zoom - 1.5).abs() < 1e-6);
    }

    #[test]
    fn zoom_is_clamped() {
        let vp = Viewport::default();
        assert_eq!(vp.zoomed_at(100.0, Xy::ZERO, 0.5, 2.0).zoom, 2.0);
        assert_eq!(vp.zoomed_at(0.001, Xy::ZERO, 0.5, 2.0).zoom, 0.5);
    }

    #[test]
    fn snapping_rounds_to_the_grid() {
        let p = snap_position(Xy::new(17.0, -9.0), (15.0, 15.0));
        assert_eq!(p, Xy::new(15.0, -15.0));
    }

    #[test]
    fn rects_union_and_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert_eq!(a.overlap_area(&b), 25.0);
        assert_eq!(a.overlap_area(&Rect::new(20.0, 20.0, 1.0, 1.0)), 0.0);
        assert_eq!(bounds_of([]), None);
    }

    #[test]
    fn fitting_centres_the_bounds() {
        let bounds = Rect::new(100.0, 100.0, 200.0, 100.0);
        let vp = viewport_for_bounds(bounds, 1000.0, 800.0, 0.1, 2.0, 0.1);
        let centre = vp.to_screen(bounds.center());
        assert!((centre.x - 500.0).abs() < 1e-3 && (centre.y - 400.0).abs() < 1e-3);
        assert!(vp.zoom <= 2.0);
        let visible = vp.visible_rect(1000.0, 800.0);
        assert!(visible.x < bounds.x && visible.right() > bounds.right());
    }

    #[test]
    fn extent_clamps_a_node() {
        let extent = Extent::new(Xy::new(0.0, 0.0), Xy::new(100.0, 100.0));
        assert_eq!(clamp_position(Xy::new(95.0, -5.0), &extent, 20.0, 20.0), Xy::new(80.0, 0.0));
    }

    #[test]
    fn auto_pan_pushes_away_from_edges() {
        let near_left = auto_pan(Xy::new(5.0, 200.0), 800.0, 600.0, 15.0, 40.0);
        assert!(near_left.x > 0.0 && near_left.y == 0.0);
        let near_right = auto_pan(Xy::new(795.0, 200.0), 800.0, 600.0, 15.0, 40.0);
        assert!(near_right.x < 0.0);
        assert_eq!(auto_pan(Xy::new(400.0, 300.0), 800.0, 600.0, 15.0, 40.0), Xy::ZERO);
    }

    #[test]
    fn wheel_zoom_direction() {
        assert!(wheel_exponent(-100.0, false, false) > 0.0);
        assert!(wheel_exponent(100.0, false, true) < wheel_exponent(100.0, false, false));
    }

    #[test]
    fn translate_extent_keeps_the_view_inside() {
        let extent = Extent::new(Xy::new(0.0, 0.0), Xy::new(1000.0, 1000.0));
        let vp = constrain_viewport(Viewport { x: 500.0, y: 0.0, zoom: 1.0 }, 400.0, 400.0, &extent);
        assert!(vp.to_flow(Xy::ZERO).x >= 0.0);
    }
}
