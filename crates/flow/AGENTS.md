# crates/flow (`gpui-flow`, lib `gpui_flow`)

Node-editor canvas for gpui, modelled on `@xyflow/svelte`. Original source for the behaviour: `/Volumes/T7WA/GitHub/webgpu-nodes/packages/editor`. Repo-wide rules are in the root `AGENTS.md`.

## Model
- `Flow` is controlled. It keeps its own copy of nodes/edges so interactions feel immediate, emits `FlowEvent`s, and the owner must write its result back with `set_nodes` / `set_edges`. Without that write-back the canvas drifts from the app model.
- Owner pattern (see `tests/pointer.rs` `subscribe_events`, `examples/demo.rs`): `cx.subscribe(&flow, |flow, ev: &FlowEvent, cx| ...)`, apply `NodesChange`/`EdgesChange` with `apply_node_changes` / `apply_edge_changes`, then `set_nodes`/`set_edges`.
- Host plugs in via `Flow::new(cx)`, `with_options`/`set_options`, `set_layers` (flow-space painters behind/in front of nodes, plus `Portal`s holding elements), `set_panels` (controls, minimap, corner panels), and a per-node `Node::render(fn(&NodeProps, &mut Window, &mut App) -> AnyElement)`.

## Module map
- `src/lib.rs`: the public surface; everything else is `mod`-private except `edge_path`, `geometry`, `lookup`, `model`.
- `src/flow.rs`: `Flow` entity, `FlowOptions`, `FlowEvent`, `Layers`/`Portal`, the pointer state machine (`enum Drag`, `ConnectionDrag`, `NodeDrag`), animation (`tick`), viewport API, and `Render`. Largest file.
- `src/model.rs`: `Node`, `Edge`, `Connection`, `ConnectionState`, `HandleBounds`, `NodeChange`/`EdgeChange`, `apply_*_changes`, `add_edge`, `reconnect_edge`, `incomers`/`outgoers`.
- `src/node.rs`: `NodeProps` (what a node renderer gets), `Handle` element, and the paint-time `canvas` measurers feeding `Measure`.
- `src/lookup.rs`: `Lookup` (`InternalNode` per node), handle positions, `closest_handle`, `handle_under`, `validate` (connection validity).
- `src/edge_path.rs`: bezier, straight and smooth-step paths returning `Segment`s; `flatten`, `distance_to_polyline` for hit tests.
- `src/geometry.rs`: `Xy`, `Rect`, `Viewport`, `Extent`, fit/zoom/auto-pan/snap helpers.
- `src/background.rs`: dots/lines/cross background with a cached texture (`BackgroundCache`).
- `src/panels.rs`: controls, minimap and the corner `Panel`s.
- `src/stroke.rs`: segment crossing for Alt (reroute) and Ctrl/Cmd (cut) strokes.
- `src/theme.rs`: `FlowTheme` (`dark()`, `light()`), passed to node renderers.
- `examples/demo.rs`: full integration example (`cargo run -p gpui-flow --example demo`).

## Key types and coordinate spaces
- Three spaces: window pixels (`Point<Pixels>`), flow-local pixels (`Flow::local` = window minus `bounds` origin), flow units (`Xy`). `Viewport { x, y, zoom }`: `screen = flow * zoom + (x, y)`. Use `screen_to_flow`, `flow_to_screen`, `node_rect`, `handle_centres` rather than redoing this maths.
- Node sizes and positions are flow units. `NodeProps::scaled(v)` converts flow units to px at the current zoom.
- Handle and node sizes come from paint-time measurement in px, divided by zoom in `rebuild_lookup`.

## Invariants and gotchas
- Measurement is paint-driven. Nodes and handles are measured in a `canvas` during paint, so the first frame has no sizes. `render` calls `rebuild_lookup` and `rebuild_edges` from the measurements of the previous paint. `FlowEvent::Initialized` fires once every visible node is measured; the initial fit (`init_fit_pending`) waits for `ready`.
- `set_nodes` drops measurements of removed ids. During a started node drag it keeps the dragged nodes' current positions and sets `dragging`, so an owner echoing back stale positions does not stutter the drag.
- Hit testing: nodes are GPUI elements with their own `on_mouse_down`, so GPUI decides which node is hit. Edges are tested by polyline distance in flow units, topmost (last drawn) first, in `edge_at`. Anything else falls through to `pane_down`.
- Edge hit reach is `interaction_width / 2`. Edge ends are checked by `edge_end_at`, and only when `edges_reconnectable`.
- Handlers use `cx.safe_listener` on the root and the pane; the canvas and measurement closures use `guard::guard`; `render` is wrapped in `guard::render("flow", ...)`. Keep to that when adding handlers.
- Animations (`anim`) advance in `tick`, called from `render`, and request the next frame from there. Do not request frames from event handlers (see root AGENTS.md).
- Performance: the flow is meant to sit inside a cached parent view (`.cached(...)`), so only the flow's own `notify` re-renders it. The test `Host`/`Inner` pattern in `tests/animation.rs` reproduces the app.
- `FlowOptions::default()` is the starting point; `snap_grid`, `connection_mode`, `selection_mode`, `min_zoom`/`max_zoom`, `delete_key`, `strokes` and `connect_on_click` are the usual knobs.

## Adding a common thing
- New interaction: add a `Drag` variant or a branch in `pane_down`/`node_down`/`mouse_move`/`mouse_up`, emit a `FlowEvent` for it, and add a `tests/pointer.rs` case.
- New event: add a variant to `FlowEvent` (`src/flow.rs`) and emit it with `cx.emit`; subscribers already match on `_ => {}`.
- New node/edge field: add it to `model.rs` with a builder method, and handle it in `rebuild_edges` / `rebuild_lookup` if it affects geometry.
- New edge path type: add a function in `edge_path.rs` returning `EdgePath`, then extend `edge_geometry` in `flow.rs`.

## Tests
- Run: `cargo test -p gpui-flow` (unit tests in `src/*.rs`, integration tests in `tests/`). The package name is `gpui-flow`, not `flow`.
- `tests/pointer.rs`: `#[gpui::test]` with `TestAppContext`. `setup(cx, with_edge)` adds a window view with `Flow::new`, `fit_view_on_init = true`, two cards `a` and `b`, and a subscriber that applies changes as an owner would. `settle` runs `run_until_parked` and `window.refresh()` until measurement and the initial fit settle.
- Drive input with `cx.simulate_mouse_down/move/up` and `simulate_click` (the delete-key and arrow-key tests show key input), and locate targets with `screen_of(flow, cx, id, local)` (uses `node_rect` + `flow_to_screen`). Nodes carry `.debug_selector(...)` so layout can be checked with `debug_bounds`.
- Assert that no panic was swallowed: `assert_eq!(guard::recovered(), before)` around each test.
- `tests/animation.rs`: frame-by-frame tests using `window.simulate_next_frame` and `window.draw`, with `Host`/`Inner` wrapped in `.cached(...)`.
- Pure unit tests: `geometry`, `edge_path`, `lookup`, `model`, `background`, `panels`, `stroke`.

## Dependencies and dependents
- Depends on: `guard` (path, panic guards), `gpui` (workspace, the gpui-fast fork), `log`. Dev-only: `gpui` with `test-support`, `gpui_platform`.
- Dependents: `crates/app` (`gpui-flow = { path = "../flow" }`); its `src/editor.rs` is the main host, with `src/graph/*` also using the crate.
