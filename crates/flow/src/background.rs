use gpui::{BorderStyle, Bounds, Hsla, Pixels, Point, Window, point, px, quad, size};

use crate::geometry::Viewport;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackgroundVariant {
    #[default]
    Dots,
    Lines,
    Cross,
}

#[derive(Clone, Copy, Debug)]
pub struct BackgroundOptions {
    pub variant: BackgroundVariant,
    pub gap: (f32, f32),
    /// Dot diameter or cross arm length, in flow units.
    pub size: f32,
    pub line_width: f32,
    /// Shifts the pattern, in flow units.
    pub offset: (f32, f32),
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
}

impl Default for BackgroundOptions {
    fn default() -> Self {
        Self {
            variant: BackgroundVariant::Dots,
            gap: (20.0, 20.0),
            size: 1.0,
            line_width: 1.0,
            offset: (0.0, 0.0),
            color: None,
            background: None,
        }
    }
}

impl BackgroundOptions {
    pub fn variant(mut self, variant: BackgroundVariant) -> Self {
        self.variant = variant;
        if variant == BackgroundVariant::Cross {
            self.size = 6.0;
        }
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = (gap, gap);
        self
    }
}

/// Positions of pattern lines along one axis within `length` pixels.
pub(crate) fn pattern_positions(length: f32, pan: f32, gap: f32, offset: f32) -> impl Iterator<Item = f32> {
    let start = (pan + offset).rem_euclid(gap);
    let count = ((length - start) / gap).floor().max(-1.0) as i32 + 1;
    (0..count.max(0)).map(move |i| start + gap * i as f32)
}

/// Above this many marks the pattern is too fine to be seen, and drawing it
/// would only cost.
const MAX_MARKS: usize = 40_000;

pub(crate) fn paint(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    vp: &Viewport,
    opts: &BackgroundOptions,
    color: Hsla,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let gx = opts.gap.0 * vp.zoom;
    let gy = opts.gap.1 * vp.zoom;
    if gx < 3.0 || gy < 3.0 {
        return;
    }
    let xs: Vec<f32> = pattern_positions(w, vp.x, gx, opts.offset.0 * vp.zoom).collect();
    let ys: Vec<f32> = pattern_positions(h, vp.y, gy, opts.offset.1 * vp.zoom).collect();
    let at = |x: f32, y: f32| -> Point<Pixels> { point(bounds.origin.x + px(x), bounds.origin.y + px(y)) };
    let rect = |window: &mut Window, x: f32, y: f32, rw: f32, rh: f32, r: f32| {
        window.paint_quad(quad(
            Bounds::new(at(x, y), size(px(rw), px(rh))),
            px(r),
            color,
            px(0.0),
            gpui::transparent_black(),
            BorderStyle::Solid,
        ));
    };
    match opts.variant {
        BackgroundVariant::Dots => {
            if xs.len() * ys.len() > MAX_MARKS {
                return;
            }
            let d = (opts.size * vp.zoom).max(1.0);
            for &y in &ys {
                for &x in &xs {
                    rect(window, x - d / 2.0, y - d / 2.0, d, d, d / 2.0);
                }
            }
        }
        BackgroundVariant::Lines => {
            let t = (opts.line_width * vp.zoom).max(1.0);
            for &x in &xs {
                rect(window, x - t / 2.0, 0.0, t, h, 0.0);
            }
            for &y in &ys {
                rect(window, 0.0, y - t / 2.0, w, t, 0.0);
            }
        }
        BackgroundVariant::Cross => {
            if xs.len() * ys.len() > MAX_MARKS / 2 {
                return;
            }
            let arm = opts.size * vp.zoom;
            let t = (opts.line_width * vp.zoom).max(1.0);
            for &y in &ys {
                for &x in &xs {
                    rect(window, x - arm / 2.0, y - t / 2.0, arm, t, 0.0);
                    rect(window, x - t / 2.0, y - arm / 2.0, t, arm, 0.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_follows_the_pan() {
        let xs: Vec<f32> = pattern_positions(100.0, 5.0, 20.0, 0.0).collect();
        assert_eq!(xs, vec![5.0, 25.0, 45.0, 65.0, 85.0]);
        let xs: Vec<f32> = pattern_positions(100.0, -15.0, 20.0, 0.0).collect();
        assert_eq!(xs[0], 5.0);
    }

    #[test]
    fn pattern_covers_the_canvas() {
        let xs: Vec<f32> = pattern_positions(1000.0, 123.0, 25.0, 0.0).collect();
        assert!(xs.first().unwrap() < &25.0);
        assert!(xs.last().unwrap() > &975.0);
    }
}
