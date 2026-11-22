# crates/app (`webgpu-nodes`): the editor binary

Purpose: the gpui window app (node editor, preview, timeline, code view, properties, output windows). Port of the original editor in `/Volumes/T7WA/GitHub/webgpu-nodes/packages/editor` and the shell in `/Volumes/T7WA/GitHub/webgpu-nodes/apps/web`. Read `docs/probing.md` for screenshot/profile/bounded-run usage.

## Module map (`src/`)
- `main.rs`: entry point. Parses CLI, installs probe, starts gpui, inits `theme`, `preferences`, `shell`, `windows`, `editor`; builds the menu bar (`menus()`, `refresh_menus`, `set_playing`); `wind_down` for bounded runs and `--screenshot`.
- `cli.rs`: clap `Cli` (all flags), `Size`, `--zoom` parser. `records()` turns probing on; `bounded()` makes the app quit itself.
- `shell.rs`: `Shell`, the root view of a project window. Owns the screen, playback, editor, properties panel, file path, discard prompts. Defines `Panel` (screen areas), `Start`, most app actions, default layout, keymap (`init`).
- `windows.rs`: window registry (`Registry` global), opening project/preferences/output windows, find/activate, owner-closed handling.
- `editor.rs` (+ `editor/handle.rs`, `editor/widget_edits.rs`): the graph `Editor` model entity, undo/redo, layout actions, add menu, popups; emits `EditorEvent`. `handle.rs` is the model shared by several graph areas (each with its own group path and selection).
- `graph/`: graph-area widgets: node cards, sockets, fields, sheet, frames, zones, breadcrumbs, add/type pickers, popovers.
- `playback.rs`: `Playback` entity: compiles the shader in the background, runs the clock, uniforms, previews' render sizes, frame/PNG export. Emits `PlaybackEvent::Changed`.
- `preview.rs`: `PreviewArea` (zoom/fit/actual size) and `PreviewToolbar`.
- `timeline.rs`: timeline bar, readout and timing controls.
- `code_view.rs`: shows the generated WGSL.
- `properties.rs`: `PropertiesPanel` (`Section`: Canvas, Time, Colour, Output); Canvas/Time/Colour save with the project settings, Output is the video export.
- `video_export.rs`: `OutputSettings` (stored with the project) and `ExportJob` (thread from the `export` crate, polled for progress).
- `output.rs`: output window view (just the playback's last frame).
- `preferences.rs`: user prefs file in the config dir (`dirs::config_dir()/webgpu-nodes/preferences.json`); `preferences_window.rs` is its window.
- `project.rs`: reads/writes project files: bare JSON or zip bundle (`project.json` + assets); last-opened path in the config dir.
- `layout.rs`: screen layout as saved in the project (areas, rects, graph path and camera per graph area). `editor_json` matches on `Panel`.
- `assets.rs`: importing media files as project assets.
- `group_bundles.rs`: export selected group / import groups as bundle files.
- `probe.rs`: `--profile`/`--frames` tracing and screenshot helper (`save_window`). Nothing recorded unless a probe flag is set.
- `perf.rs`, `scenes.rs`: test-only (benchmark regression tests, scene load/validate tests).

## Ownership and events
- `Shell` holds `Entity<Editor>`, `Entity<Playback>`, `Entity<PropertiesPanel>`, `Entity<ScreenView<Panel>>`.
- Editor → Shell → Playback: `Shell::new` subscribes to `EditorEvent` and forwards `Shader`→`Playback::set_source`, `Uniforms`→`set_uniforms`, `Settings`→`apply_settings`. `ExportGroup` is handled by the shell. Playback → Shell: `PlaybackEvent::Changed` → `adopt_playback_settings`, menus refresh.
- Model edits go through `Editor` methods, which emit `EditorEvent`; don't push state into `Playback` directly from views.
- Graph areas read the shared model via `editor/handle.rs`; the `Editor` itself is one entity per project window.

## Actions and input
- Actions are declared with `actions!` in the owning module (`shell.rs`, `editor.rs`, `windows.rs`, `preferences.rs`, `main.rs` for `Quit`). Keys are bound in each module's `init`/`cx.bind_keys`, scoped by key context (`"Shell"`, `CONTEXT`).
- Handlers: view-level `.on_action(cx.safe_listener(...))` in `render`; app-level `cx.on_action` only in `init`.
- Menus are data in `main.rs::menus()`. Labels/checks are read only when set, so call `refresh_menus` after a change to preferences or playback state.

## Invariants (seen in code)
- Output-window settings are not in the editor's undo history; `Shell::saved_output` tracks them separately for unsaved-change detection.
- `Shell::confirming` stops repeated discard prompts; quitting goes through `shell::request_quit` (deferred, so the focused window finishes its update first).
- Shader compile runs off the UI thread; the previous program stays shown until the new one is ready.
- Bounded runs: `--frames` quits after `STALL` (5 s) with no new frame; `--seconds` is wall time from launch. Profile summary is printed from `on_app_quit`.
- Project load: a zip bundle's assets are verified against the project; altered ones are dropped with a warning.
- `--screenshot` exit status is non-zero if the PNG wasn't written.

## Adding common things
- Action: declare in the module's `actions!`, bind a key if any, handle with `safe_listener` on the owning view, add a `MenuItem::action` in `main.rs::menus()`, and extend the label test in `main.rs` if the item is in a checked list.
- Screen panel/area: add a `Panel` variant (`shell.rs`, `impl screen::Editor` `all()`), handle it in the `ScreenView::new` factory in `Shell::new`, and add its case to `layout.rs::editor_json`.
- Properties section: `properties.rs` (`Section`, `SectionInfo`, `impl Render for PropertiesPanel`). Saved project settings go through `Editor::update_settings` and `EditorEvent::Settings`.
- User preference: field on `preferences::Preferences` and a control in `preferences_window.rs`.
- Graph node widget: `graph/node.rs`, `graph/widgets.rs`, `graph/fields.rs`.

## Tests
- Unit tests live in `#[cfg(test)] mod tests` in most files; GPU/UI tests use `TestAppContext`.
- All: `cargo test -p webgpu-nodes`
- Scenes only: `cargo test -p webgpu-nodes scenes`
- Perf (ignored, timing): `cargo test -p webgpu-nodes perf -- --ignored --nocapture`
- Flags and CLI behaviour: `cargo run -p webgpu-nodes -- --help`; `cli.rs` has parse tests.

## Dependencies (workspace crates)
- `guard` (panic guards, `SafeListener`), `gpui-flow` (`crates/flow`, canvas), `shader-graph` (model, codegen), `runtime` (wgpu renderer, player), `export` (video/PNG encoding), `theme` (tokens, components and `theme::Assets`), `screen` (area layout).
- Dev-only: `naga` (WGSL validation in tests).
