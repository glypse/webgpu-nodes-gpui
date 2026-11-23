//! Colour ramps: a factor mapped to a colour through stops, each a position
//! in [0, 1] and a colour. Past its first and last stops, a ramp keeps their
//! colours. Between two stops, colours are interpolated in a colour space:
//! RGB (linear), HSV (of the sRGB-encoded colour) or OKLch, their hues taking
//! the way around the circle a hue interpolation picks.
//!
//! The interpolations are Linear, Ease, Cardinal, B-Spline, Constant and
//! Smooth: a monotone cubic (Steffen's), which is smooth through every stop
//! without overshooting, and follows their spacing. The others ignore it, as
//! each segment is uniformly parameterised.
//!
//! `evaluate_ramp` is the shader's computation on the CPU, stage by stage: the
//! two must change together.

use serde::{Deserialize, Serialize};

use crate::color::{Lch, Rgb, decode_rgb, encode_rgb, linear_to_oklch, oklch_to_linear_unmapped};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RampStop {
    pub id: String,
    pub position: f64,
}

/// Four components: a colour's three in a ramp's space, then alpha.
pub type Vec4 = [f64; 4];

pub const RAMP_LINEAR: i64 = 0;
pub const RAMP_EASE: i64 = 1;
pub const RAMP_SMOOTH: i64 = 2;
pub const RAMP_CARDINAL: i64 = 3;
pub const RAMP_B_SPLINE: i64 = 4;
pub const RAMP_CONSTANT: i64 = 5;

/// Each variant's value and label.
pub const RAMP_INTERPOLATIONS: [(i64, &str); 6] = [
    (RAMP_LINEAR, "Linear"),
    (RAMP_EASE, "Ease"),
    (RAMP_SMOOTH, "Smooth"),
    (RAMP_CARDINAL, "Cardinal"),
    (RAMP_B_SPLINE, "B-Spline"),
    (RAMP_CONSTANT, "Constant"),
];

pub const RAMP_RGB: i64 = 0;
pub const RAMP_HSV: i64 = 1;
pub const RAMP_OKLCH: i64 = 2;

/// Each variant's value and label.
pub const RAMP_SPACES: [(i64, &str); 3] =
    [(RAMP_RGB, "RGB"), (RAMP_HSV, "HSV"), (RAMP_OKLCH, "OKLch")];

pub const HUE_SHORTER: i64 = 0;
pub const HUE_LONGER: i64 = 1;
pub const HUE_INCREASING: i64 = 2;
pub const HUE_DECREASING: i64 = 3;

/// Each variant's value and label, named after CSS Color 4's hue interpolation methods.
pub const RAMP_HUES: [(i64, &str); 4] = [
    (HUE_SHORTER, "Shorter"),
    (HUE_LONGER, "Longer"),
    (HUE_INCREASING, "Increasing"),
    (HUE_DECREASING, "Decreasing"),
];

/// Below this saturation (HSV) or chroma (OKLch), a colour's hue doesn't show.
pub const RAMP_ACHROMATIC: f64 = crate::color::ACHROMATIC;

/// Cardinal spline tension.
pub const RAMP_CARDINAL_TENSION: f64 = 0.71;

const STOP_PREFIX: &str = "stop-";

pub fn is_ramp_stop_id(id: &str) -> bool {
    id.strip_prefix(STOP_PREFIX)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn stop(n: usize, position: f64) -> RampStop {
    RampStop {
        id: format!("{STOP_PREFIX}{n}"),
        position,
    }
}

/// Black to white.
pub fn default_ramp() -> Vec<RampStop> {
    vec![stop(0, 0.0), stop(1, 1.0)]
}

/// A ramp's stops as its node holds them: sorted by position (equal ones
/// keeping their order) within [0, 1], ids unique; the default ramp if none.
pub fn sorted_ramp_stops(stops: Option<&[RampStop]>) -> Vec<RampStop> {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<RampStop> = stops
        .unwrap_or_default()
        .iter()
        .filter(|s| is_ramp_stop_id(&s.id) && seen.insert(s.id.clone()))
        .map(|s| RampStop {
            id: s.id.clone(),
            position: clamp01(s.position),
        })
        .collect();
    if kept.is_empty() {
        return default_ramp();
    }
    kept.sort_by(|a, b| a.position.total_cmp(&b.position));
    kept
}

fn clamp01(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.clamp(0.0, 1.0) }
}

fn free_stop_id(stops: &[RampStop]) -> String {
    let mut n = 0;
    while stops.iter().any(|s| s.id == format!("{STOP_PREFIX}{n}")) {
        n += 1;
    }
    format!("{STOP_PREFIX}{n}")
}

/// Add a stop at `position`; returns the ramp and the stop.
pub fn add_ramp_stop(stops: &[RampStop], position: f64) -> (Vec<RampStop>, RampStop) {
    let added = RampStop {
        id: free_stop_id(stops),
        position: clamp01(position),
    };
    let mut all = stops.to_vec();
    all.push(added.clone());
    (sorted_ramp_stops(Some(&all)), added)
}

/// Where a new stop goes: halfway between the selected one and the next
/// (or the previous, for the last), or in the middle without one.
pub fn new_stop_position(stops: &[RampStop], selected: Option<&str>) -> f64 {
    let Some(index) = stops.iter().position(|s| Some(s.id.as_str()) == selected) else {
        return 0.5;
    };
    let at = &stops[index];
    let neighbour = stops
        .get(index + 1)
        .or_else(|| index.checked_sub(1).and_then(|i| stops.get(i)));
    match neighbour {
        Some(n) => (at.position + n.position) / 2.0,
        None if at.position < 0.5 => 1.0,
        None => 0.0,
    }
}

/// Remove a stop, keeping at least one.
pub fn remove_ramp_stop(stops: &[RampStop], id: &str) -> Vec<RampStop> {
    if stops.len() <= 1 {
        return stops.to_vec();
    }
    stops.iter().filter(|s| s.id != id).cloned().collect()
}

/// Move a stop, which may pass others: they swap places.
pub fn move_ramp_stop(stops: &[RampStop], id: &str, position: f64) -> Vec<RampStop> {
    let moved: Vec<RampStop> = stops
        .iter()
        .map(|s| {
            if s.id == id {
                RampStop {
                    position: clamp01(position),
                    ..s.clone()
                }
            } else {
                s.clone()
            }
        })
        .collect();
    sorted_ramp_stops(Some(&moved))
}

/// Mirror the ramp: positions become 1 - position.
pub fn flip_ramp(stops: &[RampStop]) -> Vec<RampStop> {
    let flipped: Vec<RampStop> = stops
        .iter()
        .rev()
        .map(|s| RampStop {
            position: 1.0 - s.position,
            ..s.clone()
        })
        .collect();
    sorted_ramp_stops(Some(&flipped))
}

/// Space the stops evenly from 0 to 1, in their order.
pub fn distribute_ramp(stops: &[RampStop]) -> Vec<RampStop> {
    let last = stops.len().saturating_sub(1).max(1) as f64;
    stops
        .iter()
        .enumerate()
        .map(|(i, s)| RampStop {
            position: if stops.len() == 1 {
                0.5
            } else {
                i as f64 / last
            },
            ..s.clone()
        })
        .collect()
}

/// What a ramp's enum inputs pick, by variant value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RampModes {
    pub interpolation: i64,
    pub space: i64,
    pub hue: i64,
}

impl Default for RampModes {
    fn default() -> Self {
        Self {
            interpolation: RAMP_LINEAR,
            space: RAMP_RGB,
            hue: HUE_SHORTER,
        }
    }
}

/// A stop with its colour: linear RGB and alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampColorStop {
    pub position: f64,
    pub color: Vec4,
}

fn fract(n: f64) -> f64 {
    n - n.floor()
}

fn add(a: Vec4, b: Vec4) -> Vec4 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn sub(a: Vec4, b: Vec4) -> Vec4 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

fn scale(a: Vec4, k: f64) -> Vec4 {
    a.map(|n| n * k)
}

fn mix(a: Vec4, b: Vec4, t: f64) -> Vec4 {
    add(a, scale(sub(b, a), t))
}

fn weighted(points: [Vec4; 4], weights: [f64; 4]) -> Vec4 {
    points
        .iter()
        .zip(weights)
        .fold([0.0; 4], |sum, (p, w)| add(sum, scale(*p, w)))
}

/// HSV of an sRGB-encoded colour, unclamped above (HDR); hue in turns.
fn encoded_to_hsv(Rgb { r, g, b }: Rgb) -> [f64; 3] {
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let h = if delta > 0.0 {
        if max == r {
            (g - b) / delta + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        }
    } else {
        0.0
    };
    [h / 6.0, if max > 0.0 { delta / max } else { 0.0 }, max]
}

fn hsv_to_encoded(h: f64, s: f64, v: f64) -> Rgb {
    let sector = fract(h) * 6.0;
    let channel = |n: f64| {
        let k = (n + sector) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    Rgb::new(channel(5.0), channel(3.0), channel(1.0))
}

/// A colour in `space`, polar ones hue first (in turns).
pub fn ramp_to_space(color: Vec4, space: i64) -> Vec4 {
    let [r, g, b, a] = color;
    match space {
        RAMP_HSV => {
            let [h, s, v] = encoded_to_hsv(encode_rgb(Rgb::new(r, g, b)));
            [h, s, v, a]
        }
        RAMP_OKLCH => {
            let Lch { l, c, h } = linear_to_oklch(Rgb::new(r, g, b));
            [fract(h / 360.0), c, l, a]
        }
        _ => color,
    }
}

/// The linear RGB of a colour in `space`, saturation, chroma, value and lightness kept in range.
pub fn ramp_from_space(color: Vec4, space: i64) -> Vec4 {
    let [x, y, z, a] = color;
    match space {
        RAMP_HSV => {
            let encoded = hsv_to_encoded(x, clamp01(y), z.max(0.0));
            let Rgb { r, g, b } = decode_rgb(encoded);
            [r, g, b, a]
        }
        RAMP_OKLCH => {
            let Rgb { r, g, b } = oklch_to_linear_unmapped(Lch {
                l: z.max(0.0),
                c: y.max(0.0),
                h: fract(x) * 360.0,
            });
            [r, g, b, a]
        }
        _ => color,
    }
}

/// How far a hue goes (in turns) from `from` to `to`, the way `mode` picks.
pub fn hue_delta(from: f64, to: f64, mode: i64) -> f64 {
    let d = fract(to - from);
    match mode {
        HUE_LONGER => {
            if d < 0.5 {
                d - 1.0
            } else {
                d
            }
        }
        HUE_INCREASING => d,
        HUE_DECREASING => {
            if d == 0.0 {
                0.0
            } else {
                d - 1.0
            }
        }
        _ => {
            if d > 0.5 {
                d - 1.0
            } else {
                d
            }
        }
    }
}

/// The hues of four neighbouring stops unwrapped along the ramp, each going
/// from the one before the way `mode` picks. An achromatic stop takes the
/// hue of its nearest colourful neighbour (as CSS's powerless hues), so that
/// greys, whose hue is noise, don't add hues of their own.
fn unwrap_hues(q: [Vec4; 4], mode: i64) -> [Vec4; 4] {
    let grey = q.map(|c| c[1] < RAMP_ACHROMATIC);
    let hue = |i: usize| q[i][0];
    let h1 = if !grey[1] {
        hue(1)
    } else if !grey[2] {
        hue(2)
    } else if !grey[0] {
        hue(0)
    } else {
        hue(3)
    };
    let h2 = h1
        + if grey[1] || grey[2] {
            0.0
        } else {
            hue_delta(hue(1), hue(2), mode)
        };
    let h0 = if grey[0] {
        h1
    } else {
        h1 - hue_delta(hue(0), h1, if grey[1] { HUE_SHORTER } else { mode })
    };
    let h3 = if grey[3] {
        h2
    } else {
        h2 + hue_delta(h2, hue(3), if grey[2] { HUE_SHORTER } else { mode })
    };
    let with_hue = |c: Vec4, h: f64| [h, c[1], c[2], c[3]];
    [
        with_hue(q[0], h0),
        with_hue(q[1], h1),
        with_hue(q[2], h2),
        with_hue(q[3], h3),
    ]
}

fn sign(n: f64) -> f64 {
    if n > 0.0 {
        1.0
    } else if n < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// A segment's slope at its stop `q1`, by Steffen's method: between those of
/// its two sides, and flat at a peak, so the curve never overshoots.
fn steffen_slope(q0: Vec4, q1: Vec4, q2: Vec4, h0: f64, h1: f64) -> Vec4 {
    let left = if h0 > 0.0 {
        scale(sub(q1, q0), 1.0 / h0)
    } else {
        [0.0; 4]
    };
    let right = if h1 > 0.0 {
        scale(sub(q2, q1), 1.0 / h1)
    } else {
        [0.0; 4]
    };
    let span = h0 + h1;
    let middle = if span > 0.0 {
        scale(add(scale(left, h1), scale(right, h0)), 1.0 / span)
    } else {
        left
    };
    std::array::from_fn(|i| {
        let (l, r, m) = (left[i], right[i], middle[i]);
        (sign(l) + sign(r)) * l.abs().min(r.abs()).min(0.5 * m.abs())
    })
}

/// Interpolate the segment from `q1` to `q2` (at `x1` and `x2`), `q0` and `q3`
/// being their neighbours (at `x0` and `x3`): a copy of `q1` when it's the
/// first stop (`first`), of `q2` when it's the last (`last`).
fn interpolate(
    interpolation: i64,
    x: f64,
    [x0, x1, x2, x3]: [f64; 4],
    [q0, q1, q2, q3]: [Vec4; 4],
    first: bool,
    last: bool,
) -> Vec4 {
    let t = if x2 > x1 {
        clamp01((x - x1) / (x2 - x1))
    } else if x >= x2 {
        1.0
    } else {
        0.0
    };
    match interpolation {
        RAMP_CONSTANT => {
            if t >= 1.0 {
                q2
            } else {
                q1
            }
        }
        RAMP_EASE => mix(q1, q2, t * t * (3.0 - 2.0 * t)),
        RAMP_CARDINAL => {
            let fc = RAMP_CARDINAL_TENSION;
            let (t2, t3) = (t * t, t * t * t);
            weighted(
                [q0, q1, q2, q3],
                [
                    -fc * t3 + 2.0 * fc * t2 - fc * t,
                    (2.0 - fc) * t3 + (fc - 3.0) * t2 + 1.0,
                    (fc - 2.0) * t3 + (3.0 - 2.0 * fc) * t2 + fc * t,
                    fc * t3 - fc * t2,
                ],
            )
        }
        RAMP_B_SPLINE => {
            // End stops are mirrored past the ends, so the curve reaches them.
            let b0 = if first { sub(scale(q1, 2.0), q2) } else { q0 };
            let b3 = if last { sub(scale(q2, 2.0), q1) } else { q3 };
            let (t2, t3) = (t * t, t * t * t);
            weighted(
                [b0, q1, q2, b3],
                [
                    (-t3 + 3.0 * t2 - 3.0 * t + 1.0) / 6.0,
                    (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
                    (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
                    t3 / 6.0,
                ],
            )
        }
        RAMP_SMOOTH => {
            // Flat at the end stops, as the ramp is past them.
            let h = x2 - x1;
            let m1 = if first {
                [0.0; 4]
            } else {
                steffen_slope(q0, q1, q2, x1 - x0, h)
            };
            let m2 = if last {
                [0.0; 4]
            } else {
                steffen_slope(q1, q2, q3, h, x3 - x2)
            };
            let (t2, t3) = (t * t, t * t * t);
            add(
                weighted(
                    [q1, q2, [0.0; 4], [0.0; 4]],
                    [2.0 * t3 - 3.0 * t2 + 1.0, -2.0 * t3 + 3.0 * t2, 0.0, 0.0],
                ),
                weighted(
                    [m1, m2, [0.0; 4], [0.0; 4]],
                    [(t3 - 2.0 * t2 + t) * h, (t3 - t2) * h, 0.0, 0.0],
                ),
            )
        }
        _ => mix(q1, q2, t),
    }
}

/// Each stop's colour in `modes`' space, as `evaluate_converted` takes them.
pub fn convert_stops(stops: &[RampColorStop], modes: RampModes) -> Vec<RampColorStop> {
    stops
        .iter()
        .map(|s| RampColorStop {
            position: s.position,
            color: ramp_to_space(s.color, modes.space),
        })
        .collect()
}

/// The colour (linear RGB and alpha) of a ramp at `x`.
pub fn evaluate_ramp(stops: &[RampColorStop], x: f64, modes: RampModes) -> Vec4 {
    match stops {
        [] => [0.0, 0.0, 0.0, 1.0],
        [only] => only.color,
        _ => evaluate_converted(&convert_stops(stops, modes), x, modes),
    }
}

/// `evaluate_ramp` for stops already given by `convert_stops`, so a ramp
/// sampled many times converts its stops once.
pub fn evaluate_converted(stops: &[RampColorStop], x: f64, modes: RampModes) -> Vec4 {
    let count = stops.len();
    let Some(only) = stops.first() else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    if count < 2 {
        return ramp_from_space(only.color, modes.space);
    }
    let i = stops
        .iter()
        .enumerate()
        .take(count - 1)
        .skip(1)
        .rev()
        .find(|(_, stop)| stop.position <= x)
        .map_or(0, |(k, _)| k);
    let at = |k: isize| {
        stops
            .get(k.clamp(0, count as isize - 1) as usize)
            .copied()
            .unwrap_or(*only)
    };
    let i_signed = i as isize;
    let around = [
        at(i_signed - 1),
        at(i_signed),
        at(i_signed + 1),
        at(i_signed + 2),
    ];
    let first = i == 0;
    let last = i + 2 >= count;
    let RampModes {
        interpolation,
        space,
        hue,
    } = modes;
    let mut q = around.map(|s| s.color);
    if space == RAMP_HSV || space == RAMP_OKLCH {
        q = unwrap_hues(q, hue);
    }
    if first {
        q[0] = q[1];
    }
    if last {
        q[3] = q[2];
    }
    let positions = around.map(|s| s.position);
    ramp_from_space(
        interpolate(interpolation, x, positions, q, first, last),
        space,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{ColorMode, color_to_linear};

    const RED: Vec4 = [1.0, 0.0, 0.0, 1.0];
    const GREEN: Vec4 = [0.0, 1.0, 0.0, 1.0];
    const BLUE: Vec4 = [0.0, 0.0, 1.0, 1.0];
    const WHITE: Vec4 = [1.0; 4];

    fn modes(interpolation: i64, space: i64, hue: i64) -> RampModes {
        RampModes {
            interpolation,
            space,
            hue,
        }
    }

    fn stops(colors: &[(f64, Vec4)]) -> Vec<RampColorStop> {
        colors
            .iter()
            .map(|&(position, color)| RampColorStop { position, color })
            .collect()
    }

    fn close(actual: Vec4, expected: Vec4, tolerance: f64) {
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < tolerance, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn keeps_the_end_colours_past_the_end_stops() {
        let ramp = stops(&[(0.2, RED), (0.6, BLUE)]);
        for (interpolation, _) in RAMP_INTERPOLATIONS {
            for (space, _) in RAMP_SPACES {
                let m = modes(interpolation, space, HUE_SHORTER);
                close(evaluate_ramp(&ramp, 0.0, m), RED, 1e-4);
                close(evaluate_ramp(&ramp, 0.2, m), RED, 1e-4);
                close(evaluate_ramp(&ramp, 0.6, m), BLUE, 1e-4);
                close(evaluate_ramp(&ramp, 1.5, m), BLUE, 1e-4);
            }
        }
    }

    #[test]
    fn passes_through_every_stop() {
        let many = stops(&[(0.0, RED), (0.25, GREEN), (0.5, WHITE), (0.9, BLUE)]);
        for (interpolation, _) in RAMP_INTERPOLATIONS {
            if interpolation == RAMP_B_SPLINE {
                continue;
            }
            for stop in &many {
                let m = modes(interpolation, RAMP_RGB, HUE_SHORTER);
                close(evaluate_ramp(&many, stop.position, m), stop.color, 1e-5);
            }
        }
    }

    #[test]
    fn interpolates_each_mode() {
        let ramp = stops(&[(0.2, RED), (0.6, BLUE)]);
        let at = |interpolation, x| {
            evaluate_ramp(&ramp, x, modes(interpolation, RAMP_RGB, HUE_SHORTER))[0]
        };
        assert!((at(RAMP_LINEAR, 0.3) - 0.75).abs() < 1e-5);
        assert!((at(RAMP_EASE, 0.3) - (1.0 - (0.0625 * 3.0 - 2.0 * 0.015625))).abs() < 1e-5);
        assert_eq!(at(RAMP_CONSTANT, 0.59), 1.0);
        assert_eq!(at(RAMP_CONSTANT, 0.6), 0.0);
    }

    #[test]
    fn doesnt_overshoot_when_smooth() {
        let bumpy = stops(&[
            (0.0, [0.0, 0.0, 0.0, 1.0]),
            (0.1, [1.0; 4]),
            (0.9, [1.0; 4]),
            (1.0, [0.0, 0.0, 0.0, 1.0]),
        ]);
        let peak = |interpolation| {
            let m = modes(interpolation, RAMP_RGB, HUE_SHORTER);
            (0..=100)
                .map(|i| evaluate_ramp(&bumpy, f64::from(i) / 100.0, m)[0])
                .fold(0.0, f64::max)
        };
        assert!((peak(RAMP_SMOOTH) - 1.0).abs() < 1e-6);
        assert!(peak(RAMP_CARDINAL) > 1.0);
        // Smooth where the linear ramp has a corner.
        let corner = stops(&[
            (0.0, [0.0, 0.0, 0.0, 1.0]),
            (0.5, [1.0, 0.0, 0.0, 1.0]),
            (1.0, [1.5, 0.0, 0.0, 1.0]),
        ]);
        let smooth = modes(RAMP_SMOOTH, RAMP_RGB, HUE_SHORTER);
        let slope = |x: f64| {
            (evaluate_ramp(&corner, x + 1e-4, smooth)[0]
                - evaluate_ramp(&corner, x - 1e-4, smooth)[0])
                / 2e-4
        };
        assert!((slope(0.5 - 1e-3) - slope(0.5 + 1e-3)).abs() < 0.05);
    }

    #[test]
    fn goes_the_way_around_the_hue_circle_its_interpolation_picks() {
        let a = color_to_linear(ColorMode::Hsv, [0.9, 1.0, 1.0]);
        let b = color_to_linear(ColorMode::Hsv, [0.1, 1.0, 1.0]);
        let hues = stops(&[(0.0, [a.r, a.g, a.b, 1.0]), (1.0, [b.r, b.g, b.b, 1.0])]);
        let hue_at = |hue| {
            let mixed = evaluate_ramp(&hues, 0.5, modes(RAMP_LINEAR, RAMP_HSV, hue));
            ramp_to_space(mixed, RAMP_HSV)[0]
        };
        assert!(hue_at(HUE_SHORTER).abs() < 1e-4 || (hue_at(HUE_SHORTER) - 1.0).abs() < 1e-4);
        assert!((hue_at(HUE_LONGER) - 0.5).abs() < 1e-4);
        assert!(hue_at(HUE_INCREASING).abs() < 1e-4 || (hue_at(HUE_INCREASING) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn gives_a_grey_stop_its_neighbours_hue() {
        let grey = stops(&[(0.0, WHITE), (1.0, BLUE)]);
        let hue = ramp_to_space(BLUE, RAMP_OKLCH)[0];
        for (mode, _) in RAMP_HUES {
            let half = evaluate_ramp(&grey, 0.5, modes(RAMP_LINEAR, RAMP_OKLCH, mode));
            assert!((ramp_to_space(half, RAMP_OKLCH)[0] - hue).abs() < 1e-4);
        }
    }

    #[test]
    fn takes_a_greys_hue_from_its_nearest_colourful_stop_not_from_noise() {
        let around = |grey: Vec4| {
            stops(&[
                (0.0, RED),
                (0.3, [0.0, 0.0, 0.0, 1.0]),
                (0.6, grey),
                (1.0, BLUE),
            ])
        };
        let m = modes(RAMP_B_SPLINE, RAMP_OKLCH, HUE_SHORTER);
        let a = evaluate_ramp(&around([0.5, 0.5, 0.5, 1.0]), 0.55, m);
        let b = evaluate_ramp(&around([0.5, 0.5, 0.5 + 1e-6, 1.0]), 0.55, m);
        close(a, b, 1e-4);
    }

    #[test]
    fn interpolates_alpha() {
        let fade = stops(&[(0.0, [1.0, 0.0, 0.0, 0.0]), (1.0, RED)]);
        let faded = evaluate_ramp(&fade, 0.25, modes(RAMP_LINEAR, RAMP_OKLCH, HUE_SHORTER));
        assert!((faded[3] - 0.25).abs() < 1e-5);
    }

    fn ids(stops: &[RampStop]) -> Vec<&str> {
        stops.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn sorts_and_cleans_stops() {
        let raw = [
            RampStop {
                id: "stop-2".into(),
                position: 0.9,
            },
            RampStop {
                id: "bad".into(),
                position: 0.1,
            },
            RampStop {
                id: "stop-0".into(),
                position: 1.5,
            },
            RampStop {
                id: "stop-2".into(),
                position: 0.0,
            },
            RampStop {
                id: "stop-5".into(),
                position: 0.2,
            },
        ];
        let sorted = sorted_ramp_stops(Some(&raw));
        assert_eq!(ids(&sorted), ["stop-5", "stop-2", "stop-0"]);
        assert_eq!(sorted[2].position, 1.0);
        assert_eq!(sorted_ramp_stops(None), default_ramp());
        assert_eq!(sorted_ramp_stops(Some(&[])), default_ramp());
    }

    #[test]
    fn edits_stops() {
        let ramp = default_ramp();
        let (added, stop) = add_ramp_stop(&ramp, 0.5);
        assert_eq!(stop.id, "stop-2");
        assert_eq!(ids(&added), ["stop-0", "stop-2", "stop-1"]);
        assert_eq!(new_stop_position(&added, Some("stop-0")), 0.25);
        assert_eq!(new_stop_position(&added, Some("stop-1")), 0.75);
        assert_eq!(new_stop_position(&added, None), 0.5);
        assert_eq!(
            new_stop_position(
                &[RampStop {
                    id: "stop-0".into(),
                    position: 0.2
                }],
                Some("stop-0")
            ),
            1.0
        );

        let moved = move_ramp_stop(&added, "stop-0", 0.8);
        assert_eq!(ids(&moved), ["stop-2", "stop-0", "stop-1"]);
        assert_eq!(move_ramp_stop(&added, "stop-0", -3.0)[0].position, 0.0);

        assert_eq!(
            ids(&remove_ramp_stop(&added, "stop-2")),
            ["stop-0", "stop-1"]
        );
        let single = [RampStop {
            id: "stop-0".into(),
            position: 0.3,
        }];
        assert_eq!(remove_ramp_stop(&single, "stop-0"), single);

        let flipped = flip_ramp(&moved);
        assert_eq!(ids(&flipped), ["stop-1", "stop-0", "stop-2"]);
        assert!((flipped[1].position - 0.2).abs() < 1e-9);

        let spread = distribute_ramp(&moved);
        assert_eq!(
            spread.iter().map(|s| s.position).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(distribute_ramp(&single)[0].position, 0.5);
    }

    #[test]
    fn recognises_stop_ids() {
        assert!(is_ramp_stop_id("stop-12"));
        assert!(!is_ramp_stop_id("stop-"));
        assert!(is_ramp_stop_id("stop-1234567890"));
        assert!(!is_ramp_stop_id("stop-1a"));
        assert!(!is_ramp_stop_id("factor"));
    }
}
