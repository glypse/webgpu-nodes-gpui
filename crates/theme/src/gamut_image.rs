//! Pixels of an OKLch plane, as the colour picker's wheel and lightness bar draw them.

use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use gpui::RenderImage;
use graph_math::color::{
    ColorSpace, Lch, encode_in, in_gamut, oklch_to_linear, oklch_to_linear_unmapped,
};
use image::{Frame, RgbaImage};
use lru::LruCache;

/// What the pixels of a generated plane depend on. The components are
/// quantised here and the plane is generated from the quantised values, so
/// equal keys always paint equal images.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaneKey {
    /// Lightness in 1/400ths.
    Wheel { lightness: i32, gamut: ColorSpace },
    /// Chroma in 1/1000ths, hue in 1/10ths of a degree.
    Bar {
        chroma: i32,
        hue: i32,
        gamut: ColorSpace,
    },
}

/// Room for the wheels and bars of a few groups' colour nodes. A drag through
/// lightness or chroma makes a new key per step, so the bound keeps those from
/// piling up.
const PLANE_CACHE_CAPACITY: usize = 64;

static PLANES: LazyLock<Mutex<ImageCache<PlaneKey>>> =
    LazyLock::new(|| Mutex::new(ImageCache::new(PLANE_CACHE_CAPACITY)));

/// The image of `key` from the process-wide cache, generated with `generate` on a miss.
pub fn plane_image(key: PlaneKey, generate: impl FnOnce() -> Frame) -> Arc<RenderImage> {
    PLANES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(key, generate)
}

/// Rendered images by key, least recently used evicted beyond `capacity`.
struct ImageCache<K: Hash + Eq> {
    entries: LruCache<K, Arc<RenderImage>>,
}

impl<K: Hash + Eq> ImageCache<K> {
    pub fn new(capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            entries: LruCache::new(capacity),
        }
    }

    pub fn get_or_insert_with(
        &mut self,
        key: K,
        generate: impl FnOnce() -> Frame,
    ) -> Arc<RenderImage> {
        if let Some(image) = self.entries.get(&key) {
            return image.clone();
        }
        let image = Arc::new(RenderImage::new(vec![generate()]));
        self.entries.put(key, image.clone());
        image
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Sub-samples per side for the pixels on the gamut's edge.
const EDGE_SAMPLES: u32 = 4;
/// Spacing, in pixels, of the grid the colours outside the gamut are mapped
/// on: mapping each pixel is too slow to repaint while dragging, and mapped
/// colours change smoothly enough to interpolate between its points.
const MAPPED_GRID: u32 = 6;
/// How much the outline covers the colour, at its middle.
const OUTLINE_OPACITY: f64 = 0.85;

/// A pixel in gpui's image order (blue, green, red, alpha), whose colours are sRGB-encoded.
pub(crate) fn bgra(r: f64, g: f64, b: f64) -> [u8; 4] {
    let byte = |n: f64| (n.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(b), byte(g), byte(r), 255]
}

/// Paints, pixel by pixel, the OKLch colour `at` gives for each point of a
/// `width` by `height` image (in [0, 1] from its top left; black where it
/// gives none), in `gamut`. Colours outside it show what they're mapped to,
/// and a thin line traces its edge, anti-aliased by sub-sampling the pixels
/// it crosses.
pub fn paint_gamut(
    width: u32,
    height: u32,
    gamut: ColorSpace,
    at: impl Fn(f64, f64) -> Option<Lch>,
) -> Frame {
    let lch_at = |x: f64, y: f64| at(x / f64::from(width), y / f64::from(height));
    let fits = |lch: Lch| in_gamut(oklch_to_linear_unmapped(lch), gamut);
    let encoded = |lch: Lch| {
        let c = encode_in(oklch_to_linear(lch, gamut), gamut);
        [c.r, c.g, c.b]
    };

    let count = (width * height) as usize;
    // Per pixel: 1 inside the gamut, 0 outside, -1 where nothing is drawn;
    // its encoded colour when inside, and its lightness.
    let mut state = vec![-1i8; count];
    let mut colors = vec![[0.0f64; 3]; count];
    let mut lightness = vec![0.0f64; count];
    for i in 0..count {
        let (x, y) = (i as u32 % width, i as u32 / width);
        let Some(lch) = lch_at(f64::from(x) + 0.5, f64::from(y) + 0.5) else {
            continue;
        };
        lightness[i] = lch.l;
        let linear = oklch_to_linear_unmapped(lch);
        if !in_gamut(linear, gamut) {
            state[i] = 0;
            continue;
        }
        state[i] = 1;
        let c = encode_in(linear, gamut);
        colors[i] = [c.r, c.g, c.b];
    }

    let columns = width.div_ceil(MAPPED_GRID) + 1;
    let rows = height.div_ceil(MAPPED_GRID) + 1;
    let mapped: Vec<Option<[f64; 3]>> = (0..columns * rows)
        .map(|i| {
            let (gx, gy) = (i % columns, i / columns);
            lch_at(f64::from(gx * MAPPED_GRID), f64::from(gy * MAPPED_GRID)).map(encoded)
        })
        .collect();
    // A mapped colour between the grid's points, or exactly when one is missing.
    let mapped_at = |x: u32, y: u32| -> [f64; 3] {
        let gx = (f64::from(x) + 0.5) / f64::from(MAPPED_GRID);
        let gy = (f64::from(y) + 0.5) / f64::from(MAPPED_GRID);
        let (x0, y0) = (gx.floor() as u32, gy.floor() as u32);
        let corner = |dx: u32, dy: u32| {
            mapped
                .get(((y0 + dy) * columns + x0 + dx) as usize)
                .copied()
                .flatten()
        };
        let (Some(a), Some(b), Some(c), Some(d)) =
            (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1))
        else {
            return lch_at(f64::from(x) + 0.5, f64::from(y) + 0.5).map_or([0.0; 3], encoded);
        };
        let (tx, ty) = (gx - f64::from(x0), gy - f64::from(y0));
        std::array::from_fn(|k| {
            (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty
        })
    };
    // Whether a neighbour is drawn and on the other side of the gamut's edge.
    let across = |x: i64, y: i64, inside: i8| {
        x >= 0
            && y >= 0
            && x < i64::from(width)
            && y < i64::from(height)
            && state[(y * i64::from(width) + x) as usize] == 1 - inside
    };

    let mut image = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            let inside = state[i];
            if inside == -1 {
                image.put_pixel(x, y, image::Rgba(bgra(0.0, 0.0, 0.0)));
                continue;
            }
            let [mut r, mut g, mut b] = if inside == 1 {
                colors[i]
            } else {
                mapped_at(x, y)
            };
            let (xi, yi) = (i64::from(x), i64::from(y));
            let edge = across(xi - 1, yi, inside)
                || across(xi + 1, yi, inside)
                || across(xi, yi - 1, inside)
                || across(xi, yi + 1, inside);
            if edge {
                let (mut fitting, mut samples) = (0u32, 0u32);
                for sy in 0..EDGE_SAMPLES {
                    for sx in 0..EDGE_SAMPLES {
                        let px = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(EDGE_SAMPLES);
                        let py = f64::from(y) + (f64::from(sy) + 0.5) / f64::from(EDGE_SAMPLES);
                        let Some(lch) = lch_at(px, py) else { continue };
                        samples += 1;
                        if fits(lch) {
                            fitting += 1;
                        }
                    }
                }
                // The share inside tells, roughly, how far the edge passes from the
                // pixel's centre (half a pixel when it only grazes it): the line
                // fades over that distance, split between the pixels either side.
                let share = if samples > 0 {
                    f64::from(fitting) / f64::from(samples)
                } else {
                    0.0
                };
                let distance = (share - 0.5).abs();
                let line = (1.0 - 1.5 * distance).max(0.0) * OUTLINE_OPACITY;
                // Dark on light colours, light on dark ones.
                let ink = if lightness[i] > 0.6 { 0.0 } else { 1.0 };
                r += (ink - r) * line;
                g += (ink - g) * line;
                b += (ink - b) * line;
            }
            image.put_pixel(x, y, image::Rgba(bgra(r, g, b)));
        }
    }
    Frame::new(image)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        frame.buffer().get_pixel(x, y).0
    }

    #[test]
    fn leaves_what_isnt_drawn_black() {
        let frame = paint_gamut(8, 8, ColorSpace::Srgb, |_, _| None);
        assert_eq!(pixel(&frame, 3, 3), [0, 0, 0, 255]);
    }

    #[test]
    fn paints_colours_in_the_gamut_as_they_are() {
        // Grey: always inside.
        let frame = paint_gamut(8, 8, ColorSpace::Srgb, |_, _| {
            Some(Lch {
                l: 0.5,
                c: 0.0,
                h: 0.0,
            })
        });
        let [b, g, r, a] = pixel(&frame, 4, 4);
        assert_eq!(a, 255);
        assert!(b == g && g == r);
        // OKLab lightness 0.5 is 0.125 linear, which encodes to about 0.39.
        assert!((i32::from(r) - 99).abs() <= 2, "{r}");
    }

    #[test]
    fn maps_colours_outside_the_gamut_into_it() {
        // Far past sRGB's chroma at every lightness: nothing fits, all is mapped.
        let frame = paint_gamut(24, 24, ColorSpace::Srgb, |_, y| {
            Some(Lch {
                l: 0.2 + 0.6 * y,
                c: 0.5,
                h: 30.0,
            })
        });
        let [b, g, r, _] = pixel(&frame, 12, 12);
        assert!(r > g && r > b, "red hue keeps its tint: {r} {g} {b}");
    }

    #[test]
    fn draws_a_line_where_the_gamut_ends() {
        // Chroma grows to the right: inside on the left, mapped on the right.
        let frame = paint_gamut(64, 8, ColorSpace::Srgb, |x, _| {
            Some(Lch {
                l: 0.7,
                c: x * 0.4,
                h: 140.0,
            })
        });
        let sum = |x: u32| {
            pixel(&frame, x, 4)[..3]
                .iter()
                .map(|n| u32::from(*n))
                .sum::<u32>()
        };
        let darkest = (0..64).map(sum).min().unwrap_or(0);
        // The line is dark on this light colour, much darker than the colours around.
        assert!(darkest * 10 < sum(0) * 6, "{darkest} vs {}", sum(0));
    }

    fn get(cache: &mut ImageCache<u32>, generated: &Cell<u32>, key: u32) -> Arc<RenderImage> {
        cache.get_or_insert_with(key, || {
            generated.set(generated.get() + 1);
            paint_gamut(2, 2, ColorSpace::Srgb, |_, _| None)
        })
    }

    #[test]
    fn a_hit_returns_the_same_image_without_generating_it() {
        let generated = Cell::new(0);
        let mut cache = ImageCache::new(4);
        let first = get(&mut cache, &generated, 1);
        let again = get(&mut cache, &generated, 1);
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(generated.get(), 1);
    }

    #[test]
    fn the_least_recently_used_image_is_evicted_past_capacity() {
        let generated = Cell::new(0);
        let mut cache = ImageCache::new(2);
        let one = get(&mut cache, &generated, 1);
        get(&mut cache, &generated, 2);
        // Using 1 again makes 2 the least recently used.
        get(&mut cache, &generated, 1);
        get(&mut cache, &generated, 3);
        assert_eq!(cache.len(), 2);
        assert_eq!(generated.get(), 3);
        assert!(Arc::ptr_eq(&one, &get(&mut cache, &generated, 1)));
        assert_eq!(generated.get(), 3);
        get(&mut cache, &generated, 2);
        assert_eq!(generated.get(), 4);
    }
}
