# theme crate

Design tokens and the editor's gpui UI components. Original: `/Volumes/T7WA/GitHub/webgpu-nodes/packages/theme` (shadcn neutral palette, Atkinson Hyperlegible font, Phosphor icons).

Builds on `gpui-component` (0.10, `gpui-fast` feature) for its widgets (`Button`, `Input`, `PopupMenu`, `Disableable`, ...) and `gpui-kit-assets` (its icons, served as a fallback by `Assets`). Phosphor SVGs come from `phosphor-svgs`.

## Module map
- `src/lib.rs`: `init` (font, gpui-component, default Dark theme), `sync_component_theme`, re-exports.
- `src/theme.rs`: `Theme` global, `Colors` (light/dark shadcn palette), `Radii`, `TextSizes`, `Appearance`, `ActiveTheme`.
- `src/color.rs`: `oklch`/`oklch_a`, OKLCH to sRGB `Hsla` conversion (CSS-matching).
- `src/icon.rs`: `IconName` (macro-generated enum), `Assets` (AssetSource), `Icon` (RenderOnce).
- `src/popup.rs`: popup menus, cascading menus (`Cascade`, `Row`), panels and `layer` placed at window positions.
- `src/number_field.rs`: `NumberField` scrub/type numeric entity.
- `src/color_picker.rs`: `ColorPicker` entity (wheels + number fields + hex input).
- `src/color_wheels.rs`: `ColorWheels` entity (wheel + lightness/chroma bar), `oklch_max_chroma`, wheel geometry helpers.
- `src/gamut_image.rs`: `paint_gamut` pixel painter and the process-wide `plane_image` LRU.
- `src/curve_editor.rs`: `CurveEditor` entity and `curve_path` (bezier path steps).
- `src/ramp_editor.rs`: `RampEditor` entity (colour ramp stops on a lane).
- `assets/fonts/`: bundled Atkinson font (embedded via `include_bytes!`).

## Tokens
- Read with `cx.theme()` (`ActiveTheme`), then `.colors.*`, `.radius.{sm,md,lg,xl}`, `.text.{xs,sm,base}`, `.font`, `.mono_font`.
- Colours are authored as OKLCH constants in `Colors::light()` / `Colors::dark()`; the rest are literals in `Theme::new`.
- Theme is a gpui global. The app sets it in `crates/app/src/preferences.rs` (`apply`), which also calls `sync_component_theme`. Do not replace it elsewhere without the component sync.

## Component conventions
- Entities (`Render` + `EventEmitter<XEvent>`): `ColorPicker`, `ColorWheels`, `NumberField`, `CurveEditor`, `RampEditor`, `Cascade`. Create with `cx.new(|cx| X::new(...))`, subscribe with `cx.subscribe`.
- `RenderOnce`: only `Icon`. Use it for stateless visuals.
- Builder setters take `self` and return `Self` (`.alpha(true).gamut(..).disabled(..)`), applied before first render.
- Runtime setters are `set_*(value, cx)` and call `cx.notify()`. Some (`set_zoom`) skip the notify when the value is unchanged.
- Events are plain enums sent by the entity (`NumberEvent::Change(f64)`, `ColorWheelsEvent::Change([f64; 3])`); owners react in a subscription.
- State ownership: the owning view holds the `Entity<..>` and its `Subscription`s. Dropping the subscription stops events.
- Zoom: owners call `set_zoom` to scale drawn sizes and hit radii; sizes elsewhere are "at zoom 1".
- Listeners use `cx.safe_listener` and renders use `guard::render`, as in the root AGENTS.md.

## Invariants and gotchas
- `gamut_image::PLANES` is a process-wide `LruCache` (64 entries). Keys are quantised (`PlaneKey` lightness in 1/400ths, chroma 1/1000ths, hue 1/10ths) and the image is generated from the quantised values, so equal keys paint identically. Keep quantisation in the key, or equal keys diverge.
- `ImageCache::new(0)` clamps to capacity 1 rather than panicking.
- `ColorPicker` remembers its last mode in a static `LAST_MODE` (AtomicU8), so new pickers open in the mode last used anywhere.
- `Theme::sync` swaps the global but does not update gpui-component's palette; nothing calls it, and `preferences::apply` is the path that does both.
- `lib.rs::init` `expect`s the bundled font; that's compiled-in data, so it can't fail at runtime without a build error.
- `ColorPicker`/`ColorWheels` use `gamut`/`space` from `graph-math`; colour maths lives there, not here. `color.rs` is only the sRGB conversion for the theme's own colours.
- Icons: a new `IconName` needs both the Phosphor weight/const and the file stem. `Assets::list` merges in `gpui_kit_assets`.

## Adding a component
1. New `src/foo.rs`; keep public types minimal and re-export them from `lib.rs`.
2. Entity or `RenderOnce`, per the conventions above. Wrap `Render::render` in `guard::render("Foo", || ...)`; wrap `canvas` closures and other callbacks in `guard::guard`.
3. Add `#[gpui::test]` tests in the file (see `TestAppContext`, `VisualTestContext`, and `cx.debug_bounds("selector")` in `color_wheels.rs` or `number_field.rs`).
4. Register the module in `lib.rs`.

## Adding a token
- Add the field to `Colors`/`Radii`/`TextSizes`/`Theme` and set it in both `Colors::light`/`dark` (or `Theme::new`). Use OKLCH via `oklch(l, c, h)` and check the sRGB result against the original in a test (see `color.rs` tests).

## Tests
- `cargo test -p theme` (gpui `test-support` is a dev-dep). `gamut_image.rs` tests exercise `paint_gamut` and `ImageCache` directly, without a window.
- Icon assets: `icon.rs` test checks every `IconName` resolves to SVG through `Assets`.

## Dependencies
- Workspace deps: `gpui`, `gpui-component`, `gpui-kit-assets`, `graph-math`, `guard`, `phosphor-svgs`, `image`, `lru`, `unicode-segmentation`.
- Dependents: `crates/app` and `crates/screen`.
