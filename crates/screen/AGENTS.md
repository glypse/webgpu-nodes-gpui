# screen crate (agent notes)

Area layout: a window tiled by rectangles of fractional coordinates, with split/join/close/dock/edge-move operations and a gpui `ScreenView` that draws headers, dividers and gestures.

## Module map
- `src/lib.rs`: `Screen<E>` model (pure ops), `Rect`, `Area`, `AreaId`, `Axis`, `Side`, `Part`, `MinSize`, `DockTarget`, `EdgeSelection`, `Invalid`, `neighbour_side`.
- `src/gestures.rs`: corner-drag outcomes (`corner_outcome`, `Outcome`, `split_at`, `highlight`, `apply`). Pixel-to-fraction mapping, no gpui.
- `src/view.rs`: `ScreenView<E>` (gpui entity), `Editor` trait, `AreaContent`, `ScreenEvent`, `toolbar_slot`, `HEADER_HEIGHT`; header/menu/edge-menu/divider rendering; `mod ui_tests` (TestAppContext).
- `src/tests.rs`: model tests (ops, edge picking, validate, id freshness, random-sequence tiling).

## Key types
- `Screen<E>`: `areas: Vec<Area<E>>`, `next_id: u64`. Serde-derived. Ops return `Option<Screen<E>>` (`None` = refused; input untouched).
- `Area<E>`: `id`, `rect: Rect`, `editor: E`. Rect edges are window fractions in 0..=1, top-left origin.
- Ops: `split(id, axis, at, Part, MinSize)`, `join(keep, remove, tol)`, `close(id, tol)`, `dock(src, target, DockTarget, factor, tol)`, `swap(a, b)`, `edge_at(x, y, threshold, extend)`, `edge_limits`, `move_edge`.
- `ScreenView<E>`: `new(screen, render_area, cx)`, `screen()`, `set_screen()` (emits `ScreenEvent::Changed`), `maximized()`, `toggle_maximized()`. Drag state holds a snapshot of the screen taken at drag start.
- `Editor` trait: `all()`, `label()`, `icon()`; `Clone + PartialEq + 'static`. The host's panel enum implements it.

## Host integration
- The host builds `ScreenView::new(screen, |id, editor, window, cx| AreaContent { toolbar, body }, cx)`. `AreaContent` holds `AnyView`s, drawn cached: each must be correct at the size its area gets, and must notify its own entity when it changes, otherwise it won't redraw.
- Host reads the model via `screen()` and replaces it via `set_screen()`; app-side pattern is in `crates/app/src/shell.rs` (`show_screen`, `split_for_properties`).
- Serialization is not in this crate. `crates/app/src/layout.rs` `store`/`load` writes `{areas: [{id, rect, editor}]}` under the project's `layout` key and rebuilds with `Screen::new`, then `validate()`.

## Invariants (verified in code)
- Shared edges are equal `f64`s (`EPSILON` 1e-9 via `same`). Moves rewrite every coordinate equal to the old one (`move_line`), so neighbours stay flush.
- `validate()` requires: non-empty, each area sized and inside [0,1] (plus epsilon), unique ids, no overlaps, total area 1 within 1e-6.
- `Screen::new` assigns ids 0..n by position; ids in a saved file are not kept on load, only checked for uniqueness by the host. Graph state is re-attached by zipping areas with saved views in order (`layout.rs`).
- `fresh_id` takes max(existing id + 1, `next_id`), so ids never collide with deserialized areas.
- `split` refuses parts smaller than `min` or cuts at/beyond the edges. `join`/`close` cut overhangs with `MinSize::NONE` internally.
- `join` lines up ends closer than the tolerance (`align_neighbours`) and refuses if the result isn't tiled. `close` picks the neighbour with the greatest edge-length alignment and never closes the last area.
- `move_edge` clamps to `edge_limits`, which keep each adjacent area at `min` (returns the input unchanged if limits cross).
- `ScreenView::set_screen` clears `maximized` when that area no longer exists.
- Pixel-to-fraction minimums live in `ScreenView` (`min_size`: 96 px wide, header height 36 px tall; `grab_size`: 4 px). The model takes fractions only.

## Adding an area type
- Add a variant to the host's panel enum (`crates/app/src/shell.rs`), return it from `Editor::all`/`label`/`icon`, and handle it in the `render_area` closure there.
- Add its JSON form in `crates/app/src/layout.rs` (`editor_json` / `editor_from_json`) so saved layouts keep it.
- Nothing in this crate changes.

## Tests
- Model and gesture logic: `cargo test -p screen`.
- Gesture/UI tests (`ui_tests`, TestAppContext) are in the same command: `cargo test -p screen ui_tests`.
- Layout checks in the running app: `debug_selector` / `debug_bounds` (see root AGENTS.md).

## Dependencies
- Depends on: `guard` (`SafeListener`, `guard::render`), `theme` (`ActiveTheme`, `IconName`, `popup`), `gpui`, `gpui-component`, `serde`. Dev: `gpui` with `test-support`.
- Dependents: `app` (`layout.rs`, `shell.rs`, `preview.rs` uses `HEADER_HEIGHT` and `toolbar_slot`).
