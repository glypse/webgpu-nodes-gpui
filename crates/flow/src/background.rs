use gpui::{
    BorderStyle, Bounds, DevicePixels, DynamicTexture, Hsla, Pixels, Point, Window, point, px, quad, size,
};

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
    let start = phase(pan, offset, gap);
    let count = ((length - start) / gap).floor().max(-1.0) as i32 + 1;
    (0..count.max(0)).map(move |i| start + gap * i as f32)
}

fn phase(pan: f32, offset: f32, gap: f32) -> f32 {
    (pan + offset).rem_euclid(gap)
}

/// Above this many marks the pattern is too fine to be seen, and drawing it
/// would only cost.
const MAX_MARKS: usize = 40_000;

/// Gaps narrower than this many logical px are not drawn.
const MIN_SPACING: f32 = 3.0;

/// Logical px spanned by one baked tile, which the canvas is covered with.
/// Each tile holds a whole number of gaps, so the pattern repeats exactly
/// across tiles.
const TILE_TARGET: f32 = 512.0;

/// Caps the baked tile's texels, so an extreme gap cannot allocate a huge texture.
const MAX_TILE_TEXELS: usize = 4096 * 4096;

/// Where the pattern sits in the canvas for one frame, in logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    gap: (f32, f32),
    /// Offset of the first mark from the canvas origin.
    phase: (f32, f32),
    offset: (f32, f32),
    periods: (u32, u32),
    /// Dot diameter, or cross arm length.
    mark: f32,
    line: f32,
}

fn periods_per_tile(gap: f32) -> u32 {
    (TILE_TARGET / gap).floor().max(1.0) as u32
}

impl Layout {
    fn new(w: f32, h: f32, vp: &Viewport, opts: &BackgroundOptions) -> Option<Self> {
        let gap = (opts.gap.0 * vp.zoom, opts.gap.1 * vp.zoom);
        if gap.0 < MIN_SPACING || gap.1 < MIN_SPACING {
            return None;
        }
        let offset = (opts.offset.0 * vp.zoom, opts.offset.1 * vp.zoom);
        let cols = pattern_positions(w, vp.x, gap.0, offset.0).count();
        let rows = pattern_positions(h, vp.y, gap.1, offset.1).count();
        let limit = match opts.variant {
            BackgroundVariant::Dots => MAX_MARKS,
            BackgroundVariant::Cross => MAX_MARKS / 2,
            BackgroundVariant::Lines => usize::MAX,
        };
        if cols.saturating_mul(rows) > limit {
            return None;
        }
        Some(Self {
            gap,
            phase: (phase(vp.x, offset.0, gap.0), phase(vp.y, offset.1, gap.1)),
            offset,
            periods: (periods_per_tile(gap.0), periods_per_tile(gap.1)),
            mark: opts.size * vp.zoom,
            line: opts.line_width * vp.zoom,
        })
    }

    fn tile(&self) -> (f32, f32) {
        (self.periods.0 as f32 * self.gap.0, self.periods.1 as f32 * self.gap.1)
    }
}

/// Tile origins along one axis, relative to the canvas origin, covering `length`.
/// Each tile spans `tile` px starting at `phase + k * tile`.
fn tile_origins(length: f32, phase: f32, tile: f32) -> impl Iterator<Item = f32> {
    (-1i32..)
        .map(move |k| phase + k as f32 * tile)
        .take_while(move |o| *o < length)
}

fn sprite_origins(w: f32, h: f32, layout: &Layout) -> impl Iterator<Item = (f32, f32)> {
    let (tx, ty) = layout.tile();
    let (phase_x, phase_y) = layout.phase;
    tile_origins(w, phase_x, tx).flat_map(move |x| tile_origins(h, phase_y, ty).map(move |y| (x, y)))
}

#[derive(Clone, Copy, PartialEq)]
struct TileKey {
    variant: BackgroundVariant,
    gap: (f32, f32),
    periods: (u32, u32),
    mark: f32,
    line: f32,
    color: Hsla,
    scale: f32,
}

impl TileKey {
    fn new(variant: BackgroundVariant, layout: &Layout, color: Hsla, scale: f32) -> Self {
        Self {
            variant,
            gap: layout.gap,
            periods: layout.periods,
            mark: layout.mark,
            line: layout.line,
            color,
            scale,
        }
    }

    fn texels(&self) -> (usize, usize) {
        let side = |periods: u32, gap: f32| (periods as f32 * gap * self.scale).round().max(1.0) as usize;
        (side(self.periods.0, self.gap.0), side(self.periods.1, self.gap.1))
    }
}

struct CachedTile {
    key: TileKey,
    texture: DynamicTexture,
}

/// Holds the baked pattern tile between frames. Pan only moves the sprites
/// that show it, so the texture is rebuilt only when zoom, colours or the
/// window's scale change.
#[derive(Default)]
pub(crate) struct BackgroundCache {
    tile: Option<CachedTile>,
    /// Set once the platform refuses dynamic textures, so marks are drawn
    /// individually from then on instead of retrying every frame.
    unsupported: bool,
}

impl BackgroundCache {
    pub(crate) fn paint(
        &mut self,
        window: &mut Window,
        bounds: Bounds<Pixels>,
        vp: &Viewport,
        opts: &BackgroundOptions,
        color: Hsla,
    ) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let Some(layout) = Layout::new(w, h, vp, opts) else {
            return;
        };
        let key = TileKey::new(opts.variant, &layout, color, window.scale_factor());
        if self.tile.as_ref().map(|t| t.key) != Some(key) {
            self.release();
            self.bake(window, key);
        }
        match &self.tile {
            Some(tile) => paint_tiles(window, bounds, tile.texture, &layout),
            None => paint_marks(window, bounds, vp, opts, &layout, color),
        }
    }

    fn bake(&mut self, window: &mut Window, key: TileKey) {
        if self.unsupported {
            return;
        }
        let (tw, th) = key.texels();
        if tw.saturating_mul(th) > MAX_TILE_TEXELS {
            return;
        }
        let bytes = rasterize(&key, (tw, th));
        let (Ok(ew), Ok(eh)) = (i32::try_from(tw), i32::try_from(th)) else {
            return;
        };
        let extent = size(DevicePixels(ew), DevicePixels(eh));
        match window.create_dynamic_texture(extent, &bytes, (tw * 4) as u32) {
            Ok(texture) => self.tile = Some(CachedTile { key, texture }),
            Err(err) => {
                log::warn!("background pattern texture unavailable, drawing marks individually: {err:#}");
                self.unsupported = true;
            }
        }
    }

    fn release(&mut self) {
        if let Some(tile) = self.tile.take() {
            tile.texture.remove();
        }
    }
}

impl Drop for BackgroundCache {
    fn drop(&mut self) {
        self.release();
    }
}

fn paint_tiles(window: &mut Window, bounds: Bounds<Pixels>, texture: DynamicTexture, layout: &Layout) {
    let (tx, ty) = layout.tile();
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    for (x, y) in sprite_origins(w, h, layout) {
        let origin = point(bounds.origin.x + px(x), bounds.origin.y + px(y));
        if let Err(err) = window.paint_dynamic_texture(Bounds::new(origin, size(px(tx), px(ty))), texture) {
            log::warn!("background pattern tile not painted: {err:#}");
            return;
        }
    }
}

/// Premultiplied BGRA of one tile, holding the marks of one period.
fn rasterize(key: &TileKey, (tw, th): (usize, usize)) -> Vec<u8> {
    let mut canvas = Canvas::new(tw, th, key.color);
    let (w, h) = (tw as f32, th as f32);
    let pitch = (w / key.periods.0 as f32, h / key.periods.1 as f32);
    let scale = key.scale;
    let line = key.line.max(1.0) * scale;
    match key.variant {
        BackgroundVariant::Dots => {
            let r = key.mark.max(1.0) * scale / 2.0;
            for j in 0..=key.periods.0 {
                for i in 0..=key.periods.1 {
                    let (cx, cy) = (j as f32 * pitch.0, i as f32 * pitch.1);
                    canvas.fill(cx - r - 1.0, cx + r + 1.0, cy - r - 1.0, cy + r + 1.0, |x, y| {
                        let dist = ((x + 0.5 - cx).powi(2) + (y + 0.5 - cy).powi(2)).sqrt();
                        (r + 0.5 - dist).clamp(0.0, 1.0)
                    });
                }
            }
        }
        BackgroundVariant::Cross => {
            let arm = key.mark * scale / 2.0;
            for j in 0..=key.periods.0 {
                for i in 0..=key.periods.1 {
                    let (cx, cy) = (j as f32 * pitch.0, i as f32 * pitch.1);
                    canvas.fill_rect(cx - arm, cx + arm, cy - line / 2.0, cy + line / 2.0);
                    canvas.fill_rect(cx - line / 2.0, cx + line / 2.0, cy - arm, cy + arm);
                }
            }
        }
        BackgroundVariant::Lines => {
            for j in 0..=key.periods.0 {
                let cx = j as f32 * pitch.0;
                canvas.fill_rect(cx - line / 2.0, cx + line / 2.0, 0.0, h);
            }
            for i in 0..=key.periods.1 {
                let cy = i as f32 * pitch.1;
                canvas.fill_rect(0.0, w, cy - line / 2.0, cy + line / 2.0);
            }
        }
    }
    canvas.bgra
}

struct Canvas {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
    /// Straight colour in B, G, R order.
    rgb: [f32; 3],
    alpha: f32,
}

impl Canvas {
    fn new(width: usize, height: usize, color: Hsla) -> Self {
        let c = color.to_rgb();
        Self {
            width,
            height,
            bgra: vec![0; width * height * 4],
            rgb: [c.b, c.g, c.r],
            alpha: c.a,
        }
    }

    /// Composites `coverage(x, y)` over every texel of the box, where `x` and
    /// `y` are the texel's top-left corner.
    fn fill(&mut self, x0: f32, x1: f32, y0: f32, y1: f32, coverage: impl Fn(f32, f32) -> f32) {
        let (w, h) = (self.width as f32, self.height as f32);
        let ix = x0.floor().clamp(0.0, w) as usize..x1.ceil().clamp(0.0, w) as usize;
        let iy = y0.floor().clamp(0.0, h) as usize..y1.ceil().clamp(0.0, h) as usize;
        for y in iy {
            for x in ix.clone() {
                let c = coverage(x as f32, y as f32);
                self.blend(x, y, c);
            }
        }
    }

    fn fill_rect(&mut self, x0: f32, x1: f32, y0: f32, y1: f32) {
        self.fill(x0, x1, y0, y1, move |x, y| {
            ((x + 1.0).min(x1) - x.max(x0)).clamp(0.0, 1.0) * ((y + 1.0).min(y1) - y.max(y0)).clamp(0.0, 1.0)
        });
    }

    fn blend(&mut self, x: usize, y: usize, coverage: f32) {
        let a = self.alpha * coverage.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = (y * self.width + x) * 4;
        let Some(texel) = self.bgra.get_mut(i..i + 4) else {
            return;
        };
        for (c, dst) in texel.iter_mut().enumerate() {
            let src = if c < 3 { self.rgb[c] * a } else { a };
            let v = src + f32::from(*dst) / 255.0 * (1.0 - a);
            *dst = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// The per-mark path, used when the platform cannot hold the baked tile.
fn paint_marks(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    vp: &Viewport,
    opts: &BackgroundOptions,
    layout: &Layout,
    color: Hsla,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let xs: Vec<f32> = pattern_positions(w, vp.x, layout.gap.0, layout.offset.0).collect();
    let ys: Vec<f32> = pattern_positions(h, vp.y, layout.gap.1, layout.offset.1).collect();
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
            let d = layout.mark.max(1.0);
            for &y in &ys {
                for &x in &xs {
                    rect(window, x - d / 2.0, y - d / 2.0, d, d, d / 2.0);
                }
            }
        }
        BackgroundVariant::Lines => {
            let t = layout.line.max(1.0);
            for &x in &xs {
                rect(window, x - t / 2.0, 0.0, t, h, 0.0);
            }
            for &y in &ys {
                rect(window, 0.0, y - t / 2.0, w, t, 0.0);
            }
        }
        BackgroundVariant::Cross => {
            let arm = layout.mark;
            let t = layout.line.max(1.0);
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

    fn vp(x: f32, y: f32, zoom: f32) -> Viewport {
        Viewport { x, y, zoom }
    }

    /// Sprites the baked-tile path paints for a canvas.
    fn tile_sprites(w: f32, h: f32, vp: &Viewport, opts: &BackgroundOptions) -> usize {
        Layout::new(w, h, vp, opts).map_or(0, |layout| sprite_origins(w, h, &layout).count())
    }

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

    #[test]
    fn tiles_cover_the_canvas_at_any_pan() {
        for &pan in &[0.0, 1.0, 137.5, 511.0, 512.0, -3.0] {
            let layout = Layout::new(1600.0, 1000.0, &vp(pan, pan, 0.5), &BackgroundOptions::default()).unwrap();
            let (tx, ty) = layout.tile();
            let xs: Vec<f32> = tile_origins(1600.0, layout.phase.0, tx).collect();
            for probe in [0.0, 1.0, 800.0, 1599.0] {
                assert!(xs.iter().any(|&o| o <= probe && probe < o + tx), "pan {pan} misses {probe}");
            }
            let ys: Vec<f32> = tile_origins(1000.0, layout.phase.1, ty).collect();
            for probe in [0.0, 999.0] {
                assert!(ys.iter().any(|&o| o <= probe && probe < o + ty), "pan {pan} misses {probe}");
            }
        }
    }

    #[test]
    fn tile_is_independent_of_pan() {
        let opts = BackgroundOptions::default();
        let a = Layout::new(800.0, 600.0, &vp(0.0, 0.0, 1.0), &opts).unwrap();
        let b = Layout::new(800.0, 600.0, &vp(7.0, 3.0, 1.0), &opts).unwrap();
        assert_eq!(a.periods, b.periods);
        assert_eq!(a.gap, b.gap);
        assert_ne!(a.phase, b.phase);
    }

    #[test]
    fn dots_at_minimum_zoom_are_bounded_sprites() {
        let opts = BackgroundOptions::default();
        assert_eq!(tile_sprites(1600.0, 1000.0, &vp(0.0, 0.0, 0.5), &opts), 15);
        assert_eq!(tile_sprites(1600.0, 1000.0, &vp(0.0, 0.0, 0.1), &opts), 0);
    }

    #[test]
    fn lines_and_cross_use_bounded_sprites() {
        let lines = BackgroundOptions::default().variant(BackgroundVariant::Lines);
        assert_eq!(tile_sprites(1600.0, 1000.0, &vp(0.0, 0.0, 0.5), &lines), 15);
        let cross = BackgroundOptions::default().variant(BackgroundVariant::Cross);
        assert_eq!(tile_sprites(1600.0, 1000.0, &vp(0.0, 0.0, 0.5), &cross), 15);
    }

    #[test]
    fn dot_tile_has_marks_at_each_gap_only() {
        let key = TileKey {
            variant: BackgroundVariant::Dots,
            gap: (20.0, 20.0),
            periods: (25, 25),
            mark: 4.0,
            line: 1.0,
            color: gpui::black(),
            scale: 1.0,
        };
        let (tw, th) = key.texels();
        assert_eq!((tw, th), (500, 500));
        let bgra = rasterize(&key, (tw, th));
        let alpha = |x: usize, y: usize| bgra[(y * tw + x) * 4 + 3];
        assert_eq!(alpha(100, 100), 255);
        assert_eq!(alpha(110, 110), 0);
        assert_eq!(alpha(100, 110), 0);
    }
}
