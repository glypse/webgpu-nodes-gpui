# graph-math agent notes

Colour, curve and colour-ramp maths shared by the node graph (its WGSL computes the same) and the editor widgets.

## Module map
- `src/lib.rs`: exports `color`, `curve`, `ramp`.
- `src/color/mod.rs`: `ColorSpace` (Srgb, DisplayP3), re-exports the three submodules.
- `src/color/convert.rs`: `Rgb`, `Rgba`, `Hsv`, `Lch`; sRGB transfer, RGB<->HSV, linear<->OKLch, hex, hue-keeping helpers.
- `src/color/modes.rs`: `ColorMode` (Rgb, Hsv, Oklch) with parse/name; `color_to_linear`, `linear_to_color`, `convert_color_mode`.
- `src/color/display.rs`: sRGB<->Display P3 matrices, `to_gamut`/`from_gamut`/`clip_to_gamut`/`in_gamut`, `wgsl_mat3`.
- `src/curve.rs`: `CurvePoint` (kind: vector/free/aligned/mirrored), cubic segments, evaluation, point/handle editing, presets.
- `src/ramp.rs`: `RampStop`, `RampColorStop`, `RampModes`, stop editing, `ramp_to_space`/`ramp_from_space`, `hue_delta`, `evaluate_ramp` (or `convert_stops` once + `evaluate_converted` per sample); interpolation and hue-mode code constants.

## Key items
- Colour: `Rgb`/`Rgba` (linear sRGB), `encode_in`/`decode_from`, `linear_to_oklch`/`oklch_to_linear`, `hex_to_linear`/`linear_to_hex`.
- Curve: `evaluate_curve`, `curve_segments`, `segment_parameter` (Newton), `move_curve_point`, `move_curve_handle`, `set_curve_point_kind`, `curve_presets`.
- Ramp: `evaluate_ramp`, `sorted_ramp_stops`, `add_ramp_stop`, `move_ramp_stop`, `distribute_ramp`, `RAMP_*`/`HUE_*` i64 codes (shader-graph re-exports `ramp::*`).
- Consumers: `shader-graph` (`evaluate.rs`, `color`, `curve`, `ramp`), `theme` (color pickers, wheels, gamut image, curve and ramp editors).

## External crates (workspace Cargo.toml)
- `palette` 0.7 (no default features, `std`): OKLab/OKLch, sRGB transfer and HSV conversions.
- `kurbo`: cubic root solvers (`solve_cubic`, `solve_quadratic`) and `CubicBez` for curves.
- `serde`: derives on the enum/struct types that are saved in graphs.

## Invariants (checked in code)
- Colours are linear sRGB, unbounded: components below 0 or above 1 are valid (HDR, colours outside sRGB). Only swatches, hex and display clip.
- `srgb_encode`/`srgb_decode` clamp negative inputs to 0 first, so out-of-gamut negatives do not survive an encode.
- HSV is of the sRGB-encoded colour. Hue is normalised to [0, 1) in `hsv_to_rgb`.
- OKLch hue is in degrees; `Lch` chroma is unbounded above.
- Display P3 matrices in `display.rs` are the CSS Color 4 rational values, deliberately kept over `palette`'s derived ones (about 3e-5 apart). Don't "fix" them to match palette.
- The OKLab matrices (`LINEAR_RGB_TO_LMS`, `LMS_TO_OKLAB`, ...) are emitted to WGSL; a test checks they agree with `palette`.
- `wgsl_mat3` takes a row-by-row `Mat3` and builds the WGSL `mat3x3f` from its columns (WGSL constructors take columns), so the shader matrix equals the Rust one.
- Curve: outside the first and last points the curve is flat; neighbouring points form cubic segments; vector points join by straight lines whatever handles they keep; handles are shortened so x keeps growing.
- `CURVE_SOLVE_STEPS` (16) is the Newton iteration count the generated WGSL uses. It is the shader's only accuracy knob: change it on both sides.
- Ramp: `evaluate_ramp` mirrors the shader stage by stage (its doc says the two must change together). Past the end stops, the end colours are kept. `Smooth` is a monotone cubic (no overshoot). Grey detection uses `RAMP_ACHROMATIC` (1e-4); `RAMP_CARDINAL_TENSION` is 0.71.
- Ramp stop positions are passed to the shader as uniforms, so moving a stop does not change the compiled shader.

## Tests
- Inline `#[cfg(test)] mod tests` in each source file (`color/display.rs`, `color/modes.rs`, `color/convert.rs`, `curve.rs`, `ramp.rs`). No `tests/` directory.
- Run: `cargo test -p graph-math` (from the repo root; shared `target/`).

## Workspace dependents
- `crates/shader-graph/Cargo.toml`: `graph-math = { path = "../graph-math" }`.
- `crates/theme/Cargo.toml`: `graph-math = { path = "../graph-math" }`.
